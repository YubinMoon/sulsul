use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};
use toml_edit::DocumentMut;

use super::merge::merge_document;
use super::{Config, ConfigIssue, ConfigProblem, DEFAULT_CONFIG_TOML};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read {}: {source}", path.display())]
    Read { path: PathBuf, source: io::Error },
    #[error("failed to write {}: {source}", path.display())]
    Write { path: PathBuf, source: io::Error },
    #[error("failed to parse {}{}: {message}", path.display(), location(*line, *column))]
    Parse {
        path: PathBuf,
        message: String,
        /// 1부터 시작
        line: Option<usize>,
        /// 1부터 시작
        column: Option<usize>,
    },
    #[error("failed to serialize config: {0}")]
    Serialize(String),
    #[error("failed to watch {}: {message}", path.display())]
    Watch { path: PathBuf, message: String },
}

fn location(line: Option<usize>, column: Option<usize>) -> String {
    match (line, column) {
        (Some(line), Some(column)) => format!(" (line {line}, column {column})"),
        (Some(line), None) => format!(" (line {line})"),
        _ => String::new(),
    }
}

/// 읽어 들인 설정과 그 과정에서 발견한 문제
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig {
    pub config: Config,
    pub issues: Vec<ConfigIssue>,
}

/// 설정 파일 읽기·쓰기·감시
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 파일이 없으면 기본 설정 파일(주석 포함)을 만든 뒤 읽습니다.
    pub fn load_or_create(&self) -> Result<LoadedConfig, ConfigError> {
        if !self.path.exists() {
            self.write_text(DEFAULT_CONFIG_TOML)?;
        }
        self.load()
    }

    pub fn load(&self) -> Result<LoadedConfig, ConfigError> {
        let text = fs::read_to_string(&self.path).map_err(|source| ConfigError::Read {
            path: self.path.clone(),
            source,
        })?;
        self.parse(&text)
    }

    /// 설정 텍스트를 해석합니다. 모르는 키는 오류 대신 [`ConfigProblem::UnknownKey`]로 알려 줍니다.
    pub fn parse(&self, text: &str) -> Result<LoadedConfig, ConfigError> {
        let mut issues = Vec::new();
        let deserializer =
            toml::Deserializer::parse(text).map_err(|e| self.parse_error(text, &e))?;
        let config: Config = serde_ignored::deserialize(deserializer, |path| {
            issues.push(ConfigIssue {
                path: path.to_string(),
                problem: ConfigProblem::UnknownKey,
            });
        })
        .map_err(|e| self.parse_error(text, &e))?;
        issues.extend(config.validate());
        Ok(LoadedConfig { config, issues })
    }

    /// 설정을 저장합니다. 기존 파일의 주석·순서·직접 추가한 키는 유지합니다.
    ///
    /// 기존 파일이 문법 오류로 읽히지 않으면 `config.toml.bak`으로 옮겨 두고 새로 씁니다.
    pub fn save(&self, config: &Config) -> Result<(), ConfigError> {
        let serialized =
            toml::to_string(config).map_err(|e| ConfigError::Serialize(e.to_string()))?;
        let fresh: DocumentMut = serialized
            .parse()
            .map_err(|e: toml_edit::TomlError| ConfigError::Serialize(e.to_string()))?;

        let existing = match fs::read_to_string(&self.path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(source) => {
                return Err(ConfigError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        let mut doc = match existing.as_deref().map(str::parse::<DocumentMut>) {
            Some(Ok(doc)) => doc,
            Some(Err(_)) => {
                self.backup_broken_file()?;
                template_document()
            }
            None => template_document(),
        };
        merge_document(&mut doc, &fresh);
        // `Vec::is_empty`로 직렬화가 생략되는 배열 테이블은 모두 삭제됐을 때 명시적으로 비워 줍니다.
        if config.stt.api_profiles.is_empty()
            && let Some(stt_table) = doc.get_mut("stt").and_then(|item| item.as_table_mut())
        {
            stt_table.remove("api_profiles");
        }
        self.write_text(&doc.to_string())
    }

    /// 설정 파일이 바뀌면 `on_change`를 호출합니다. 반환값을 들고 있는 동안만 감시합니다.
    ///
    /// 에디터는 보통 파일을 통째로 교체하므로 파일이 아닌 폴더를 감시합니다.
    pub fn watch<F>(&self, mut on_change: F) -> Result<ConfigWatcher, ConfigError>
    where
        F: FnMut() + Send + 'static,
    {
        let watch_error = |message: String| ConfigError::Watch {
            path: self.path.clone(),
            message,
        };
        let dir = self
            .path
            .parent()
            .ok_or_else(|| watch_error("config path has no parent directory".into()))?
            .to_path_buf();
        let file_name = self
            .path
            .file_name()
            .ok_or_else(|| watch_error("config path has no file name".into()))?
            .to_os_string();

        let mut debouncer = new_debouncer(
            Duration::from_millis(200),
            move |result: DebounceEventResult| match result {
                Ok(events) => {
                    if events
                        .iter()
                        .any(|e| e.path.file_name() == Some(file_name.as_os_str()))
                    {
                        on_change();
                    }
                }
                Err(e) => tracing::warn!("config watcher error: {e}"),
            },
        )
        .map_err(|e| watch_error(e.to_string()))?;
        debouncer
            .watcher()
            .watch(&dir, RecursiveMode::NonRecursive)
            .map_err(|e| watch_error(e.to_string()))?;
        Ok(ConfigWatcher {
            _debouncer: debouncer,
        })
    }

    fn parse_error(&self, text: &str, error: &toml::de::Error) -> ConfigError {
        let (line, column) = match error.span() {
            Some(span) => {
                let (line, column) = line_column(text, span.start);
                (Some(line), Some(column))
            }
            None => (None, None),
        };
        ConfigError::Parse {
            path: self.path.clone(),
            message: error.message().to_owned(),
            line,
            column,
        }
    }

    fn backup_broken_file(&self) -> Result<(), ConfigError> {
        let backup = self.path.with_extension("toml.bak");
        tracing::warn!(
            "config file is not valid TOML; moving it to {}",
            backup.display()
        );
        fs::rename(&self.path, &backup).map_err(|source| ConfigError::Write {
            path: backup,
            source,
        })
    }

    /// 임시 파일에 쓴 뒤 바꿔치기해서, 쓰는 도중 앱이 꺼져도 파일이 깨지지 않게 합니다.
    fn write_text(&self, text: &str) -> Result<(), ConfigError> {
        let write_error = |source| ConfigError::Write {
            path: self.path.clone(),
            source,
        };
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(write_error)?;
        }
        let tmp = self.path.with_extension("toml.tmp");
        fs::write(&tmp, text).map_err(write_error)?;
        if let Err(e) = fs::rename(&tmp, &self.path) {
            // 다른 프로그램이 파일을 잡고 있으면 교체가 실패할 수 있어 직접 씁니다.
            tracing::debug!("atomic replace failed ({e}); writing in place");
            let _ = fs::remove_file(&tmp);
            fs::write(&self.path, text).map_err(write_error)?;
        }
        Ok(())
    }
}

fn template_document() -> DocumentMut {
    DEFAULT_CONFIG_TOML
        .parse()
        .expect("default config template must be valid TOML")
}

/// 바이트 위치를 1부터 시작하는 (줄, 칸)으로 바꿉니다.
fn line_column(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

/// [`ConfigStore::watch`]가 돌려주는 감시 핸들. 버리면 감시가 멈춥니다.
pub struct ConfigWatcher {
    _debouncer: Debouncer<RecommendedWatcher>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HotkeyMode;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn store() -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.toml"));
        (dir, store)
    }

    #[test]
    fn creates_default_file() {
        let (_dir, store) = store();
        let loaded = store.load_or_create().unwrap();
        assert_eq!(loaded.config, Config::default());
        assert!(loaded.issues.is_empty());
        assert_eq!(
            fs::read_to_string(store.path()).unwrap(),
            DEFAULT_CONFIG_TOML
        );
    }

    #[test]
    fn save_preserves_comments() {
        let (_dir, store) = store();
        let mut config = store.load_or_create().unwrap().config;
        config.hotkey.mode = HotkeyMode::PushToTalk;
        config
            .stt
            .engines
            .entry("mock".into())
            .or_default()
            .insert("delay_ms".into(), toml::Value::Integer(10));
        store.save(&config).unwrap();

        let text = fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("# 술술(sulsul) 설정 파일"));
        assert!(text.contains("# push_to_talk: 누르고 있는 동안만 녹음\nmode = \"push_to_talk\""));
        assert!(text.contains("[stt.engines.mock]\ndelay_ms = 10"));
        assert_eq!(store.load().unwrap().config, config);
    }

    #[test]
    fn reports_unknown_keys() {
        let (_dir, store) = store();
        let loaded = store
            .parse("[hotkey]\nkye = \"F9\"\n[unknown]\na = 1\n")
            .unwrap();
        let paths: Vec<_> = loaded.issues.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["hotkey.kye", "unknown"]);
    }

    #[test]
    fn parse_error_has_location() {
        let (_dir, store) = store();
        let err = store
            .parse("[audio]\nmax_record_seconds = \"long\"\n")
            .unwrap_err();
        match err {
            ConfigError::Parse { line, .. } => assert_eq!(line, Some(2)),
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn broken_file_is_backed_up_on_save() {
        let (_dir, store) = store();
        fs::write(store.path(), "this is [not toml").unwrap();
        store.save(&Config::default()).unwrap();
        assert!(store.path().with_extension("toml.bak").exists());
        assert_eq!(store.load().unwrap().config, Config::default());
    }

    #[test]
    fn watch_detects_external_edit() {
        let (_dir, store) = store();
        store.load_or_create().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let _watcher = store
            .watch(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();

        std::thread::sleep(Duration::from_millis(100));
        fs::write(store.path(), "[hotkey]\nmode = \"push_to_talk\"\n").unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while hits.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(hits.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn saving_empty_api_profiles_removes_stale_entries_and_preserves_comments() {
        use crate::config::ApiProfile;

        let (_dir, store) = store();
        store.load_or_create().unwrap();

        let mut config = store.load().unwrap().config;
        config.stt.api_profile = "p1".into();
        config.stt.api_profiles = vec![
            ApiProfile {
                id: "p1".into(),
                name: "OpenAI".into(),
                provider: "openai".into(),
                endpoint: "https://api.openai.com/v1".into(),
                api_key: "sk-1".into(),
                model: "whisper-1".into(),
            },
            ApiProfile {
                id: "p2".into(),
                name: "Groq".into(),
                provider: "groq".into(),
                endpoint: "https://api.groq.com/openai/v1".into(),
                api_key: "gsk-2".into(),
                model: "whisper-large-v3-turbo".into(),
            },
        ];
        store.save(&config).unwrap();

        let loaded = store.load().unwrap().config;
        assert_eq!(loaded.stt.api_profiles.len(), 2);

        config.stt.api_profiles.remove(0);
        config.stt.api_profile = "p2".into();
        store.save(&config).unwrap();
        let loaded = store.load().unwrap().config;
        assert_eq!(loaded.stt.api_profiles.len(), 1);
        assert_eq!(loaded.stt.api_profiles[0].id, "p2");

        config.stt.api_profiles.clear();
        config.stt.api_profile.clear();
        store.save(&config).unwrap();
        let loaded = store.load().unwrap().config;
        assert!(loaded.stt.api_profiles.is_empty());

        let text = fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("# 술술(sulsul) 설정 파일"));
    }
}
