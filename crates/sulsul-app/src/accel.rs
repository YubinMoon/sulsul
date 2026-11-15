//! 가속 장치(GPU·NPU) 상태: 엔진마다 무엇을 쓸 수 있고, 지금 설정으로 무엇을 쓸지.
//!
//! 엔진 크레이트가 알려 주는 값(빌드에 들어 있는 백엔드, 감지한 장치)과 설정 값을 조합합니다.
//!
//! 여기서 구하는 장치는 **예상**입니다. 실제로 쓴 장치는 모델을 불러온 뒤 엔진이 보고하는
//! 값([`ComputeDevice`])이고, 장치 메모리가 모자라는 등의 이유로 예상과 다를 수 있습니다.
//! 화면에서는 둘을 섞지 않습니다. 예상은 "예상 장치"로, 실제 값은 테스트·인식 결과에만 씁니다.

use std::sync::OnceLock;

use rust_i18n::t;
use sulsul_core::config::SttConfig;
use sulsul_core::stt::{ComputeDevice, EngineKind, SttRegistry};
use sulsul_stt_sensevoice::{DetectedDevices, DevicePreference};

use crate::i18n::tr;

/// 이 빌드와 컴퓨터에서 쓸 수 있는 가속 수단. 단위 테스트에서는 값을 직접 넣습니다.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccelEnv {
    /// Whisper 빌드에 들어 있는 GPU 백엔드 (`CUDA`, `Vulkan`). CPU 전용 빌드면 `None`입니다.
    pub whisper_backend: Option<&'static str>,
    /// Whisper가 실행 중에 쓸 수 있는 GPU 장치 이름
    pub whisper_gpu: Option<String>,
    /// 이 빌드가 DirectML(SenseVoice 가속)을 지원하는지
    pub directml_built: bool,
    /// DirectML이 쓸 수 있는 NPU·GPU
    pub directml: DetectedDevices,
}

impl AccelEnv {
    /// 실행 중인 컴퓨터의 상태. 처음 부를 때 한 번 알아보고 그 뒤로는 같은 값을 돌려줍니다.
    ///
    /// 엔진 크레이트도 감지 결과를 캐시하고, 앱을 시작할 때 엔진을 등록하면서 이미 감지를
    /// 마쳤으므로 화면을 그리는 중에 불러도 멈칫하지 않습니다.
    pub fn current() -> &'static Self {
        static ENV: OnceLock<AccelEnv> = OnceLock::new();
        ENV.get_or_init(|| Self {
            whisper_backend: sulsul_stt_whisper::compiled_gpu_backend(),
            whisper_gpu: sulsul_stt_whisper::gpu_device().map(str::to_owned),
            directml_built: sulsul_stt_sensevoice::accel::DIRECTML_BUILT,
            directml: sulsul_stt_sensevoice::detected_devices().clone(),
        })
    }
}

/// 가속 장치의 종류
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Cpu,
    Gpu,
    Npu,
}

impl DeviceKind {
    /// 엔진이 보고한 장치의 종류. 장치를 알 수 없으면(클라우드 등) `None`입니다.
    pub fn of(device: &ComputeDevice) -> Option<Self> {
        match device {
            ComputeDevice::Unknown => None,
            ComputeDevice::Cpu => Some(Self::Cpu),
            ComputeDevice::Gpu(_) => Some(Self::Gpu),
            ComputeDevice::Npu(_) => Some(Self::Npu),
        }
    }

    /// 화면에 보일 짧은 이름 (`CPU`, `GPU`, `NPU`)
    pub fn label(self) -> String {
        match self {
            Self::Cpu => t!("models.accel.kind.cpu"),
            Self::Gpu => t!("models.accel.kind.gpu"),
            Self::Npu => t!("models.accel.kind.npu"),
        }
        .into_owned()
    }
}

/// 엔진 설정이 요청한 가속
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccelRequest {
    Whisper { use_gpu: bool },
    SenseVoice(DevicePreference),
}

impl AccelRequest {
    /// 엔진의 지금 설정에서 읽습니다. 가속 설정이 없는 엔진(클라우드 등)이면 `None`입니다.
    pub fn from_settings(registry: &SttRegistry, stt: &SttConfig, engine_id: &str) -> Option<Self> {
        let resolved = registry.resolve_settings(stt, engine_id).ok()?;
        if engine_id == sulsul_stt_whisper::ENGINE_ID {
            Some(Self::Whisper {
                use_gpu: resolved.bool(sulsul_stt_whisper::keys::USE_GPU),
            })
        } else if engine_id == sulsul_stt_sensevoice::ENGINE_ID {
            let preference =
                DevicePreference::from_setting(resolved.str(sulsul_stt_sensevoice::keys::DEVICE))
                    .unwrap_or(DevicePreference::Auto);
            Some(Self::SenseVoice(preference))
        } else {
            None
        }
    }

