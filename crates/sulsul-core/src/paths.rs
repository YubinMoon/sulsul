//! 앱이 사용하는 파일 경로.
//!
//! 기본값은 OS 표준 위치이고(Windows: 설정은 `%APPDATA%\sulsul\config`,
//! 데이터는 `%LOCALAPPDATA%\sulsul\data`), 환경 변수 `SULSUL_HOME`을 지정하면
//! 모든 경로를 그 아래로 모읍니다(개발·테스트용).

use std::path::PathBuf;

use directories::ProjectDirs;

pub const APP_ID: &str = "sulsul";
pub const HOME_ENV: &str = "SULSUL_HOME";
const CONFIG_FILE_NAME: &str = "config.toml";
const HISTORY_FILE_NAME: &str = "history.json";
const LOG_FILE_NAME: &str = "sulsul.log";

#[derive(Debug, thiserror::Error)]
pub enum PathsError {
    #[error("could not determine the user's home directory")]
    NoHomeDir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    /// 설정 파일이 있는 폴더
    pub config_dir: PathBuf,
    /// 모델·녹음 등 큰 데이터를 두는 폴더 (로밍되지 않는 위치)
    pub data_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Result<Self, PathsError> {
        if let Some(home) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
            return Ok(Self::under(home));
        }
        let dirs = ProjectDirs::from("", "", APP_ID).ok_or(PathsError::NoHomeDir)?;
        Ok(Self {
            config_dir: dirs.config_dir().to_path_buf(),
            data_dir: dirs.data_local_dir().to_path_buf(),
        })
    }

    /// 모든 경로를 `root` 아래에 둡니다.
    pub fn under(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            config_dir: root.join("config"),
            data_dir: root.join("data"),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILE_NAME)
    }

    pub fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }

    pub fn recordings_dir(&self) -> PathBuf {
        self.data_dir.join("recordings")
    }

    /// 앱 로그 폴더
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    /// 앱 로그 파일 (로그 폴더 안). 커지면 같은 폴더의 `sulsul.old.log`로 돌리고 새로 시작합니다.
    pub fn log_file(&self) -> PathBuf {
        self.logs_dir().join(LOG_FILE_NAME)
    }

    /// 단축키 받아쓰기 기록 파일 (설정 파일과 따로 둡니다)
    pub fn history_file(&self) -> PathBuf {
        self.data_dir.join(HISTORY_FILE_NAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_file_sits_in_the_data_logs_folder() {
        let paths = AppPaths::under("home");
        assert_eq!(paths.logs_dir(), PathBuf::from("home/data/logs"));
        assert_eq!(paths.log_file(), PathBuf::from("home/data/logs/sulsul.log"));
    }
}
