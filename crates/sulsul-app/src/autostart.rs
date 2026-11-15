//! 로그인할 때 자동 실행.
//!
//! 설정 파일의 `general.launch_at_startup`이 기준입니다. 앱이 시작할 때와 그 값이 바뀔 때(화면이든 파일이든)
//! OS 등록을 그 값에 맞춥니다. 자동 실행은 창 없이 트레이로 시작합니다([`BACKGROUND_ARG`]).
//!
//! Windows는 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`에 값을 둡니다. 개발 빌드는 이름이 다른
//! 값([`ENTRY_NAME`])을 써서 설치본의 등록을 건드리지 않고, 설정·데이터 폴더를 따로 정한 실행(`--home`,
//! `SULSUL_HOME`)도 폴더마다 이름이 달라 서로의 등록을 지우지 않습니다. 다른 OS는 아직 지원하지 않습니다.
//! 등록을 읽고 쓰는 부분은 [`LoginItems`]로 떼어 두어, 시험에서는 실제 레지스트리를 건드리지 않습니다.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 창 없이 트레이로 시작하라는 명령줄 인자
pub const BACKGROUND_ARG: &str = "--background";
/// 설정·데이터 폴더를 정하는 명령줄 인자 (`SULSUL_HOME`과 같음). 자동 실행이 같은 폴더로 시작하게 씁니다.
pub const HOME_ARG: &str = "--home";

/// 자동 실행 항목 이름. 폴더를 따로 정했으면 뒤에 폴더 해시를 붙입니다.
#[cfg(not(debug_assertions))]
pub const ENTRY_NAME: &str = "sulsul";
#[cfg(debug_assertions)]
pub const ENTRY_NAME: &str = "sulsul-dev";

/// 로그인 항목 저장소
pub trait LoginItems {
    /// 등록된 명령. 없으면 `None`.
    fn get(&self, name: &str) -> Result<Option<String>, AutostartError>;
    fn set(&mut self, name: &str, command: &str) -> Result<(), AutostartError>;
    fn remove(&mut self, name: &str) -> Result<(), AutostartError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AutostartError {
    #[error("launching at login is not supported on this platform")]
    /// 지원하지 않는 OS에서만 만듭니다.
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
    #[error("could not update the login item: {0}")]
    Os(#[from] std::io::Error),
}

/// 등록할 항목
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub command: String,
}

impl Entry {
    /// 이 실행 파일을 트레이로 시작하는 항목. `home`은 명령줄이나 `SULSUL_HOME`으로 정한 폴더입니다.
    pub fn new(exe: &Path, home: Option<&Path>) -> Self {
        let mut command = format!("{} {BACKGROUND_ARG}", quote(exe));
        let mut name = ENTRY_NAME.to_owned();
        if let Some(home) = home {
            command.push_str(&format!(" {HOME_ARG} {}", quote(home)));
            // 이름에 다 넣기엔 길어서 앞 32비트만 씁니다.
            name.push_str(&format!(
                "-{:08x}",
                crate::instance::folder_hash(home) >> 32
            ));
        }
        Self { name, command }
    }

    /// 지금 실행 중인 파일로 만든 항목
    pub fn current(home: Option<&Path>) -> std::io::Result<Self> {
        Ok(Self::new(&std::env::current_exe()?, home))
    }
}

fn quote(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// 등록을 `enabled`에 맞춥니다. 바꾼 것이 있으면 `true`.
pub fn apply(
    items: &mut impl LoginItems,
    entry: &Entry,
    enabled: bool,
) -> Result<bool, AutostartError> {
    let current = items.get(&entry.name)?;
    if enabled {
        // 실행 파일을 옮겼으면 새 경로로 고칩니다.
        if current.as_deref() == Some(entry.command.as_str()) {
            return Ok(false);
        }
        items.set(&entry.name, &entry.command)?;
        tracing::info!(command = %entry.command, "registered launch at login");
    } else {
        if current.is_none() {
            return Ok(false);
        }
        items.remove(&entry.name)?;
        tracing::info!("removed launch at login");
    }
    Ok(true)
}

/// 명령줄 인자
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchArgs {
    /// 창 없이 트레이로 시작
    pub background: bool,
    /// `--home <폴더>`
    pub home: Option<PathBuf>,
}

impl LaunchArgs {
    /// 실행 파일 이름을 뺀 인자를 읽습니다. 모르는 인자는 무시합니다.
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Self {
        let mut parsed = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == BACKGROUND_ARG {
                parsed.background = true;
            } else if arg == HOME_ARG {
                parsed.home = args
                    .next()
                    .filter(|home| !home.is_empty())
                    .map(PathBuf::from);
            } else {
                tracing::debug!("ignoring unknown argument {arg:?}");
            }
        }
        parsed
    }
}

/// OS의 로그인 항목
pub struct SystemLoginItems;

#[cfg(not(windows))]
impl LoginItems for SystemLoginItems {
    fn get(&self, _name: &str) -> Result<Option<String>, AutostartError> {
        Err(AutostartError::Unsupported)
    }

