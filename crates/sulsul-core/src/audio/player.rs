//! 소리 재생: 녹음을 기본 출력 장치로 한 번 들려줍니다.
//!
//! 재생 스레드가 출력 스트림을 만들고, 소리가 끝나거나 멈추라는 신호를 받으면 그 스레드가 스트림을 닫습니다.
//! 그래서 부르는 쪽(화면 스레드)은 장치를 여는 동안에도 기다리지 않습니다.
//!
//! 소리는 장치의 기본 형식으로 내보냅니다. 표본 빈도가 다르면 재생하면서 선형 보간으로 맞추고,
//! 모노 소리를 모든 채널에 똑같이 넣습니다. 공유 모드 출력 스트림이라 다른 소리(예: 앱의 효과음)와
//! 시스템에서 함께 섞여 나고, 서로 끊지 않습니다.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};

use super::{AudioClip, AudioError};

/// 소리가 끝난 뒤 더 내보내는 무음. 장치 버퍼에 남은 끝부분까지 다 들린 뒤에 스트림을 닫습니다.
const TAIL: Duration = Duration::from_millis(200);
/// 재생 스레드가 끝났는지·오류가 났는지 살피는 간격 (멈추라는 신호에는 바로 깨어남)
const POLL_INTERVAL: Duration = Duration::from_millis(20);
/// 장치가 소리를 더 받아 가지 않아도 재생이 끝나도록 두는 여유
const STALL_GRACE: Duration = Duration::from_secs(2);

