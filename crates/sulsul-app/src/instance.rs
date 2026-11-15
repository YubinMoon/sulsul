//! 한 번만 실행하기.
//!
//! 같은 설정 폴더를 쓰는 술술이 이미 떠 있으면, 떠 있는 앱에 창을 보여 달라고 알리고 새 실행은 끝납니다.
//! 설정 폴더가 다르면(`SULSUL_HOME`, `--home`) 따로 실행됩니다.
//!
//! Windows는 이름 있는 뮤텍스로 이미 떠 있는지 알아보고, 이름 있는 이벤트로 창을 보여 달라고 알립니다.
//! 다른 OS는 아직 지원하지 않아 늘 새로 실행됩니다.

use std::path::Path;

/// 실행을 이어 갈지
pub enum Instance {
    /// 처음 실행된 앱입니다.
    Primary(PrimaryInstance),
    /// 이미 떠 있는 앱이 있습니다. 새 실행은 끝내야 합니다.
    Secondary,
}

/// 처음 실행된 앱의 표시. 앱이 끝날 때까지 들고 있어야 합니다.
pub struct PrimaryInstance {
    #[cfg(windows)]
    handles: Option<windows::Handles>,
}

/// 설정 폴더마다 다른 이름의 바탕. 대소문자와 구분자 차이는 같은 폴더로 봅니다.
fn instance_key(config_dir: &Path) -> String {
    format!("sulsul-{:016x}", folder_hash(config_dir))
}

