use std::path::Path;

use super::{AudioClip, AudioError};

/// 16비트 PCM 모노 WAV로 저장합니다.
pub fn write_wav(path: &Path, clip: &AudioClip) -> Result<(), AudioError> {
    let error = |e: hound::Error| AudioError::Wav {
        path: path.to_path_buf(),
        message: e.to_string(),
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| error(hound::Error::IoError(e)))?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: clip.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(error)?;
    for sample in &clip.samples {
        let value = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        writer.write_sample(value).map_err(error)?;
    }
    writer.finalize().map_err(error)
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
}