/// 진행 중인 재생. 버리면 재생을 멈추고 재생 스레드가 끝날 때까지 기다립니다.
pub struct Playback {
    shared: Arc<Shared>,
    /// 보내는 쪽을 버리면 재생 스레드가 멈추라는 뜻으로 알아듣습니다 (값은 보내지 않음).
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Shared {
    /// 출력 콜백이 소리와 끝 무음을 모두 내보냄
    drained: AtomicBool,
    /// 재생 중에 난 스트림 오류 (원문)
    error: Mutex<Option<String>>,
    /// 재생 스레드가 끝나며 남긴 결과
    outcome: Mutex<Option<Result<(), AudioError>>>,
}

impl Shared {
    fn take_outcome(&self) -> Option<Result<(), AudioError>> {
        self.outcome
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

impl Playback {
    /// `clip`을 기본 출력 장치로 재생하기 시작합니다. 기다리지 않고 바로 돌아옵니다.
    ///
    /// 재생이 끝나면(다 들려줬거나, 멈췄거나, 실패했거나) 재생 스레드에서 `on_end`를 한 번 부릅니다.
    /// 화면을 다시 그리게 해서 [`Playback::poll`]로 결과를 챙기게 할 때 씁니다.
    pub fn start(
        clip: Arc<AudioClip>,
        on_end: impl FnOnce() + Send + 'static,
    ) -> Result<Self, AudioError> {
        let shared = Arc::new(Shared::default());
        let (stop, stopped) = mpsc::channel();
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("sulsul-playback".into())
            .spawn(move || {
                // 재생 코드가 패닉해도 부르는 쪽이 끝난 것을 알 수 있도록 맨 먼저 만들어 둡니다.
                let _notify = OnDrop(Some(on_end));
                let outcome = play(&clip, &thread_shared, &stopped);
                if let Err(error) = &outcome {
                    tracing::warn!("playback failed: {error}");
                }
                *thread_shared
                    .outcome
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            })
            .map_err(|error| {
                tracing::warn!("could not start the playback thread: {error}");
                AudioError::Stream(format!("could not start the playback thread: {error}"))
            })?;
        Ok(Self {
            shared,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    /// 재생이 끝났으면 결과를 돌려줍니다 (다 들려줬거나 멈췄으면 `Ok`). 아직 재생 중이면 `None`이고,
    /// 결과는 한 번만 돌려줍니다.
    pub fn poll(&mut self) -> Option<Result<(), AudioError>> {
        if let Some(outcome) = self.shared.take_outcome() {
            self.join();
            return Some(outcome);
        }
        if !self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
            return None;
        }
        // 결과를 남기기 전에 끝난 스레드 (패닉)
        self.join();
        Some(self.shared.take_outcome().unwrap_or_else(|| {
            Err(AudioError::Stream(
                "the playback thread ended without a result".into(),
            ))
        }))
    }

    fn join(&mut self) {
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("the playback thread panicked");
        }
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        // 보내는 쪽을 먼저 버려야 재생 스레드가 깨어나 스트림을 닫고 끝납니다.
        drop(self.stop.take());
        self.join();
    }
}

/// 버려질 때 한 번 부르는 함수
struct OnDrop<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

/// 재생 스레드: 출력 스트림을 열어 소리를 끝까지 들려주거나, 멈추라는 신호를 받을 때까지 기다립니다.
/// 돌아갈 때 스트림을 닫습니다.
fn play(
    clip: &Arc<AudioClip>,
    shared: &Arc<Shared>,
    stop: &Receiver<()>,
) -> Result<(), AudioError> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or(AudioError::NoOutputDevice)?;
    let supported = device.default_output_config().map_err(output_error)?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.into();
    if stop_requested(stop) {
        return Ok(());
    }

    let feed = Feed::new(Arc::clone(clip), config.sample_rate, config.channels);
    let stream = match sample_format {
        SampleFormat::I8 => build_stream::<i8>(&device, &config, feed, shared),
        SampleFormat::I16 => build_stream::<i16>(&device, &config, feed, shared),
        SampleFormat::I32 => build_stream::<i32>(&device, &config, feed, shared),
        SampleFormat::U8 => build_stream::<u8>(&device, &config, feed, shared),
        SampleFormat::U16 => build_stream::<u16>(&device, &config, feed, shared),
        SampleFormat::U32 => build_stream::<u32>(&device, &config, feed, shared),
        SampleFormat::F32 => build_stream::<f32>(&device, &config, feed, shared),
        SampleFormat::F64 => build_stream::<f64>(&device, &config, feed, shared),
        other => return Err(AudioError::UnsupportedFormat(other.to_string())),
    }?;
    stream.play().map_err(output_error)?;
    tracing::info!(
        device = %device,
        sample_rate = config.sample_rate,
        channels = config.channels,
        format = %sample_format,
        seconds = clip.duration().as_secs_f64(),
        "playback started"
    );

    let deadline = Instant::now() + clip.duration() + TAIL + STALL_GRACE;
    loop {
        match stop.recv_timeout(POLL_INTERVAL) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                tracing::debug!("playback stopped");
                return Ok(());
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        let error = shared
            .error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(message) = error {
            return Err(AudioError::Stream(message));
        }
        if shared.drained.load(Ordering::Relaxed) {
            tracing::debug!("playback finished");
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(AudioError::Stream(
                "the output device stopped taking audio".into(),
            ));
        }
    }
}

/// 멈추라는 신호가 왔는지 (보내는 쪽이 버려졌는지)
fn stop_requested(stop: &Receiver<()>) -> bool {
    !matches!(stop.try_recv(), Err(TryRecvError::Empty))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    mut feed: Feed,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let data_shared = Arc::clone(shared);
    let error_shared = Arc::clone(shared);
    device
        .build_output_stream::<T, _, _>(
            *config,
            move |data: &mut [T], _| {
                if feed.fill(data) {
                    data_shared.drained.store(true, Ordering::Relaxed);
                }
            },
            move |error: cpal::Error| {
                use cpal::ErrorKind;
                match error.kind() {
                    // 잠깐의 끊김이나 기본 장치가 바뀌어 따라간 것은 재생을 멈출 이유가 아님
                    ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged => {
                        tracing::debug!("playback stream warning: {error}");
                    }
                    _ => {
                        tracing::warn!("playback stream error: {error}");
                        *error_shared
                            .error
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner) = Some(error.to_string());
                    }
                }
            },
            None,
        )
        .map_err(output_error)
}

/// 출력 장치 오류를 [`AudioError`]로 바꿉니다 (`From<cpal::Error>`는 입력 장치 기준이라 따로 둠).
fn output_error(error: cpal::Error) -> AudioError {
    use cpal::ErrorKind;
    match error.kind() {
        ErrorKind::DeviceNotAvailable => AudioError::NoOutputDevice,
        ErrorKind::DeviceBusy => AudioError::DeviceBusy,
        _ => AudioError::Device(error.to_string()),
    }
}

/// 출력 콜백이 채울 소리: 모노 클립을 장치의 표본 빈도·채널 수에 맞춰 차례로 내보내고,
/// 끝나면 [`TAIL`]만큼 무음을 더 내보냅니다.
struct Feed {
    clip: Arc<AudioClip>,
    /// 출력 프레임 하나마다 원본에서 나아가는 표본 수
    step: f64,
    /// 다음에 내보낼 원본 위치 (표본 단위, 소수는 보간 비율)
    position: f64,
    channels: usize,
    /// 소리가 끝난 뒤 아직 내보내지 않은 무음 프레임 수
    tail_left: usize,
}

impl Feed {
    fn new(clip: Arc<AudioClip>, output_rate: u32, channels: u16) -> Self {
        let playable = clip.sample_rate > 0 && output_rate > 0;
        let (step, position) = if playable {
            (f64::from(clip.sample_rate) / f64::from(output_rate), 0.0)
        } else {
            // 들려줄 수 없는 형식이면 처음부터 끝난 것으로 둡니다.
            (1.0, clip.samples.len() as f64)
        };
        let tail_left = (u128::from(output_rate) * TAIL.as_millis()).div_ceil(1000);
        Self {
            clip,
            step,
            position,
            channels: usize::from(channels.max(1)),
            tail_left: usize::try_from(tail_left).unwrap_or(usize::MAX),
        }
    }

