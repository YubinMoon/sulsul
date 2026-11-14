use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};

use super::device::find_input_device;
use super::{AudioClip, AudioError};

#[derive(Debug, Clone, Default)]
pub struct RecordingOptions {
    /// 장치 ID 또는 이름. 비어 있으면 기본 장치
    pub device: String,
    /// 이 길이에 닿으면 더 받지 않습니다 ([`Recording::limit_reached`])
    pub max_duration: Option<Duration>,
}

/// 진행 중인 녹음. [`Recording::stop`]으로 끝내고 오디오를 받습니다.
/// 결과가 필요 없으면 그냥 버리면 녹음이 취소됩니다.
pub struct Recording {
    stream: cpal::Stream,
    shared: Arc<Shared>,
    device_name: String,
    sample_rate: u32,
    channels: u16,
}

struct Shared {
    samples: Mutex<Vec<f32>>,
    captured: AtomicUsize,
    /// 마지막 콜백의 최대 음량 (f32 비트)
    level: AtomicU32,
    max_samples: Option<usize>,
    limit_reached: AtomicBool,
    error: Mutex<Option<String>>,
}

impl Recording {
    pub fn start(options: &RecordingOptions) -> Result<Self, AudioError> {
        let device = find_input_device(&options.device)?;
        let supported = device.default_input_config()?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.into();

        let max_samples = options
            .max_duration
            .map(|d| (d.as_secs_f64() * f64::from(config.sample_rate)).ceil() as usize);
        let shared = Arc::new(Shared {
            samples: Mutex::new(Vec::with_capacity(config.sample_rate as usize * 10)),
            captured: AtomicUsize::new(0),
            level: AtomicU32::new(0),
            max_samples,
            limit_reached: AtomicBool::new(false),
            error: Mutex::new(None),
        });

        let stream = match sample_format {
            SampleFormat::I8 => build_stream::<i8>(&device, &config, &shared),
            SampleFormat::I16 => build_stream::<i16>(&device, &config, &shared),
            SampleFormat::I32 => build_stream::<i32>(&device, &config, &shared),
            SampleFormat::U8 => build_stream::<u8>(&device, &config, &shared),
            SampleFormat::U16 => build_stream::<u16>(&device, &config, &shared),
            SampleFormat::U32 => build_stream::<u32>(&device, &config, &shared),
            SampleFormat::F32 => build_stream::<f32>(&device, &config, &shared),
            SampleFormat::F64 => build_stream::<f64>(&device, &config, &shared),
            other => return Err(AudioError::UnsupportedFormat(other.to_string())),
        }?;
        stream.play()?;

        let device_name = device.to_string();
        tracing::info!(
            device = %device_name,
            sample_rate = config.sample_rate,
            channels = config.channels,
            format = %sample_format,
            "recording started"
        );
        Ok(Self {
            stream,
            shared,
            device_name,
            sample_rate: config.sample_rate,
            channels: config.channels,
        })
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// 장치에서 받는 샘플레이트 (결과 [`AudioClip`]도 이 값)
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// 장치 채널 수 (결과는 모노로 합쳐짐)
    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// 현재 입력 음량 (0.0 ~ 1.0)
    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    /// 지금까지 받은 오디오 길이
    pub fn elapsed(&self) -> Duration {
        let captured = self.shared.captured.load(Ordering::Relaxed);
        Duration::from_secs_f64(captured as f64 / f64::from(self.sample_rate))
    }

    /// 최대 녹음 길이에 닿았는지
    pub fn limit_reached(&self) -> bool {
        self.shared.limit_reached.load(Ordering::Relaxed)
    }

    /// 녹음 중 발생한 장치 오류 (예: 마이크 연결 해제)
    pub fn error(&self) -> Option<String> {
        self.shared
            .error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// 녹음을 끝내고 모노 오디오를 돌려줍니다.
    pub fn stop(self) -> Result<AudioClip, AudioError> {
        if let Err(e) = self.stream.pause() {
            tracing::debug!("failed to pause stream before stopping: {e}");
        }
        drop(self.stream);
        let samples = std::mem::take(
            &mut *self
                .shared
                .samples
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        let error = self
            .shared
            .error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match error {
            Some(message) if samples.is_empty() => Err(AudioError::Stream(message)),
            Some(message) => {
                tracing::warn!("recording finished with a stream error: {message}");
                Ok(AudioClip::new(samples, self.sample_rate))
            }
            None => Ok(AudioClip::new(samples, self.sample_rate)),
        }
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels.max(1));
    let data_shared = Arc::clone(shared);
    let error_shared = Arc::clone(shared);
    let stream = device.build_input_stream::<T, _, _>(
        *config,
        move |data: &[T], _| data_shared.push(data, channels),
        move |error: cpal::Error| {
            use cpal::ErrorKind;
            match error.kind() {
                // 일시적인 끊김은 녹음을 멈출 이유가 아님
                ErrorKind::Xrun | ErrorKind::RealtimeDenied => {
                    tracing::debug!("audio stream warning: {error}");
                }
                _ => {
                    tracing::warn!("audio stream error: {error}");
                    *error_shared
                        .error
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(error.to_string());
                }
            }
        },
        None,
    )?;
    Ok(stream)
}

impl Shared {
    /// 오디오 콜백에서 호출됩니다. 채널을 평균 내 모노로 쌓습니다.
    fn push<T>(&self, data: &[T], channels: usize)
    where
        T: Sample,
        f32: FromSample<T>,
    {
        if self.limit_reached.load(Ordering::Relaxed) {
            self.level.store(0, Ordering::Relaxed);
            return;
        }
        let mut samples = self.samples.lock().unwrap_or_else(PoisonError::into_inner);
        let mut peak = 0.0_f32;
        for frame in data.chunks(channels) {
            let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
            let mono = sum / frame.len() as f32;
            peak = peak.max(mono.abs());
            samples.push(mono);
        }
        if let Some(max) = self.max_samples
            && samples.len() >= max
        {
            samples.truncate(max);
            self.limit_reached.store(true, Ordering::Relaxed);
        }
        self.captured.store(samples.len(), Ordering::Relaxed);
        self.level.store(peak.min(1.0).to_bits(), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(max_samples: Option<usize>) -> Shared {
        Shared {
            samples: Mutex::new(Vec::new()),
            captured: AtomicUsize::new(0),
            level: AtomicU32::new(0),
            max_samples,
            limit_reached: AtomicBool::new(false),
            error: Mutex::new(None),
        }
    }

    #[test]
    fn push_mixes_channels_to_mono() {
        let shared = shared(None);
        shared.push(&[0.5_f32, 0.1, -1.0, 0.0], 2);
        assert_eq!(*shared.samples.lock().unwrap(), vec![0.3, -0.5]);
        assert_eq!(f32::from_bits(shared.level.load(Ordering::Relaxed)), 0.5);
        assert_eq!(shared.captured.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn push_converts_integer_samples() {
        let shared = shared(None);
        shared.push(&[i16::MAX, i16::MIN], 1);
        let samples = shared.samples.lock().unwrap().clone();
        assert!((samples[0] - 1.0).abs() < 1e-4);
        assert_eq!(samples[1], -1.0);
    }

    #[test]
    fn push_stops_at_limit() {
        let shared = shared(Some(3));
        shared.push(&[0.1_f32; 2], 1);
        assert!(!shared.limit_reached.load(Ordering::Relaxed));
        shared.push(&[0.1_f32; 2], 1);
        assert!(shared.limit_reached.load(Ordering::Relaxed));
        shared.push(&[0.1_f32; 2], 1);
        assert_eq!(shared.samples.lock().unwrap().len(), 3);
    }
}