    /// 설정 값을 화면에 보일 글로 바꿉니다 (예: `GPU 가속 켬`, `자동 (NPU → GPU → CPU)`).
    pub fn label(self) -> String {
        match self {
            Self::Whisper { use_gpu: true } => t!("models.accel.whisper.setting_on").into_owned(),
            Self::Whisper { use_gpu: false } => t!("models.accel.whisper.setting_off").into_owned(),
            Self::SenseVoice(preference) => tr(&format!(
                "engine.sense_voice_local.settings.device_{}",
                preference.as_str()
            )),
        }
    }
}

/// 예상 장치가 그렇게 정해진 이유
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// 설정한 대로 가속 장치를 씁니다.
    Configured,
    /// 자동 선택이 NPU를 골랐습니다.
    AutoNpu,
    /// 자동 선택이 NPU가 없어 GPU를 골랐습니다.
    AutoGpu,
    /// 자동 선택인데 쓸 수 있는 가속 장치가 없습니다.
    AutoNone,
    /// 사용자가 가속을 껐거나 CPU를 골랐습니다.
    TurnedOff,
    /// 이 빌드에 가속 백엔드가 없습니다 (Whisper CPU 전용 빌드, DirectML이 없는 빌드).
    NotBuilt,
    /// 쓸 수 있는 GPU를 찾지 못했습니다.
    NoGpu,
    /// NPU를 찾지 못했습니다.
    NoNpu,
}

impl Why {
    /// 쓰려던 가속을 환경 때문에 쓰지 못하는 경우인지 (화면에서 경고색으로 알립니다)
    pub fn is_blocked(self) -> bool {
        matches!(self, Self::NotBuilt | Self::NoGpu | Self::NoNpu)
    }
}

/// 지금 설정으로 엔진이 쓸 것으로 예상하는 장치
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub device: ComputeDevice,
    pub why: Why,
}

impl Plan {
    fn cpu(why: Why) -> Self {
        Self {
            device: ComputeDevice::Cpu,
            why,
        }
    }
}

/// 설정과 환경으로 예상 장치를 구합니다.
///
/// 엔진이 장치를 고르는 규칙을 그대로 따릅니다. Whisper는 `WhisperEngine::load`,
/// SenseVoice는 `plan_targets`(둘 다 엔진 크레이트의 `engine.rs`)입니다.
pub fn plan(request: AccelRequest, env: &AccelEnv) -> Plan {
    match request {
        AccelRequest::Whisper { use_gpu } => match (env.whisper_backend, &env.whisper_gpu) {
            (None, _) => Plan::cpu(Why::NotBuilt),
            (Some(_), None) => Plan::cpu(Why::NoGpu),
            (Some(_), Some(_)) if !use_gpu => Plan::cpu(Why::TurnedOff),
            (Some(_), Some(name)) => Plan {
                device: ComputeDevice::Gpu(name.clone()),
                why: Why::Configured,
            },
        },
        AccelRequest::SenseVoice(preference) => {
            // 엔진이 보고하는 이름과 같은 형식으로 맞춥니다.
            let npu = env
                .directml
                .npu()
                .map(|name| ComputeDevice::Npu(format!("DirectML ({name})")));
            let gpu = env
                .directml
                .gpu()
                .map(|name| ComputeDevice::Gpu(format!("DirectML ({name})")));
            let found = |device: Option<ComputeDevice>, why: Why, missing: Why| match device {
                Some(device) => Plan { device, why },
                None => Plan::cpu(missing),
            };
            match preference {
                DevicePreference::Cpu => Plan::cpu(Why::TurnedOff),
                _ if !env.directml_built => Plan::cpu(Why::NotBuilt),
                DevicePreference::Npu => found(npu, Why::Configured, Why::NoNpu),
                DevicePreference::Gpu => found(gpu, Why::Configured, Why::NoGpu),
                DevicePreference::Auto => match (npu, gpu) {
                    (Some(device), _) => Plan {
                        device,
                        why: Why::AutoNpu,
                    },
                    (None, Some(device)) => Plan {
                        device,
                        why: Why::AutoGpu,
                    },
                    (None, None) => Plan::cpu(Why::AutoNone),
                },
            }
        }
    }
}