    fn set(&mut self, _name: &str, _command: &str) -> Result<(), AutostartError> {
        Err(AutostartError::Unsupported)
    }

    fn remove(&mut self, _name: &str) -> Result<(), AutostartError> {
        Err(AutostartError::Unsupported)
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey,
        RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
    };

    use super::{AutostartError, LoginItems, SystemLoginItems};

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn os_error(code: WIN32_ERROR) -> AutostartError {
        AutostartError::Os(std::io::Error::from_raw_os_error(code as i32))
    }

    /// 쓰기용으로 연 `Run` 키. 버리면 닫습니다.
    struct RunKey(HKEY);

    impl RunKey {
        fn open_for_write() -> Result<Self, AutostartError> {
            let mut key: HKEY = std::ptr::null_mut();
            let path = wide(RUN_KEY);
            // SAFETY: 널로 끝나는 경로와 결과를 받을 자리를 넘깁니다.
            let status = unsafe {
                RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_SET_VALUE, &mut key)
            };
            if status != ERROR_SUCCESS {
                return Err(os_error(status));
            }
            Ok(Self(key))
        }
    }

    impl Drop for RunKey {
        fn drop(&mut self) {
            // SAFETY: `open_for_write`에서 연 키입니다.
            unsafe { RegCloseKey(self.0) };
        }
    }

    impl LoginItems for SystemLoginItems {
        fn get(&self, name: &str) -> Result<Option<String>, AutostartError> {
            let path = wide(RUN_KEY);
            let name = wide(name);
            let mut size: u32 = 0;
            // SAFETY: 크기만 먼저 묻습니다.
            let status = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    path.as_ptr(),
                    name.as_ptr(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut size,
                )
            };
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            if status != ERROR_SUCCESS {
                return Err(os_error(status));
            }
            let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
            let mut size = (buffer.len() * 2) as u32;
            // SAFETY: `size` 바이트짜리 버퍼를 넘깁니다.
            let status = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    path.as_ptr(),
                    name.as_ptr(),
                    RRF_RT_REG_SZ,
                    std::ptr::null_mut(),
                    buffer.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            if status == ERROR_FILE_NOT_FOUND {
                return Ok(None);
            }
            if status != ERROR_SUCCESS {
                return Err(os_error(status));
            }
            let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
            Ok(Some(String::from_utf16_lossy(&buffer[..len])))
        }

