//! 숫자·시간 표시 형식.

use std::time::Duration;

use rust_i18n::t;

/// 이보다 조용하면 소리가 들어오지 않은 것으로 봅니다 (일반적인 방 소음은 -60 ~ -45 dBFS).
pub const SILENCE_DBFS: f32 = -60.0;

/// 음량 미터가 표시하는 가장 작은 값
pub const METER_FLOOR_DBFS: f32 = -60.0;

/// 선형 음량(0.0 ~ 1.0)을 dBFS로 바꿉니다. 완전한 무음은 -120으로 봅니다.
pub fn dbfs(level: f32) -> f32 {
    20.0 * level.max(1e-6).log10()
}

/// dBFS를 미터의 0.0 ~ 1.0 위치로 바꿉니다.
pub fn meter_position(dbfs: f32) -> f32 {
    ((dbfs - METER_FLOOR_DBFS) / -METER_FLOOR_DBFS).clamp(0.0, 1.0)
}

pub fn dbfs_text(dbfs: f32) -> String {
    t!("unit.dbfs", value = format!("{dbfs:.1}")).into_owned()
}

pub fn hertz_text(rate: u32) -> String {
    t!("unit.hertz", value = group_thousands(rate)).into_owned()
}

pub fn percent_text(ratio: f32) -> String {
    t!("unit.percent", value = format!("{:.0}", ratio * 100.0)).into_owned()
}

/// 초 단위 (예: `3.42초`)
pub fn seconds_text(duration: Duration) -> String {
    t!(
        "unit.seconds",
        value = format!("{:.2}", duration.as_secs_f64())
    )
    .into_owned()
}

/// 짧은 처리 시간 (1초 미만은 ms, 그 이상은 초)
pub fn elapsed_text(duration: Duration) -> String {
    if duration < Duration::from_secs(1) {
        t!("unit.milliseconds", value = duration.as_millis()).into_owned()
    } else {
        seconds_text(duration)
    }
}

/// 시계 형식 (예: `1:05.3`)
pub fn clock_text(duration: Duration) -> String {
    let tenths = (duration.as_secs_f64() * 10.0).floor() as u64;
    format!("{}:{:02}.{}", tenths / 600, (tenths / 10) % 60, tenths % 10)
}

/// 시계 형식, 초까지만 (예: `2:00`)
pub fn clock_text_short(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn group_thousands(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dbfs_conversion() {
        assert_eq!(dbfs(1.0), 0.0);
        assert!((dbfs(0.1) + 20.0).abs() < 1e-4);
        assert_eq!(dbfs(0.0), -120.0);
    }

    #[test]
    fn meter_position_is_clamped() {
        assert_eq!(meter_position(-120.0), 0.0);
        assert_eq!(meter_position(-30.0), 0.5);
        assert_eq!(meter_position(3.0), 1.0);
    }

    #[test]
    fn clock_formats() {
        assert_eq!(clock_text(Duration::from_millis(65_340)), "1:05.3");
        assert_eq!(clock_text(Duration::ZERO), "0:00.0");
        assert_eq!(clock_text_short(Duration::from_secs(120)), "2:00");
    }

    #[test]
    fn thousands_grouping() {
        assert_eq!(group_thousands(48_000), "48,000");
        assert_eq!(group_thousands(800), "800");
        assert_eq!(group_thousands(1_234_567), "1,234,567");
    }
}