/// 가속 장치 카드에 보여 줄 엔진 하나의 상태
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineAccel {
    /// 빌드에 들어 있는 가속 백엔드 이름 (`Vulkan`, `CUDA`, `DirectML`). 없으면 `None`입니다.
    pub backend: Option<&'static str>,
    /// 이 엔진이 쓸 수 있는 장치를 종류별로 찾은 결과. 찾지 못한 종류는 목록이 비어 있습니다.
    /// 가속 백엔드가 없는 빌드에서는 찾아보지 않으므로 통째로 비어 있습니다.
    pub devices: Vec<(DeviceKind, Vec<String>)>,
    pub request: AccelRequest,
    pub plan: Plan,
}

impl EngineAccel {
    /// 엔진의 가속 상태. 가속 설정이 없는 엔진이면 `None`입니다.
    pub fn of(
        registry: &SttRegistry,
        stt: &SttConfig,
        engine_id: &str,
        env: &AccelEnv,
    ) -> Option<Self> {
        let request = AccelRequest::from_settings(registry, stt, engine_id)?;
        let (backend, devices) = match request {
            AccelRequest::Whisper { .. } => (
                env.whisper_backend,
                if env.whisper_backend.is_some() {
                    vec![(DeviceKind::Gpu, env.whisper_gpu.iter().cloned().collect())]
                } else {
                    Vec::new()
                },
            ),
            AccelRequest::SenseVoice(_) if env.directml_built => (
                Some("DirectML"),
                vec![
                    (DeviceKind::Npu, env.directml.npus.clone()),
                    (DeviceKind::Gpu, env.directml.gpus.clone()),
                ],
            ),
            AccelRequest::SenseVoice(_) => (None, Vec::new()),
        };
        Some(Self {
            backend,
            devices,
            request,
            plan: plan(request, env),
        })
    }

    /// 예상 장치가 그렇게 정해진 이유를 설명하는 한 문장
    pub fn note(&self, registry: &SttRegistry) -> String {
        match (self.request, self.plan.why) {
            // Whisper가 GPU를 쓸 수 없는 이유는 엔진이 설정 항목을 잠그며 알려 준 문구를 그대로 씁니다.
            (AccelRequest::Whisper { .. }, Why::NotBuilt | Why::NoGpu) => {
                whisper_lock_reason(registry)
                    .unwrap_or_else(|| t!("models.accel.whisper.cpu_only").into_owned())
            }
            (AccelRequest::Whisper { .. }, Why::TurnedOff) => {
                t!("models.accel.whisper.turned_off").into_owned()
            }
            (AccelRequest::Whisper { .. }, _) => t!("models.accel.whisper.gpu").into_owned(),
            (AccelRequest::SenseVoice(_), why) => match why {
                Why::Configured => t!("models.accel.sense_voice.chosen"),
                Why::AutoNpu => t!("models.accel.sense_voice.auto_npu"),
                Why::AutoGpu => t!("models.accel.sense_voice.auto_gpu"),
                Why::AutoNone => t!("models.accel.sense_voice.auto_none"),
                Why::TurnedOff => t!("models.accel.sense_voice.cpu_chosen"),
                Why::NotBuilt => t!("models.accel.sense_voice.not_built"),
                Why::NoGpu => t!(
                    "models.accel.sense_voice.missing",
                    device = DeviceKind::Gpu.label()
                ),
                Why::NoNpu => t!(
                    "models.accel.sense_voice.missing",
                    device = DeviceKind::Npu.label()
                ),
            }
            .into_owned(),
        }
    }
}

/// Whisper의 GPU 가속 항목이 잠겨 있으면 엔진이 알려 준 이유
fn whisper_lock_reason(registry: &SttRegistry) -> Option<String> {
    registry
        .descriptor(sulsul_stt_whisper::ENGINE_ID)?
        .settings
        .iter()
        .find(|field| field.key == sulsul_stt_whisper::keys::USE_GPU)?
        .unavailable_key
        .as_deref()
        .map(tr)
}

/// 엔진이 계산에 쓴(또는 쓸) 장치를 화면에 보일 글로 바꿉니다.
///
/// 엔진이 장치를 알려 주지 않았으면(`Unknown`) "알 수 없음"입니다. 엔진이 보고한 실제 장치를
/// 보여 줄 때는 엔진 종류까지 보는 [`reported_device_label`]을 씁니다.
pub fn device_label(device: &ComputeDevice) -> String {
    match device {
        ComputeDevice::Cpu => t!("models.device.cpu").into_owned(),
        ComputeDevice::Gpu(name) => t!("models.device.gpu", name = name).into_owned(),
        ComputeDevice::Npu(name) => t!("models.device.npu", name = name).into_owned(),
        ComputeDevice::Unknown => t!("models.device.unknown").into_owned(),
    }
}