/// 폴더 경로의 해시. 대소문자와 구분자 차이, 끝의 구분자는 같은 폴더로 봅니다.
pub fn folder_hash(path: &Path) -> u64 {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let normalized = absolute
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase();
    // FNV-1a: 빌드나 Rust 버전이 달라도 같은 값이 나와야 하므로 표준 해시를 쓰지 않습니다.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in normalized.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// 이미 떠 있는 앱이 있는지 확인합니다. 있으면 `show_window`일 때 그 앱에 창을 보여 달라고 알립니다.
pub fn acquire(config_dir: &Path, show_window: bool) -> Instance {
    let key = instance_key(config_dir);
    #[cfg(windows)]
    {
        windows::acquire(&key, show_window)
    }
    #[cfg(not(windows))]
    {
        let _ = (key, show_window);
        Instance::Primary(PrimaryInstance {})
    }
}

impl PrimaryInstance {
    /// 다른 실행이 창을 보여 달라고 하면 `on_show`를 부릅니다 (따로 도는 스레드에서).
    pub fn listen(&self, on_show: impl Fn() + Send + 'static) {
        #[cfg(windows)]
        if let Some(handles) = &self.handles {
            handles.listen(on_show);
        }
        #[cfg(not(windows))]
        let _ = on_show;
    }
}

#[cfg(windows)]
mod windows {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, INFINITE, SetEvent, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

    use super::{Instance, PrimaryInstance};

    /// 뮤텍스와 이벤트. 앱이 끝날 때까지 닫지 않습니다(닫으면 다음 실행이 처음 실행이 됨).
    pub struct Handles {
        _mutex: Owned,
        _event: Owned,
        event_name: Vec<u16>,
    }

    /// 닫을 핸들. 스레드로 넘길 수 있게 주소값으로 둡니다.
    struct Owned(usize);

    impl Owned {
        fn handle(&self) -> HANDLE {
            self.0 as HANDLE
        }
    }

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: 우리가 만든 핸들을 한 번만 닫습니다.
            unsafe { CloseHandle(self.handle()) };
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 이름 있는 이벤트를 만들거나(이미 있으면) 엽니다. 자동으로 다시 꺼지고 처음에는 꺼져 있습니다.
    fn open_event(name: &[u16]) -> Option<Owned> {
        // SAFETY: 널로 끝나는 이름을 넘깁니다.
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        if event.is_null() {
            tracing::warn!(
                "could not create the single-instance event: {}",
                std::io::Error::last_os_error()
            );
            return None;
        }
        Some(Owned(event as usize))
    }

    pub fn acquire(key: &str, show_window: bool) -> Instance {
        // 세션마다 따로 둡니다(`Local\`). 이벤트를 먼저 만들어야, 처음 실행이 듣기 전에 온 알림도 남습니다.
        let event_name = wide(&format!("Local\\{key}-show"));
        let mutex_name = wide(&format!("Local\\{key}-instance"));
        let Some(event) = open_event(&event_name) else {
            return Instance::Primary(PrimaryInstance { handles: None });
        };
        // SAFETY: 널로 끝나는 이름을 넘깁니다. 뮤텍스는 소유하지 않고 이름으로 존재만 알립니다.
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
        // SAFETY: 바로 앞 호출의 결과를 읽습니다.
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if mutex.is_null() {
            tracing::warn!(
                "could not create the single-instance mutex: {}",
                std::io::Error::last_os_error()
            );
            return Instance::Primary(PrimaryInstance { handles: None });
        }
        let mutex = Owned(mutex as usize);
        if already {
            tracing::info!("sulsul is already running");
            if show_window {
                // SAFETY: 떠 있는 앱이 창을 앞으로 가져올 수 있게 허락하고 알립니다.
                unsafe {
                    AllowSetForegroundWindow(ASFW_ANY);
                    SetEvent(event.handle());
                }
            }
            return Instance::Secondary;
        }
        Instance::Primary(PrimaryInstance {
            handles: Some(Handles {
                _mutex: mutex,
                _event: event,
                event_name,
            }),
        })
    }

    impl Handles {
        pub fn listen(&self, on_show: impl Fn() + Send + 'static) {
            // 듣는 스레드는 자기 핸들을 따로 엽니다. 기다리는 동안 핸들이 닫히는 일이 없게 합니다.
            let Some(event) = open_event(&self.event_name) else {
                return;
            };
            let spawned = std::thread::Builder::new()
                .name("sulsul-instance".into())
                .spawn(move || {
                    loop {
                        // SAFETY: 이 스레드가 가진 핸들이라 기다리는 동안 닫히지 않습니다.
                        let result = unsafe { WaitForSingleObject(event.handle(), INFINITE) };
                        if result != WAIT_OBJECT_0 {
                            tracing::warn!("stopped listening for other launches");
                            break;
                        }
                        on_show();
                    }
                });
            if let Err(error) = spawned {
                tracing::warn!("could not listen for other launches: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_depends_on_the_folder_only() {
        let a = instance_key(Path::new(r"C:\Users\me\AppData\Roaming\sulsul\config"));
        assert_eq!(
            a,
            instance_key(Path::new(r"c:\users\ME\AppData\Roaming\sulsul\config\"))
        );
        assert_eq!(
            a,
            instance_key(Path::new("C:/Users/me/AppData/Roaming/sulsul/config"))
        );
        assert_ne!(a, instance_key(Path::new(r"D:\sulsul-home\config")));
        assert!(a.starts_with("sulsul-"));
        assert!(!a.contains('\\'), "이름에 경로 구분자가 있으면 안 됨");
    }

    #[cfg(windows)]
    #[test]
    fn second_launch_with_the_same_folder_is_secondary() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire(dir.path(), false);
        assert!(matches!(first, Instance::Primary(_)));

        let (tx, rx) = std::sync::mpsc::channel();
        if let Instance::Primary(primary) = &first {
            primary.listen(move || {
                let _ = tx.send(());
            });
        }
        assert!(matches!(acquire(dir.path(), true), Instance::Secondary));
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .expect("처음 실행이 창을 보여 달라는 알림을 받아야 합니다");

        let other = tempfile::tempdir().unwrap();
        assert!(matches!(acquire(other.path(), false), Instance::Primary(_)));
        drop(first);
    }
}