        fn set(&mut self, name: &str, command: &str) -> Result<(), AutostartError> {
            let key = RunKey::open_for_write()?;
            let name = wide(name);
            let value = wide(command);
            // SAFETY: 널로 끝나는 UTF-16 값을 바이트 크기와 함께 넘깁니다.
            let status = unsafe {
                RegSetValueExW(
                    key.0,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    value.as_ptr().cast(),
                    (value.len() * 2) as u32,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(os_error(status));
            }
            Ok(())
        }

        fn remove(&mut self, name: &str) -> Result<(), AutostartError> {
            let key = RunKey::open_for_write()?;
            let name = wide(name);
            // SAFETY: 연 키와 널로 끝나는 이름을 넘깁니다.
            let status = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
            if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
                return Err(os_error(status));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// 시험용 로그인 항목 (실제 레지스트리 대신)
    #[derive(Default)]
    struct FakeItems {
        values: BTreeMap<String, String>,
        writes: usize,
    }

    impl LoginItems for FakeItems {
        fn get(&self, name: &str) -> Result<Option<String>, AutostartError> {
            Ok(self.values.get(name).cloned())
        }

        fn set(&mut self, name: &str, command: &str) -> Result<(), AutostartError> {
            self.writes += 1;
            self.values.insert(name.to_owned(), command.to_owned());
            Ok(())
        }

        fn remove(&mut self, name: &str) -> Result<(), AutostartError> {
            self.writes += 1;
            self.values.remove(name);
            Ok(())
        }
    }

    fn entry() -> Entry {
        Entry::new(Path::new(r"C:\Program Files\sulsul\sulsul.exe"), None)
    }

    #[test]
    fn entry_starts_in_the_background() {
        assert_eq!(
            entry().command,
            r#""C:\Program Files\sulsul\sulsul.exe" --background"#
        );
        assert_eq!(entry().name, ENTRY_NAME);
        let with_home = Entry::new(Path::new(r"C:\a\sulsul.exe"), Some(Path::new(r"D:\홈")));
        assert_eq!(
            with_home.command,
            r#""C:\a\sulsul.exe" --background --home "D:\홈""#
        );
    }

    #[test]
    fn each_home_folder_has_its_own_entry() {
        let exe = Path::new(r"C:\a\sulsul.exe");
        let a = Entry::new(exe, Some(Path::new(r"D:\홈")));
        assert!(a.name.starts_with(&format!("{ENTRY_NAME}-")), "{}", a.name);
        assert_eq!(a.name.len(), ENTRY_NAME.len() + 9);
        assert_eq!(
            a.name,
            Entry::new(Path::new(r"E:\moved.exe"), Some(Path::new(r"d:\홈\"))).name,
            "같은 폴더면 실행 파일을 옮겨도 같은 이름"
        );
        assert_ne!(a.name, Entry::new(exe, Some(Path::new(r"D:\다른 홈"))).name);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn development_builds_use_their_own_entry() {
        // 시험은 개발 빌드라서 설치본의 항목 이름과 달라야 합니다.
        assert_eq!(ENTRY_NAME, "sulsul-dev");
    }

    #[test]
    fn apply_registers_updates_and_removes() {
        let mut items = FakeItems::default();
        assert!(
            !apply(&mut items, &entry(), false).unwrap(),
            "없으면 지울 것도 없음"
        );
        assert!(apply(&mut items, &entry(), true).unwrap());
        assert_eq!(items.values.get(ENTRY_NAME), Some(&entry().command));
        assert!(
            !apply(&mut items, &entry(), true).unwrap(),
            "같으면 다시 쓰지 않음"
        );
        assert_eq!(items.writes, 1);

        // 실행 파일을 옮겼으면 새 경로로 고칩니다.
        let moved = Entry::new(Path::new(r"D:\sulsul.exe"), None);
        assert!(apply(&mut items, &moved, true).unwrap());
        assert_eq!(items.values.get(ENTRY_NAME), Some(&moved.command));

        assert!(apply(&mut items, &moved, false).unwrap());
        assert!(items.values.is_empty());
    }

    #[test]
    fn other_entries_are_left_alone() {
        let mut items = FakeItems::default();
        items.values.insert("other".into(), "other app".into());
        // 다른 폴더로 실행한 술술의 등록
        let other_home = Entry::new(Path::new(r"C:\a\sulsul.exe"), Some(Path::new(r"D:\홈")));
        apply(&mut items, &other_home, true).unwrap();

        apply(&mut items, &entry(), true).unwrap();
        apply(&mut items, &entry(), false).unwrap();
        assert_eq!(
            items.values.get("other").map(String::as_str),
            Some("other app")
        );
        assert_eq!(
            items.values.get(&other_home.name),
            Some(&other_home.command)
        );
        assert_eq!(items.values.len(), 2);
    }

    #[test]
    fn launch_args_are_parsed() {
        let parse = |args: &[&str]| LaunchArgs::parse(args.iter().map(OsString::from));
        assert_eq!(parse(&[]), LaunchArgs::default());
        assert_eq!(
            parse(&["--background", "--home", r"D:\홈", "--unknown"]),
            LaunchArgs {
                background: true,
                home: Some(PathBuf::from(r"D:\홈")),
            }
        );
        assert_eq!(parse(&["--home"]).home, None);
    }
}