/// `kind` 종류의 엔진이 보고한 실제 장치를 화면에 보일 글로 바꿉니다.
///
/// 클라우드 엔진은 계산을 외부 서비스가 하므로 장치를 알려 주지 않습니다(`Unknown`). 그때만
/// "클라우드 API"로 보여 주고, 로컬 엔진이 장치를 알려 주지 않았으면 "알 수 없음"으로 보여 줍니다.
pub fn reported_device_label(device: &ComputeDevice, kind: EngineKind) -> String {
    match (device, kind) {
        (ComputeDevice::Unknown, EngineKind::Cloud) => t!("models.device.remote").into_owned(),
        _ => device_label(device),
    }
}

/// 가속을 요청했는데 그 장치로 실행되지 않은 이유
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// 이 빌드에 가속 백엔드가 없습니다.
    NotBuilt,
    /// 쓸 수 있는 장치를 찾지 못했습니다.
    NoDevice,
    /// 장치는 있는데 모델을 올리지 못했습니다 (장치 메모리 부족, 초기화 실패 등).
    LoadFailed,
}

/// 요청한 가속과 실제로 쓴 장치를 견주어, 알릴 것이 있으면 그 이유를 돌려줍니다.
///
/// - 가속을 끄거나 CPU를 골랐으면 알리지 않습니다.
/// - 예상한 가속 장치로 실행했으면 알리지 않습니다.
/// - 자동 선택은 가속 장치가 있는데도 쓰지 못했을 때만 알립니다.
pub fn fallback(request: AccelRequest, actual: &ComputeDevice, env: &AccelEnv) -> Option<Fallback> {
    let actual = DeviceKind::of(actual)?;
    let planned = plan(request, env);
    let planned_kind = DeviceKind::of(&planned.device).unwrap_or(DeviceKind::Cpu);
    if planned_kind != DeviceKind::Cpu {
        // 가속 장치를 쓸 예정이었는데 다른 장치로 실행했습니다.
        return (actual != planned_kind).then_some(Fallback::LoadFailed);
    }
    // 처음부터 CPU로 예상한 경우에는, 가속을 직접 요청했을 때만 이유를 알립니다.
    let explicit = match request {
        AccelRequest::Whisper { use_gpu } => use_gpu,
        AccelRequest::SenseVoice(preference) => {
            matches!(preference, DevicePreference::Npu | DevicePreference::Gpu)
        }
    };
    if !explicit || actual != DeviceKind::Cpu {
        return None;
    }
    match planned.why {
        Why::NotBuilt => Some(Fallback::NotBuilt),
        Why::NoGpu | Why::NoNpu => Some(Fallback::NoDevice),
        _ => None,
    }
}

/// [`fallback`]의 결과를 사용자에게 보일 문장으로 바꿉니다. `actual`은 실제로 쓴 장치입니다.
pub fn fallback_text(
    request: AccelRequest,
    fallback: Fallback,
    actual: &ComputeDevice,
    env: &AccelEnv,
) -> String {
    match request {
        AccelRequest::Whisper { .. } => match fallback {
            Fallback::NotBuilt => t!("models.test.gpu_not_built"),
            Fallback::NoDevice => t!("models.test.gpu_no_device"),
            Fallback::LoadFailed => t!(
                "models.test.gpu_fallback",
                device = env.whisper_gpu.as_deref().unwrap_or_default()
            ),
        }
        .into_owned(),
        AccelRequest::SenseVoice(preference) => {
            // 고른 장치. 자동 선택이면 먼저 시도했을 장치입니다.
            let planned = plan(request, env);
            let kind = match preference {
                DevicePreference::Npu => DeviceKind::Npu,
                DevicePreference::Gpu => DeviceKind::Gpu,
                DevicePreference::Auto | DevicePreference::Cpu => {
                    DeviceKind::of(&planned.device).unwrap_or(DeviceKind::Cpu)
                }
            };
            match fallback {
                Fallback::NotBuilt => t!("models.test.accel_not_built", device = kind.label()),
                Fallback::NoDevice => t!("models.test.accel_missing", device = kind.label()),
                Fallback::LoadFailed => {
                    let name = match kind {
                        DeviceKind::Npu => env.directml.npu(),
                        DeviceKind::Gpu => env.directml.gpu(),
                        DeviceKind::Cpu => None,
                    };
                    t!(
                        "models.test.accel_fallback",
                        device = kind.label(),
                        name = name.unwrap_or_default(),
                        actual = device_label(actual)
                    )
                }
            }
            .into_owned()
        }
    }
}

