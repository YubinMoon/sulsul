//! 가속 장치 선택과 실행 중 장치 감지.
//!
//! Windows에서는 ONNX Runtime의 DirectML 실행 공급자(EP)로 GPU나 NPU를 씁니다.
//! 어떤 장치가 있는지는 DXCore 어댑터 열거로 알아내고, 결과는 프로세스 동안 캐시합니다.

use std::sync::OnceLock;

/// 설정 `device`의 값
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePreference {
    /// NPU → GPU → CPU 순서로, 이 PC에 있는 장치부터 시도합니다.
    Auto,
    Npu,
    Gpu,
    Cpu,
}

impl DevicePreference {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Npu, Self::Gpu, Self::Cpu];

    /// 설정 파일에 저장되는 값
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Npu => "npu",
            Self::Gpu => "gpu",
            Self::Cpu => "cpu",
        }
    }

    /// 모르는 값이면 `None`. 스키마 검사를 거친 값이면 항상 `Some`입니다.
    pub fn from_setting(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == value)
    }
}

/// 이 PC에서 찾은 가속 장치 이름. 각 목록은 DirectML이 고르는 순서(하드웨어, 고성능 우선)입니다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetectedDevices {
    /// `D3D12_GENERIC_ML`만 지원하고 그래픽은 지원하지 않는 어댑터
    pub npus: Vec<String>,
    /// `D3D12_GRAPHICS`를 지원하는 하드웨어 어댑터 (소프트웨어 렌더러 제외)
    pub gpus: Vec<String>,
}

impl DetectedDevices {
    /// DirectML이 쓸 NPU (첫 번째)
    pub fn npu(&self) -> Option<&str> {
        self.npus.first().map(String::as_str)
    }

    /// DirectML이 쓸 GPU (첫 번째)
    pub fn gpu(&self) -> Option<&str> {
        self.gpus.first().map(String::as_str)
    }
}

/// 이 빌드에서 DirectML 가속을 쓸 수 있는지 (Windows 빌드에서만 `true`)
pub const DIRECTML_BUILT: bool = cfg!(windows);

/// 이 PC의 NPU·GPU 목록. 처음 부를 때 한 번 감지하고 결과를 캐시합니다.
///
/// Windows가 아니거나 감지에 실패하면 빈 목록입니다(실패 원인은 `tracing::warn`으로 남깁니다).
pub fn detected_devices() -> &'static DetectedDevices {
    static DETECTED: OnceLock<DetectedDevices> = OnceLock::new();
    DETECTED.get_or_init(|| {
        let devices = detect();
        tracing::debug!(npus = ?devices.npus, gpus = ?devices.gpus, "가속 장치 감지");
        devices
    })
}

#[cfg(not(windows))]
fn detect() -> DetectedDevices {
    DetectedDevices::default()
}

#[cfg(windows)]
fn detect() -> DetectedDevices {
    match dxcore::enumerate() {
        Ok(devices) => devices,
        Err(error) => {
            tracing::warn!("DXCore로 가속 장치를 찾지 못했습니다: {error}");
            DetectedDevices::default()
        }
    }
}

#[cfg(windows)]
mod dxcore {
    use windows::Win32::Graphics::DXCore::{
        DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS, DXCoreCreateAdapterFactory, DXCoreHardwareID,
        DriverDescription, Hardware, HardwareID, HighPerformance, IDXCoreAdapter,
        IDXCoreAdapterFactory, IDXCoreAdapterList, IsHardware,
    };
    use windows::core::GUID;

    use super::DetectedDevices;

    /// `DXCORE_ADAPTER_ATTRIBUTE_D3D12_GENERIC_ML` (Windows SDK `dxcore_interface.h`).
    /// windows 크레이트 0.62에는 이 상수가 없어서 직접 정의합니다.
    const D3D12_GENERIC_ML: GUID = GUID::from_u128(0xb71b0d41_1088_422f_a27c_0250b7d3a988);

