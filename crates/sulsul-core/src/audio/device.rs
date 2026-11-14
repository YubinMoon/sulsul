use cpal::traits::{DeviceTrait, HostTrait};

use super::AudioError;

/// 입력(마이크) 장치 정보
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDevice {
    /// 재부팅해도 유지되는 장치 ID. 설정 파일에는 이 값을 저장합니다.
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
}

/// 사용 가능한 입력 장치 목록
pub fn input_devices() -> Result<Vec<InputDevice>, AudioError> {
    let host = cpal::default_host();
    let default_id = host.default_input_device().and_then(|d| d.id().ok());
    let mut devices = Vec::new();
    for device in host.input_devices()? {
        let id = device.id().ok();
        let config = device.default_input_config().ok();
        devices.push(InputDevice {
            id: id.as_ref().map(ToString::to_string).unwrap_or_default(),
            name: device.to_string(),
            is_default: id.is_some() && id == default_id,
            sample_rate: config.as_ref().map(|c| c.sample_rate()),
            channels: config.as_ref().map(|c| c.channels()),
        });
    }
    Ok(devices)
}

/// 설정 값으로 장치를 찾습니다. 비어 있으면 기본 장치, 아니면 ID → 이름 순으로 찾습니다.
pub(super) fn find_input_device(selector: &str) -> Result<cpal::Device, AudioError> {
    let host = cpal::default_host();
    let selector = selector.trim();
    if selector.is_empty() {
        return host.default_input_device().ok_or(AudioError::NoInputDevice);
    }
    if let Ok(id) = selector.parse::<cpal::DeviceId>()
        && let Some(device) = host.device_by_id(&id)
    {
        return Ok(device);
    }
    host.input_devices()?
        .find(|d| d.to_string() == selector)
        .ok_or_else(|| AudioError::DeviceNotFound(selector.to_owned()))
}