/// 가속을 요청했는데 그 장치로 실행하지 못했으면, 사용자에게 알릴 문장을 돌려줍니다.
///
/// `actual`은 엔진이 보고한 실제 장치입니다. 알릴 것이 없으면 `None`입니다([`fallback`] 참고).
pub fn fallback_note(
    request: AccelRequest,
    actual: &ComputeDevice,
    env: &AccelEnv,
) -> Option<String> {
    fallback(request, actual, env).map(|reason| fallback_text(request, reason, actual, env))
}

/// 테스트를 기다리는 동안 보여 줄 안내. 가속 장치를 쓸 예정일 때만 있습니다.
///
/// 가속 장치는 그 컴퓨터에서 처음 쓸 때 준비(셰이더 컴파일 등)가 오래 걸립니다.
pub fn first_run_hint(request: AccelRequest, env: &AccelEnv) -> Option<String> {
    let planned = plan(request, env);
    if planned.device == ComputeDevice::Cpu {
        return None;
    }
    let vulkan = matches!(request, AccelRequest::Whisper { .. })
        && env
            .whisper_backend
            .is_some_and(|backend| backend.eq_ignore_ascii_case("vulkan"));
    Some(
        if vulkan {
            t!("models.test.first_run_vulkan")
        } else {
            t!("models.test.first_run_accel")
        }
        .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const RTX: &str = "NVIDIA GeForce RTX 3060";
    const AI_BOOST: &str = "Intel(R) AI Boost";

    /// Whisper 환경: `backend`로 빌드했고 `gpu`를 찾았습니다.
    fn whisper_env(backend: Option<&'static str>, gpu: Option<&str>) -> AccelEnv {
        AccelEnv {
            whisper_backend: backend,
            whisper_gpu: gpu.map(str::to_owned),
            ..AccelEnv::default()
        }
    }

    /// SenseVoice 환경: DirectML 빌드 여부와 찾은 장치
    fn directml_env(built: bool, npu: bool, gpu: bool) -> AccelEnv {
        AccelEnv {
            directml_built: built,
            directml: DetectedDevices {
                npus: npu.then(|| AI_BOOST.to_owned()).into_iter().collect(),
                gpus: gpu.then(|| RTX.to_owned()).into_iter().collect(),
            },
            ..AccelEnv::default()
        }
    }

    fn whisper(use_gpu: bool) -> AccelRequest {
        AccelRequest::Whisper { use_gpu }
    }

    fn sense_voice(preference: DevicePreference) -> AccelRequest {
        AccelRequest::SenseVoice(preference)
    }

    fn gpu(name: &str) -> ComputeDevice {
        ComputeDevice::Gpu(name.to_owned())
    }

    fn npu(name: &str) -> ComputeDevice {
        ComputeDevice::Npu(name.to_owned())
    }

    #[test]
    fn whisper_plan_covers_every_state() {
        let vulkan0 = "Vulkan0 (NVIDIA GeForce RTX 3060)";
        // 상태 1: GPU를 쓸 수 있음
        let usable = whisper_env(Some("Vulkan"), Some(vulkan0));
        assert_eq!(
            plan(whisper(true), &usable),
            Plan {
                device: gpu(vulkan0),
                why: Why::Configured
            }
        );
        // 상태 4: 사용자가 끔
        assert_eq!(plan(whisper(false), &usable), Plan::cpu(Why::TurnedOff));
        // 상태 2: GPU 빌드인데 장치가 없음 (켜든 끄든 이유는 같습니다)
        let no_device = whisper_env(Some("Vulkan"), None);
        assert_eq!(plan(whisper(true), &no_device), Plan::cpu(Why::NoGpu));
        assert_eq!(plan(whisper(false), &no_device), Plan::cpu(Why::NoGpu));
        // 상태 3: CPU 전용 빌드
        let cpu_build = whisper_env(None, None);
        assert_eq!(plan(whisper(true), &cpu_build), Plan::cpu(Why::NotBuilt));
    }

    #[test]
    fn sense_voice_plan_follows_engine_order() {
        let both = directml_env(true, true, true);
        let gpu_only = directml_env(true, false, true);
        let none = directml_env(true, false, false);
        let dml_npu = npu(&format!("DirectML ({AI_BOOST})"));
        let dml_gpu = gpu(&format!("DirectML ({RTX})"));
        let planned = |preference, env: &AccelEnv| plan(sense_voice(preference), env);

        // 자동: NPU → GPU → CPU
        let auto = DevicePreference::Auto;
        assert_eq!(planned(auto, &both).device, dml_npu);
        assert_eq!(planned(auto, &both).why, Why::AutoNpu);
        assert_eq!(planned(auto, &gpu_only).device, dml_gpu);
        assert_eq!(planned(auto, &gpu_only).why, Why::AutoGpu);
        assert_eq!(planned(auto, &none), Plan::cpu(Why::AutoNone));

        // 직접 고른 장치
        assert_eq!(planned(DevicePreference::Gpu, &both).device, dml_gpu);
        assert_eq!(planned(DevicePreference::Npu, &both).device, dml_npu);
        assert_eq!(
            planned(DevicePreference::Npu, &gpu_only),
            Plan::cpu(Why::NoNpu)
        );
        assert_eq!(planned(DevicePreference::Gpu, &none), Plan::cpu(Why::NoGpu));
        assert_eq!(
            planned(DevicePreference::Cpu, &both),
            Plan::cpu(Why::TurnedOff)
        );

        // DirectML이 없는 빌드에서는 장치가 있어도 CPU입니다.
        let not_built = directml_env(false, true, true);
        assert_eq!(planned(auto, &not_built), Plan::cpu(Why::NotBuilt));
        assert_eq!(
            planned(DevicePreference::Gpu, &not_built),
            Plan::cpu(Why::NotBuilt)
        );
        assert_eq!(
            planned(DevicePreference::Cpu, &not_built),
            Plan::cpu(Why::TurnedOff)
        );
    }

    #[test]
    fn whisper_fallback_explains_each_state() {
        let vulkan0 = "Vulkan0 (NVIDIA GeForce RTX 3060)";
        let usable = whisper_env(Some("Vulkan"), Some(vulkan0));
        let no_device = whisper_env(Some("Vulkan"), None);
        let cpu_build = whisper_env(None, None);
        let cpu = ComputeDevice::Cpu;

        // 상태 1: GPU로 실행했으면 알릴 것이 없습니다.
        assert_eq!(fallback(whisper(true), &gpu(vulkan0), &usable), None);
        // 상태 4: 사용자가 껐으면 어떤 환경에서도 알리지 않습니다.
        for env in [&usable, &no_device, &cpu_build] {
            assert_eq!(fallback(whisper(false), &cpu, env), None);
        }
        // 상태 2·3·5는 이유가 서로 다릅니다.
        assert_eq!(
            fallback(whisper(true), &cpu, &no_device),
            Some(Fallback::NoDevice)
        );
        assert_eq!(
            fallback(whisper(true), &cpu, &cpu_build),
            Some(Fallback::NotBuilt)
        );
        assert_eq!(
            fallback(whisper(true), &cpu, &usable),
            Some(Fallback::LoadFailed)
        );
        // 장치를 알 수 없으면(클라우드 등) 판단하지 않습니다.
        assert_eq!(
            fallback(whisper(true), &ComputeDevice::Unknown, &usable),
            None
        );
    }

    #[test]
    fn sense_voice_fallback_explains_explicit_choices() {
        let both = directml_env(true, true, true);
        let gpu_only = directml_env(true, false, true);
        let none = directml_env(true, false, false);
        let not_built = directml_env(false, false, false);
        let cpu = ComputeDevice::Cpu;
        let dml_gpu = gpu(&format!("DirectML ({RTX})"));
        let dml_npu = npu(&format!("DirectML ({AI_BOOST})"));
        let note = |preference, actual: &ComputeDevice, env: &AccelEnv| {
            fallback(sense_voice(preference), actual, env)
        };

        // 자동으로 GPU나 NPU를 썼으면 알리지 않습니다.
        let auto = DevicePreference::Auto;
        assert_eq!(note(auto, &dml_gpu, &gpu_only), None);
        assert_eq!(note(auto, &dml_npu, &both), None);
        // 자동인데 가속 장치가 없어 CPU로 실행한 것도 알릴 일이 아닙니다.
        assert_eq!(note(auto, &cpu, &none), None);
        assert_eq!(note(auto, &cpu, &not_built), None);
        // 자동인데 있는 장치를 쓰지 못했으면 알립니다.
        assert_eq!(note(auto, &cpu, &gpu_only), Some(Fallback::LoadFailed));
        assert_eq!(note(auto, &dml_gpu, &both), Some(Fallback::LoadFailed));

        // 직접 고른 장치
        let pick_gpu = DevicePreference::Gpu;
        let pick_npu = DevicePreference::Npu;
        assert_eq!(note(pick_gpu, &dml_gpu, &gpu_only), None);
        assert_eq!(note(pick_gpu, &cpu, &gpu_only), Some(Fallback::LoadFailed));
        assert_eq!(note(pick_gpu, &cpu, &none), Some(Fallback::NoDevice));
        assert_eq!(note(pick_npu, &cpu, &gpu_only), Some(Fallback::NoDevice));
        assert_eq!(note(pick_npu, &cpu, &not_built), Some(Fallback::NotBuilt));
        assert_eq!(note(pick_npu, &cpu, &both), Some(Fallback::LoadFailed));

        // CPU를 골랐으면 알리지 않습니다.
        assert_eq!(note(DevicePreference::Cpu, &cpu, &both), None);
    }

    /// 테스트 결과 배너에 붙는 안내 문장. 예전에는 `pages/models.rs`의 `gpu_fallback_note`가
    /// Whisper만 보고 "GPU 빌드인데 장치 없음"을 초기화 실패로 설명했습니다.
    #[test]
    fn gpu_note_explains_why_gpu_was_not_used() {
        rust_i18n::set_locale("ko");
        let cpu = ComputeDevice::Cpu;
        let vulkan0 = "Vulkan0 (NVIDIA GeForce RTX 3060)";
        let usable = whisper_env(Some("Vulkan"), Some(vulkan0));
        let no_device = whisper_env(Some("Vulkan"), None);
        let cpu_build = whisper_env(None, None);

        // Whisper 상태 1·4: GPU를 썼거나 사용자가 껐으면 알릴 것이 없습니다.
        assert_eq!(fallback_note(whisper(true), &gpu(vulkan0), &usable), None);
        assert_eq!(fallback_note(whisper(false), &cpu, &usable), None);
        assert_eq!(fallback_note(whisper(false), &cpu, &cpu_build), None);
        assert_eq!(
            fallback_note(whisper(true), &ComputeDevice::Unknown, &usable),
            None
        );

        // 상태 3: CPU 전용 빌드
        let text = fallback_note(whisper(true), &cpu, &cpu_build).unwrap();
        assert!(text.contains("CPU 전용"), "{text}");
        // 상태 2: GPU 빌드인데 장치가 없음. 초기화 실패로 설명하지 않습니다.
        let text = fallback_note(whisper(true), &cpu, &no_device).unwrap();
        assert!(text.contains("GPU를 찾지 못해"), "{text}");
        assert!(!text.contains("초기화"), "{text}");
        // 상태 5: 장치는 있는데 CPU로 실행됨. 메모리 부족과 초기화 실패 가능성을 함께 알립니다.
        let text = fallback_note(whisper(true), &cpu, &usable).unwrap();
        assert!(text.contains(vulkan0), "{text}");
        assert!(text.contains("VRAM"), "{text}");
        assert!(text.contains("초기화"), "{text}");

        // SenseVoice: 직접 고른 장치를 쓰지 못했을 때
        let gpu_only = directml_env(true, false, true);
        let pick_npu = sense_voice(DevicePreference::Npu);
        let text = fallback_note(pick_npu, &cpu, &gpu_only).unwrap();
        assert!(text.contains("NPU를 찾지 못해"), "{text}");
        let text = fallback_note(pick_npu, &cpu, &directml_env(false, false, false)).unwrap();
        assert!(text.contains("DirectML"), "{text}");
        let pick_gpu = sense_voice(DevicePreference::Gpu);
        let text = fallback_note(pick_gpu, &cpu, &gpu_only).unwrap();
        assert!(text.contains(RTX), "{text}");
        assert!(text.contains("CPU"), "{text}");

        // SenseVoice: 자동으로 GPU를 썼거나 CPU를 골랐으면 알리지 않습니다.
        let dml_gpu = gpu(&format!("DirectML ({RTX})"));
        let auto = sense_voice(DevicePreference::Auto);
        assert_eq!(fallback_note(auto, &dml_gpu, &gpu_only), None);
        assert_eq!(fallback_note(pick_gpu, &dml_gpu, &gpu_only), None);
        assert_eq!(
            fallback_note(sense_voice(DevicePreference::Cpu), &cpu, &gpu_only),
            None
        );
    }

    #[test]
    fn first_run_hint_only_when_accelerated() {
        rust_i18n::set_locale("ko");
        let vulkan = whisper_env(Some("Vulkan"), Some("Vulkan0 (RTX)"));
        let hint = first_run_hint(whisper(true), &vulkan).unwrap();
        assert!(hint.contains("Vulkan"), "{hint}");
        assert_eq!(first_run_hint(whisper(false), &vulkan), None);
        assert_eq!(
            first_run_hint(whisper(true), &whisper_env(Some("Vulkan"), None)),
            None
        );

        // Vulkan이 아닌 가속은 일반 안내를 씁니다.
        let cuda = whisper_env(Some("CUDA"), Some("CUDA0 (RTX)"));
        let hint = first_run_hint(whisper(true), &cuda).unwrap();
        assert!(!hint.contains("Vulkan"), "{hint}");
        let gpu_only = directml_env(true, false, true);
        assert!(first_run_hint(sense_voice(DevicePreference::Auto), &gpu_only).is_some());
        assert_eq!(
            first_run_hint(sense_voice(DevicePreference::Cpu), &gpu_only),
            None
        );
    }

    #[test]
    fn device_label_includes_npu() {
        rust_i18n::set_locale("ko");
        assert_eq!(device_label(&ComputeDevice::Cpu), "CPU");
        let label = device_label(&npu("DirectML (Intel(R) AI Boost)"));
        assert!(label.starts_with("NPU"), "{label}");
        assert!(label.contains("Intel(R) AI Boost"), "{label}");
        let label = device_label(&gpu("Vulkan0 (RTX)"));
        assert!(label.starts_with("GPU"), "{label}");
    }

    #[test]
    fn unknown_device_is_called_cloud_only_for_cloud_engines() {
        rust_i18n::set_locale("ko");
        let unknown = ComputeDevice::Unknown;
        assert_eq!(device_label(&unknown), "알 수 없음");
        assert_eq!(
            reported_device_label(&unknown, EngineKind::Cloud),
            "클라우드 API"
        );
        assert_eq!(
            reported_device_label(&unknown, EngineKind::Local),
            "알 수 없음",
            "로컬 엔진이 장치를 알려 주지 않았다고 클라우드로 보이면 안 됩니다"
        );
        // 장치를 알려 준 경우에는 엔진 종류와 상관없이 그 장치를 보여 줍니다.
        assert_eq!(
            reported_device_label(&ComputeDevice::Cpu, EngineKind::Local),
            "CPU"
        );
        assert_eq!(
            reported_device_label(&ComputeDevice::Cpu, EngineKind::Cloud),
            "CPU"
        );
    }

    #[test]
    fn engine_accel_reads_settings_from_registry() {
        let registry = crate::engines::registry();
        let mut stt = SttConfig::default();
        let env = directml_env(true, false, true);

        // 클라우드처럼 가속 설정이 없는 엔진은 카드에 나오지 않습니다.
        assert_eq!(
            EngineAccel::of(&registry, &stt, sulsul_stt_cloud::ENGINE_ID, &env),
            None
        );

        let sense_voice_id = sulsul_stt_sensevoice::ENGINE_ID;
        let report = EngineAccel::of(&registry, &stt, sense_voice_id, &env).unwrap();
        assert_eq!(report.backend, Some("DirectML"));
        assert_eq!(
            report.request,
            AccelRequest::SenseVoice(DevicePreference::Auto)
        );
        assert_eq!(report.plan.why, Why::AutoGpu);
        assert_eq!(
            report.devices,
            vec![
                (DeviceKind::Npu, Vec::new()),
                (DeviceKind::Gpu, vec![RTX.to_owned()])
            ]
        );

        // 저장된 값이 이 PC에 없는 장치여도 그대로 읽고, CPU로 실행할 것으로 예상합니다.
        stt.engines
            .entry(sense_voice_id.to_owned())
            .or_default()
            .insert(sulsul_stt_sensevoice::keys::DEVICE.into(), "npu".into());
        let report = EngineAccel::of(&registry, &stt, sense_voice_id, &env).unwrap();
        assert_eq!(
            report.request,
            AccelRequest::SenseVoice(DevicePreference::Npu)
        );
        assert_eq!(report.plan, Plan::cpu(Why::NoNpu));
        assert!(report.plan.why.is_blocked());
    }
}
