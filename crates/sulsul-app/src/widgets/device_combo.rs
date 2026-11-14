//! 입력 장치 선택 콤보 (녹음 화면과 설정 화면에서 함께 사용).

use egui::Ui;
use rust_i18n::t;

use super::controls::{self, ComboItem};
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
