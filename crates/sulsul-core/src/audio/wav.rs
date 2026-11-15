use std::io::{BufWriter, Cursor, Seek, Write};
use std::path::{Path, PathBuf};

use super::{AudioClip, AudioError};

fn pcm_spec(sample_rate: u32) -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

fn write_pcm_samples<W: Write + Seek>(sink: W, clip: &AudioClip) -> Result<(), hound::Error> {
    let mut writer = hound::WavWriter::new(sink, pcm_spec(clip.sample_rate))?;
    for sample in &clip.samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        writer.write_sample(value)?;
    }
    writer.finalize()
}

/// 16비트 PCM 모노 WAV로 저장합니다.
pub fn write_wav(path: &Path, clip: &AudioClip) -> Result<(), AudioError> {
    let error = |e: hound::Error| AudioError::Wav {
        path: path.to_path_buf(),
        message: e.to_string(),
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| error(hound::Error::IoError(e)))?;
    }
    let file = std::fs::File::create(path).map_err(|e| error(hound::Error::IoError(e)))?;
    write_pcm_samples(BufWriter::new(file), clip).map_err(error)
}

/// 16비트 PCM 모노 WAV 바이트를 메모리에 만듭니다.
pub fn encode_wav_bytes(clip: &AudioClip) -> Result<Vec<u8>, AudioError> {
    let mut bytes = Cursor::new(Vec::new());
    write_pcm_samples(&mut bytes, clip).map_err(|e| AudioError::Wav {
        path: PathBuf::from("<memory.wav>"),
        message: e.to_string(),
    })?;
    Ok(bytes.into_inner())
}

/// 엔진·API 연결 동작을 확인하기 위한 짧은 테스트용 오디오(1.5초)를 만듭니다.
pub fn test_audio_clip(sample_rate: u32) -> AudioClip {
    let rate = sample_rate.max(1);
    let len = (rate as usize * 3) / 2;
    let fade_len = (rate as usize / 50).max(1);
    let samples = (0..len)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let env_in = (i as f32 / fade_len as f32).min(1.0);
            let env_out = ((len.saturating_sub(i)) as f32 / fade_len as f32).min(1.0);
            let env = env_in.min(env_out);
            0.05 * env * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
        })
        .collect();
    AudioClip::new(samples, rate)
}

/// WAV 파일을 읽어 모노 f32로 바꿉니다 (정수·실수, 여러 채널 모두 지원).
pub fn read_wav(path: &Path) -> Result<AudioClip, AudioError> {
    let error = |e: hound::Error| AudioError::Wav {
        path: path.to_path_buf(),
        message: e.to_string(),
    };
    let mut reader = hound::WavReader::open(path).map_err(error)?;
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(error)?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1_i64 << (spec.bits_per_sample.max(1) - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(error)?
        }
    };
    let channels = usize::from(spec.channels.max(1));
    let samples = interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect();
    Ok(AudioClip::new(samples, spec.sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/clip.wav");
        let clip = AudioClip::new(vec![0.0, 0.5, -0.5, 1.0, -1.0], 16_000);
        write_wav(&path, &clip).unwrap();

        let back = read_wav(&path).unwrap();
        assert_eq!(back.sample_rate, 16_000);
        assert_eq!(back.samples.len(), clip.samples.len());
        for (a, b) in clip.samples.iter().zip(&back.samples) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
    }

    #[test]
    fn reads_stereo_float_as_mono() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stereo.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for s in [0.2_f32, 0.4, -0.6, 0.0] {
            writer.write_sample(s).unwrap();
        }
        writer.finalize().unwrap();

        let clip = read_wav(&path).unwrap();
        assert_eq!(clip.sample_rate, 48_000);
        assert_eq!(clip.samples.len(), 2);
        assert!((clip.samples[0] - 0.3).abs() < 1e-6);
        assert!((clip.samples[1] + 0.3).abs() < 1e-6);
    }

    #[test]
    fn encodes_test_clip_in_memory() {
        let clip = test_audio_clip(16_000);
        assert_eq!(clip.samples.len(), 24_000);
        let bytes = encode_wav_bytes(&clip).unwrap();
        assert!(bytes.starts_with(b"RIFF"));
        let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().channels, 1);
    }
}
