//! 가속 장치 설정과 세션을 만들어 볼 장치 순서.
//!
//! Windows에서는 ONNX Runtime의 DirectML 실행 공급자로 GPU를 씁니다. 어떤 장치가 있는지는 SenseVoice 엔진과
//! 같은 감지 결과([`sulsul_stt_sensevoice::accel::detected_devices`])를 씁니다.
//! GPU를 골라도 GPU에서 도는 것은 인코더뿐이고 디코더는 늘 CPU에서 돕니다. 그쪽이 더 빨랐습니다(`runner.rs` 참고).
//! 그래도 보고하는 장치는 인코더가 쓰는 장치(`DirectML (어댑터 이름)`)입니다.
//!
//! NPU는 지원하지 않습니다. AMD Hawk Point의 NPU가 Windows에 보여도 DirectML에서 쓸 수 있다는
//! 뜻은 아닙니다. 현재 sherpa-onnx Whisper 인코더의 NPU 실행 경로는 검증되지 않았습니다.
//! 공급자 등록이나 세션 생성만으로 NPU 연산이 실행되었다고 판단하지 않습니다.

use sulsul_core::settings::ChoiceOption;
use sulsul_core::stt::ComputeDevice;
use sulsul_stt_sensevoice::accel::DetectedDevices;

/// 설정 `device`의 값
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePreference {
    /// 이 PC에 GPU가 있으면 GPU, 없으면 CPU
    Auto,
    Gpu,
    Cpu,
}

impl DevicePreference {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Gpu, Self::Cpu];

    /// 설정 파일에 저장되는 값
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Gpu => "gpu",
            Self::Cpu => "cpu",
        }
    }

    /// 모르는 값이면 `None`. 스키마 검사를 거친 값이면 항상 `Some`입니다.
    pub fn from_setting(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == value)
    }
}

/// `device` 설정의 선택지. 이 PC에 GPU가 없으면 쓸 수 없음으로 표시하고, 있으면 장치 이름을 붙입니다.
pub(crate) fn device_options(
    detected: &DetectedDevices,
    directml_built: bool,
) -> Vec<ChoiceOption> {
    const PREFIX: &str = "engine.whisper_onnx.settings.device";
    DevicePreference::ALL
        .into_iter()
        .map(|pref| {
            let value = pref.as_str();
            let option = ChoiceOption::new(value).with_label(format!("{PREFIX}_{value}"));
            if pref != DevicePreference::Gpu {
                return option;
            }
            match detected.gpu() {
                _ if !directml_built => option.unavailable(format!("{PREFIX}_not_built")),
                Some(name) => option.with_detail(name),
                None => option.unavailable(format!("{PREFIX}_gpu_missing")),
            }
        })
        .collect()
}

/// 인코더 세션을 만들어 볼 장치. 이름은 감지한 어댑터 이름입니다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    Gpu(String),
    Cpu,
}

impl Target {
    pub(crate) fn compute_device(&self) -> ComputeDevice {
        match self {
            Self::Gpu(name) => ComputeDevice::Gpu(format!("DirectML ({name})")),
            Self::Cpu => ComputeDevice::Cpu,
        }
    }
}

/// 설정과 감지 결과로 시도할 장치 순서를 정합니다. 마지막은 항상 CPU입니다.
///
/// GPU를 골랐는데 쓸 수 없으면 그 이유를 `tracing::warn`으로 남깁니다.
pub(crate) fn plan_targets(
    preference: DevicePreference,
    detected: &DetectedDevices,
    directml_built: bool,
) -> Vec<Target> {
    let mut targets = Vec::new();
    match preference {
        DevicePreference::Cpu => {}
        _ if !directml_built => {
            if preference == DevicePreference::Gpu {
                tracing::warn!(
                    "GPU acceleration requested but this build has no DirectML, using CPU"
                );
            }
        }
        _ => match detected.gpu() {
            Some(name) => targets.push(Target::Gpu(name.to_owned())),
            None if preference == DevicePreference::Gpu => {
                tracing::warn!("GPU acceleration requested but no GPU was found, using CPU");
            }
            None => tracing::debug!("no GPU found, auto uses CPU"),
        },
    }
    targets.push(Target::Cpu);
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devices(npu: bool, gpu: bool) -> DetectedDevices {
        DetectedDevices {
            npus: npu.then(|| "AI Boost".to_owned()).into_iter().collect(),
            gpus: gpu.then(|| "RTX".to_owned()).into_iter().collect(),
        }
    }

    #[test]
    fn preference_round_trips_without_npu() {
        for p in DevicePreference::ALL {
            assert_eq!(DevicePreference::from_setting(p.as_str()), Some(p));
        }
        assert_eq!(DevicePreference::from_setting("npu"), None);
    }

    #[test]
    fn auto_and_gpu_try_gpu_then_cpu() {
        let gpu = Target::Gpu("RTX".into());
        for pref in [DevicePreference::Auto, DevicePreference::Gpu] {
            assert_eq!(
                plan_targets(pref, &devices(false, true), true),
                vec![gpu.clone(), Target::Cpu]
            );
            // GPU가 없거나 DirectML이 없는 빌드면 CPU만 씁니다.
            assert_eq!(
                plan_targets(pref, &devices(true, false), true),
                vec![Target::Cpu]
            );
            assert_eq!(
                plan_targets(pref, &devices(false, true), false),
                vec![Target::Cpu]
            );
        }
        assert_eq!(
            plan_targets(DevicePreference::Cpu, &devices(true, true), true),
            vec![Target::Cpu]
        );
    }

    #[test]
    fn npu_is_never_tried() {
        let only_npu = devices(true, false);
        for pref in DevicePreference::ALL {
            assert_eq!(plan_targets(pref, &only_npu, true), vec![Target::Cpu]);
        }
    }

    #[test]
    fn target_reports_device_kind_and_name() {
        assert_eq!(
            Target::Gpu("RTX".into()).compute_device(),
            ComputeDevice::Gpu("DirectML (RTX)".into())
        );
        assert_eq!(Target::Cpu.compute_device(), ComputeDevice::Cpu);
    }

    #[test]
    fn missing_gpu_is_marked_unavailable() {
        let unavailable = |options: &[ChoiceOption], value: &str| {
            options
                .iter()
                .find(|o| o.value == value)
                .unwrap()
                .unavailable_key
                .clone()
        };
        let options = device_options(&devices(true, false), true);
        let values: Vec<_> = options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, ["auto", "gpu", "cpu"]);
        assert_eq!(
            unavailable(&options, "gpu").as_deref(),
            Some("engine.whisper_onnx.settings.device_gpu_missing")
        );
        assert_eq!(unavailable(&options, "auto"), None);
        assert_eq!(unavailable(&options, "cpu"), None);
        for option in &options {
            assert_eq!(
                option.label_key.as_deref(),
                Some(format!("engine.whisper_onnx.settings.device_{}", option.value).as_str())
            );
        }

        let options = device_options(&devices(false, true), true);
        let gpu = options.iter().find(|o| o.value == "gpu").unwrap();
        assert_eq!(gpu.unavailable_key, None);
        assert_eq!(gpu.detail.as_deref(), Some("RTX"));

        let options = device_options(&devices(false, true), false);
        assert_eq!(
            unavailable(&options, "gpu").as_deref(),
            Some("engine.whisper_onnx.settings.device_not_built")
        );
    }
}