    pub(super) fn enumerate() -> windows::core::Result<DetectedDevices> {
        // SAFETY: DXCore 팩토리 생성은 인자가 없는 COM 호출입니다.
        let factory: IDXCoreAdapterFactory = unsafe { DXCoreCreateAdapterFactory() }?;
        let gpus = adapters(&factory, DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS)?
            .iter()
            .filter_map(name)
            .collect();
        // 구버전 Windows는 GENERIC_ML 속성을 모를 수 있으므로 실패하면 NPU가 없는 것으로 봅니다.
        let npus = adapters(&factory, D3D12_GENERIC_ML)
            .unwrap_or_else(|error| {
                tracing::debug!("GENERIC_ML 어댑터를 열거하지 못했습니다: {error}");
                Vec::new()
            })
            .iter()
            // 그래픽도 되는 어댑터는 GPU입니다.
            // SAFETY: 유효한 어댑터에 상수 GUID 포인터를 넘깁니다.
            .filter(|a| unsafe {
                !a.IsAttributeSupported(&DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS)
            })
            .filter_map(name)
            .collect();
        Ok(DetectedDevices { npus, gpus })
    }

    /// `attribute`를 지원하는 하드웨어 어댑터를 DirectML과 같은 기준(하드웨어, 고성능)으로 정렬해 돌려줍니다.
    fn adapters(
        factory: &IDXCoreAdapterFactory,
        attribute: GUID,
    ) -> windows::core::Result<Vec<IDXCoreAdapter>> {
        // SAFETY: 아래 호출은 모두 유효한 COM 객체에 대한 읽기 전용 호출입니다.
        unsafe {
            let list: IDXCoreAdapterList = factory.CreateAdapterList(&[attribute])?;
            list.Sort(&[Hardware, HighPerformance])?;
            let mut out = Vec::new();
            for index in 0..list.GetAdapterCount() {
                let adapter: IDXCoreAdapter = list.GetAdapter(index)?;
                if is_hardware(&adapter) {
                    out.push(adapter);
                }
            }
            Ok(out)
        }
    }

    fn is_hardware(adapter: &IDXCoreAdapter) -> bool {
        let mut value = 0u8;
        // SAFETY: IsHardware 속성은 1바이트 bool이고 버퍼 크기를 그대로 넘깁니다.
        unsafe {
            adapter.IsPropertySupported(IsHardware)
                && adapter
                    .GetProperty(IsHardware, size_of::<u8>(), (&raw mut value).cast())
                    .is_ok()
                && value != 0
        }
    }

    fn name(adapter: &IDXCoreAdapter) -> Option<String> {
        // SAFETY: GetPropertySize로 받은 크기만큼 버퍼를 만들어 넘깁니다.
        unsafe {
            if !adapter.IsPropertySupported(DriverDescription) {
                return None;
            }
            let size = adapter.GetPropertySize(DriverDescription).ok()?;
            let mut buffer = vec![0u8; size];
            adapter
                .GetProperty(DriverDescription, size, buffer.as_mut_ptr().cast())
                .ok()?;
            let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
            let text = String::from_utf8_lossy(&buffer[..end]).trim().to_owned();
            if !text.is_empty() {
                return Some(text);
            }
            // 드라이버 설명이 비어 있으면 PCI 장치 ID로 구분합니다.
            let mut id = DXCoreHardwareID::default();
            adapter
                .GetProperty(
                    HardwareID,
                    size_of::<DXCoreHardwareID>(),
                    (&raw mut id).cast(),
                )
                .ok()?;
            Some(format!("PCI {:04x}:{:04x}", id.vendorID, id.deviceID))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_round_trips() {
        for p in DevicePreference::ALL {
            assert_eq!(DevicePreference::from_setting(p.as_str()), Some(p));
        }
        assert_eq!(DevicePreference::from_setting("tpu"), None);
    }

    #[test]
    fn detection_is_cached_and_does_not_panic() {
        let first = detected_devices();
        let second = detected_devices();
        assert!(std::ptr::eq(first, second));
        if !DIRECTML_BUILT {
            assert_eq!(first, &DetectedDevices::default());
        }
    }
}
