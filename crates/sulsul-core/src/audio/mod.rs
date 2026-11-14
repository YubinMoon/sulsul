//! 오디오 데이터 타입과 마이크 녹음.
//!
//! 녹음은 장치의 기본 형식(샘플레이트·채널)으로 받아 모노로 합친 뒤 [`AudioClip`]으로
//! 돌려줍니다. 엔진이 원하는 샘플레이트로는 [`AudioClip::resampled`]로 바꿉니다.

mod clip;
mod device;
mod recorder;
mod resample;
mod wav;

pub use clip::AudioClip;
pub use device::{InputDevice, input_devices};
pub use recorder::{Recording, RecordingOptions};
pub use resample::resample;
pub use wav::{read_wav, write_wav};

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no input device is available")]
    NoInputDevice,
    #[error("input device '{0}' was not found")]
    DeviceNotFound(String),
    #[error("microphone access was denied by the system")]
    PermissionDenied,
    #[error("the device is busy or used exclusively by another app")]
    DeviceBusy,
    #[error("unsupported sample format: {0}")]
    UnsupportedFormat(String),
    #[error("audio device error: {0}")]
    Device(String),
    #[error("audio stream error: {0}")]
    Stream(String),
    #[error("resampling failed: {0}")]
    Resample(String),
    #[error("WAV file error ({}): {message}", path.display())]
    Wav { path: PathBuf, message: String },
}

impl From<cpal::Error> for AudioError {
    fn from(error: cpal::Error) -> Self {
        use cpal::ErrorKind;
        match error.kind() {
            ErrorKind::PermissionDenied => Self::PermissionDenied,
            ErrorKind::DeviceBusy => Self::DeviceBusy,
            ErrorKind::DeviceNotAvailable => Self::NoInputDevice,
            _ => Self::Device(error.to_string()),
        }
    }
}
