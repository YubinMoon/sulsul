//! 벽시계 시각: 유닉스 시각과 달력 날짜·시각.
//!
//! 받아쓰기 기록의 시각을 화면에 보일 때 씁니다. Windows는 시스템의 시간대 설정(일광 절약 시간 포함)으로
//! 이 컴퓨터의 지역 시각을 구하고, 다른 OS는 아직 UTC로 보여 줍니다.

use std::time::{SystemTime, UNIX_EPOCH};

const MILLIS_PER_DAY: i64 = 86_400_000;

/// 유닉스 시각 (밀리초). 1970년 이전이면 음수입니다.
pub fn unix_millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
    }
}

/// 달력 날짜와 시각 (분까지)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl DateTime {
    /// 이 컴퓨터의 지역 시각. 시간대를 알 수 없으면 UTC입니다.
    pub fn local(millis: i64) -> Self {
        #[cfg(windows)]
        if let Some(local) = windows_local(millis) {
            return local;
        }
        Self::utc(millis)
    }

    /// 지금 지역 시각
    pub fn now_local() -> Self {
        Self::local(unix_millis(SystemTime::now()))
    }

    /// 하루 전 같은 시각의 지역 시각 ("어제"를 가리는 데 씁니다)
    pub fn yesterday_local() -> Self {
        Self::local(unix_millis(SystemTime::now()).saturating_sub(MILLIS_PER_DAY))
    }

    /// UTC 시각
    pub fn utc(millis: i64) -> Self {
        let days = millis.div_euclid(MILLIS_PER_DAY);
        let minutes = millis.rem_euclid(MILLIS_PER_DAY) / 60_000;
        let (year, month, day) = civil_from_days(days);
        Self {
            year: i32::try_from(year).unwrap_or(if year < 0 { i32::MIN } else { i32::MAX }),
            month,
            day,
            hour: (minutes / 60) as u32,
            minute: (minutes % 60) as u32,
        }
    }

    pub fn same_day(&self, other: &Self) -> bool {
        (self.year, self.month, self.day) == (other.year, other.month, other.day)
    }
}

/// 1970-01-01부터 지난 날 수를 (연, 월, 일)로 바꿉니다 (그레고리력).
/// Howard Hinnant의 `civil_from_days` 계산법입니다.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Windows의 시간대 설정으로 지역 시각을 구합니다. 바꾸지 못하면 `None`.
#[cfg(windows)]
fn windows_local(millis: i64) -> Option<DateTime> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

    /// 1601-01-01부터 1970-01-01까지의 시간 (100나노초 단위)
    const EPOCH_OFFSET: i64 = 116_444_736_000_000_000;
    let ticks = millis.checked_mul(10_000)?.checked_add(EPOCH_OFFSET)?;
    let ticks = u64::try_from(ticks).ok()?;
    let file_time = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    // SAFETY: 입력은 위에서 만든 값이고, 출력은 이 함수의 지역 변수입니다. 시간대 정보가 널이면
    // 시스템의 지금 시간대 설정을 씁니다.
    unsafe {
        if FileTimeToSystemTime(&file_time, &mut utc) == 0 {
            return None;
        }
        if SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0 {
            return None;
        }
    }
    Some(DateTime {
        year: i32::from(local.wYear),
        month: u32::from(local.wMonth),
        day: u32::from(local.wDay),
        hour: u32::from(local.wHour),
        minute: u32::from(local.wMinute),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn date(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime {
        DateTime {
            year,
            month,
            day,
            hour,
            minute,
        }
    }

    #[test]
    fn utc_calendar_dates() {
        assert_eq!(DateTime::utc(0), date(1970, 1, 1, 0, 0));
        assert_eq!(DateTime::utc(1_700_000_000_000), date(2023, 11, 14, 22, 13));
        // 윤년의 2월 29일과 해가 바뀌기 직전
        assert_eq!(DateTime::utc(951_782_400_000), date(2000, 2, 29, 0, 0));
        assert_eq!(DateTime::utc(1_735_689_599_000), date(2024, 12, 31, 23, 59));
        // 1970년 이전
        assert_eq!(DateTime::utc(-60_000), date(1969, 12, 31, 23, 59));
    }

    #[test]
    fn unix_millis_counts_from_the_epoch() {
        assert_eq!(unix_millis(UNIX_EPOCH), 0);
        assert_eq!(
            unix_millis(UNIX_EPOCH + Duration::from_millis(1_234)),
            1_234
        );
        assert_eq!(unix_millis(UNIX_EPOCH - Duration::from_secs(1)), -1_000);
    }

    #[test]
    fn local_time_is_within_a_day_of_utc() {
        // 시간대는 컴퓨터마다 다르니, 지역 날짜가 UTC 날짜와 하루 이상 벌어지지 않는지만 봅니다.
        let millis = 1_700_000_000_000;
        let local = DateTime::local(millis);
        let near = [
            DateTime::utc(millis - MILLIS_PER_DAY),
            DateTime::utc(millis),
            DateTime::utc(millis + MILLIS_PER_DAY),
        ];
        assert!(
            near.iter().any(|day| day.same_day(&local)),
            "지역 날짜가 UTC 날짜와 너무 다릅니다: {local:?}"
        );
        assert!(local.hour < 24 && local.minute < 60);
    }
}
