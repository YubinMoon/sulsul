//! 화면들이 함께 쓰는 실행 중 상태 (설정 파일에 저장하지 않는 것들).

use std::sync::Arc;
use std::time::{Duration, Instant};

use sulsul_core::AppPaths;
use sulsul_core::audio::{self, AudioClip, AudioError, InputDevice};
use sulsul_core::config::Config;
use sulsul_core::stt::SttRegistry;

use crate::pages::Page;
use crate::widgets::banner::Tone;

/// 화면에 넘겨주는 묶음
pub struct PageContext<'a> {
    pub shared: &'a mut Shared,
    /// 화면에 보이는 설정. 값을 바꾸면 앱이 알아채고 파일에 저장합니다.
    pub config: &'a mut Config,
    /// 설정 파일을 읽지 못해 설정 편집이 잠겨 있는지
    pub locked: bool,
    /// 파일 대화상자의 부모 창
    pub frame: &'a eframe::Frame,
}

pub struct Shared {
    pub paths: AppPaths,
    pub registry: SttRegistry,
    pub devices: DeviceList,
    /// 방금 녹음한 오디오 (받아쓰기 테스트에서 사용)
    pub last_recording: Option<Arc<Recorded>>,
    pub toasts: Toasts,
    /// 다음 프레임에 이동할 화면
    pub navigate: Option<Page>,
}

impl Shared {
    pub fn new(paths: AppPaths, registry: SttRegistry) -> Self {
        Self {
            paths,
            registry,
            devices: DeviceList::load(),
            last_recording: None,
            toasts: Toasts::default(),
            navigate: None,
        }
    }
}

/// 녹음 결과
pub struct Recorded {
    pub clip: AudioClip,
    pub device_name: String,
    /// 최대 녹음 길이에 닿아 자동으로 끝났는지
    pub auto_stopped: bool,
}

/// 입력 장치 목록. 장치를 훑는 데 시간이 걸릴 수 있어 필요할 때만 다시 읽습니다.
pub struct DeviceList {
    pub devices: Vec<InputDevice>,
    pub error: Option<AudioError>,
}

impl DeviceList {
    pub fn load() -> Self {
        match audio::input_devices() {
            Ok(devices) => Self {
                devices,
                error: None,
            },
            Err(error) => {
                tracing::warn!("could not list input devices: {error}");
                Self {
                    devices: Vec::new(),
                    error: Some(error),
                }
            }
        }
    }

    pub fn refresh(&mut self) {
        *self = Self::load();
    }

    /// 설정 값(장치 ID 또는 이름)에 해당하는 장치. 빈 값이면 시스템 기본 장치입니다.
    pub fn find(&self, selector: &str) -> Option<&InputDevice> {
        let selector = selector.trim();
        if selector.is_empty() {
            return self.default_device();
        }
        self.devices
            .iter()
            .find(|d| !d.id.is_empty() && d.id == selector)
            .or_else(|| self.devices.iter().find(|d| d.name == selector))
    }

    pub fn default_device(&self) -> Option<&InputDevice> {
        self.devices.iter().find(|d| d.is_default)
    }
}

/// 설정 파일에 저장할 장치 값 (ID가 없으면 이름)
pub fn device_key(device: &InputDevice) -> String {
    if device.id.is_empty() {
        device.name.clone()
    } else {
        device.id.clone()
    }
}

/// 잠깐 떴다 사라지는 알림
#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

pub struct Toast {
    pub tone: Tone,
    pub text: String,
    expires: Instant,
}

impl Toasts {
    const LIFETIME: Duration = Duration::from_secs(4);

    pub fn info(&mut self, text: impl Into<String>) {
        self.push(Tone::Success, text);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push(Tone::Error, text);
    }

    fn push(&mut self, tone: Tone, text: impl Into<String>) {
        self.items.push(Toast {
            tone,
            text: text.into(),
            expires: Instant::now() + Self::LIFETIME,
        });
        // 화면을 덮지 않도록 최근 것 몇 개만 남깁니다.
        let overflow = self.items.len().saturating_sub(3);
        self.items.drain(..overflow);
    }

    /// 시간이 지난 알림을 지우고 남은 것을 돌려줍니다.
    /// 두 번째 값은 다음 알림이 사라질 때까지 남은 시간입니다.
    pub fn visible(&mut self) -> (&[Toast], Option<Duration>) {
        let now = Instant::now();
        self.items.retain(|toast| toast.expires > now);
        let next = self.items.iter().map(|toast| toast.expires - now).min();
        (&self.items, next)
    }
}
