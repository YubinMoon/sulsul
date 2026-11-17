//! 입력 장치 선택 콤보와 장치 상태 문구 (설정 화면에서 고르고, 테스트 화면에서 알려 줌).

use egui::Ui;
use rust_i18n::t;

use super::controls::{self, ComboItem};
use crate::error_text;
use crate::format::hertz_text;
use crate::session::{DeviceList, device_key};

/// 장치를 고르면 `selected`에 장치 ID(시스템 기본은 빈 문자열)를 넣고 `true`를 돌려줍니다.
pub fn device_combo(
    ui: &mut Ui,
    id_salt: &str,
    width: f32,
    selected: &mut String,
    devices: &DeviceList,
) -> bool {
    let mut system_default = ComboItem::new(String::new(), t!("device.system_default"));
    if let Some(device) = devices.default_device() {
        system_default = system_default.detail(&device.name);
    }
    let mut items = vec![system_default];
    for device in &devices.devices {
        let mut item = ComboItem::new(device_key(device), &device.name);
        if device.is_default {
            item = item.detail(t!("device.default_tag"));
        }
        items.push(item);
    }

    // 설정에는 ID 대신 이름이 들어 있을 수도 있으므로 목록의 값으로 맞춰서 비교합니다.
    let known = !selected.trim().is_empty() && devices.find(selected).is_some();
    let mut current = if known {
        devices.find(selected).map(device_key).unwrap_or_default()
    } else {
        selected.trim().to_owned()
    };
    let missing_label = t!("device.missing", name = current.as_str()).into_owned();

    if controls::combo(ui, id_salt, width, &mut current, &items, &missing_label) {
        *selected = current;
        true
    } else {
        false
    }
}

/// 고른 장치(`selector`, 빈 값이면 시스템 기본)의 형식. 시스템 기본이면 지금 어떤 장치인지도 붙입니다.
/// 장치를 쓸 수 없으면 `None`입니다 (이유는 [`device_problem`]).
pub fn device_details(devices: &DeviceList, selector: &str) -> Option<String> {
    let device = devices.find(selector)?;
    let format = match (device.sample_rate, device.channels) {
        (Some(rate), Some(channels)) => t!(
            "device.format",
            rate = hertz_text(rate),
            channels = channels
        ),
        _ => t!("device.format_unknown"),
    };
    let details = if selector.trim().is_empty() {
        t!("device.using_default", name = device.name, format = format)
    } else {
        format
    };
    Some(details.into_owned())
}

/// 고른 장치(`selector`)를 지금 쓸 수 없으면 그 이유. 쓸 수 있으면 `None`입니다.
pub fn device_problem(devices: &DeviceList, selector: &str) -> Option<String> {
    if let Some(error) = &devices.error {
        return Some(error_text::audio(error));
    }
    if devices.find(selector).is_some() {
        return None;
    }
    let problem = if devices.devices.is_empty() {
        t!("device.none")
    } else {
        t!("device.not_connected")
    };
    Some(problem.into_owned())
}