    /// 다음 출력 프레임의 값. 소리가 끝났으면 `None`
    fn next_value(&mut self) -> Option<f32> {
        let samples = &self.clip.samples;
        // 위치는 0 이상이라 버림이 곧 내림입니다.
        let index = self.position as usize;
        let current = *samples.get(index)?;
        let next = samples.get(index + 1).copied().unwrap_or(current);
        let fraction = (self.position - index as f64) as f32;
        self.position += self.step;
        Some((current + (next - current) * fraction).clamp(-1.0, 1.0))
    }

    /// 출력 버퍼를 채웁니다. 소리와 끝 무음을 모두 내보냈으면 `true`
    fn fill<T>(&mut self, out: &mut [T]) -> bool
    where
        T: Sample + FromSample<f32>,
    {
        for frame in out.chunks_mut(self.channels) {
            let value = self.next_value().unwrap_or_else(|| {
                self.tail_left = self.tail_left.saturating_sub(1);
                0.0
            });
            frame.fill(T::from_sample(value));
        }
        self.is_drained()
    }

    fn is_drained(&self) -> bool {
        self.position as usize >= self.clip.samples.len() && self.tail_left == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(samples: Vec<f32>, sample_rate: u32, output_rate: u32, channels: u16) -> Feed {
        Feed::new(
            Arc::new(AudioClip::new(samples, sample_rate)),
            output_rate,
            channels,
        )
    }

    /// 무음 꼬리까지 모두 내보낼 때까지 채운 출력 (프레임 수로 끊어 부름)
    fn drain(feed: &mut Feed, frames_per_call: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for _ in 0..10_000 {
            let mut buffer = vec![9.0_f32; frames_per_call * feed.channels];
            let drained = feed.fill(&mut buffer);
            out.extend(buffer);
            if drained {
                return out;
            }
        }
        panic!("소리를 다 내보낸 뒤에는 끝나야 합니다");
    }

    #[test]
    fn same_rate_plays_samples_unchanged_then_silence() {
        let mut feed = feed(vec![0.1, -0.2, 0.3], 1_000, 1_000, 1);
        let out = drain(&mut feed, 64);
        assert_eq!(out[..3], [0.1, -0.2, 0.3]);
        // 끝 무음은 200ms = 200프레임이고, 버퍼를 다 채우므로 그 뒤도 무음입니다.
        assert!(out.len() >= 3 + 200, "끝 무음이 모자랍니다: {}", out.len());
        assert!(
            out[3..].iter().all(|s| *s == 0.0),
            "끝난 뒤에는 무음이어야 함"
        );
    }

    #[test]
    fn mono_is_copied_to_every_channel() {
        let mut feed = feed(vec![0.5, -0.25], 1_000, 1_000, 2);
        let mut out = [9.0_f32; 6];
        feed.fill(&mut out);
        assert_eq!(out, [0.5, 0.5, -0.25, -0.25, 0.0, 0.0]);
    }

    #[test]
    fn higher_output_rate_interpolates_between_samples() {
        // 1kHz 원본을 2kHz 장치로: 원본 표본 사이에 중간값이 들어갑니다.
        let mut feed = feed(vec![0.0, 1.0, 0.0], 1_000, 2_000, 1);
        let mut out = [9.0_f32; 6];
        feed.fill(&mut out);
        assert_eq!(out, [0.0, 0.5, 1.0, 0.5, 0.0, 0.0]);
    }

    #[test]
    fn lower_output_rate_keeps_the_duration() {
        // 48kHz로 녹음한 1초를 44.1kHz 장치로 들려주면 44,100프레임이 나옵니다.
        let mut feed = feed(vec![0.25; 48_000], 48_000, 44_100, 1);
        let out = drain(&mut feed, 512);
        let sound = out.iter().filter(|s| **s != 0.0).count();
        assert!(
            sound.abs_diff(44_100) <= 1,
            "길이가 유지되어야 함: {sound}프레임"
        );
    }

    #[test]
    fn integer_formats_get_converted_samples_and_centered_silence() {
        let mut feed = feed(vec![1.0, -1.0], 1_000, 1_000, 1);
        let mut out = [0_u16; 3];
        feed.fill(&mut out);
        assert_eq!(out[0], u16::MAX);
        assert_eq!(out[1], 0);
        assert_eq!(out[2], 32_768, "부호 없는 형식의 무음은 가운데 값입니다");
    }

    #[test]
    fn loud_samples_are_clamped() {
        let mut feed = feed(vec![1.5, -3.0], 1_000, 1_000, 1);
        let mut out = [0.0_f32; 2];
        feed.fill(&mut out);
        assert_eq!(out, [1.0, -1.0]);
    }

    #[test]
    fn finishes_only_after_the_tail() {
        let mut feed = feed(vec![0.1; 10], 1_000, 1_000, 1);
        let mut out = [0.0_f32; 10];
        assert!(
            !feed.fill(&mut out),
            "소리만 내보낸 뒤에는 아직 끝나지 않습니다"
        );
        let mut tail = [0.0_f32; 199];
        assert!(!feed.fill(&mut tail), "끝 무음이 한 프레임 남았습니다");
        let mut last = [0.0_f32; 1];
        assert!(feed.fill(&mut last));
    }

    #[test]
    fn empty_or_unplayable_clip_only_plays_the_tail() {
        for mut feed in [
            feed(Vec::new(), 16_000, 48_000, 2),
            feed(vec![0.5; 100], 0, 48_000, 2),
        ] {
            let out = drain(&mut feed, 480);
            assert!(out.iter().all(|s| *s == 0.0), "들려줄 소리가 없습니다");
        }
    }

    #[test]
    fn stop_is_seen_when_the_sender_is_dropped() {
        let (stop, stopped) = mpsc::channel::<()>();
        assert!(!stop_requested(&stopped));
        drop(stop);
        assert!(stop_requested(&stopped));
    }

    /// 실제 기본 출력 장치로 짧은 무음을 들려주고, 긴 소리는 버리는 즉시 멈추는지 봅니다.
    #[test]
    #[ignore = "기본 출력 장치가 필요합니다"]
    fn plays_to_the_default_output_device_and_stops_on_drop() {
        let clip = Arc::new(AudioClip::new(vec![0.0; 1_600], 16_000));
        let (ended, ended_rx) = mpsc::channel();
        let mut playback = Playback::start(clip, move || {
            let _ = ended.send(());
        })
        .unwrap();
        ended_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("짧은 소리는 곧 끝나야 합니다");
        let outcome = playback.poll().expect("끝난 뒤에는 결과가 있어야 합니다");
        outcome.unwrap();
        assert!(playback.poll().is_none(), "결과는 한 번만 돌려줍니다");

        let long = Arc::new(AudioClip::new(vec![0.0; 16_000 * 30], 16_000));
        let playback = Playback::start(long, || {}).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let stopping = Instant::now();
        drop(playback);
        assert!(
            stopping.elapsed() < Duration::from_secs(1),
            "버리면 바로 멈춰야 합니다: {:?}",
            stopping.elapsed()
        );
    }

    #[test]
    fn output_errors_are_told_apart_from_input_errors() {
        use cpal::ErrorKind;
        assert!(matches!(
            output_error(cpal::Error::new(ErrorKind::DeviceNotAvailable)),
            AudioError::NoOutputDevice
        ));
        assert!(matches!(
            output_error(cpal::Error::new(ErrorKind::BackendError)),
            AudioError::Device(_)
        ));
    }
}
