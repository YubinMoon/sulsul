//! 설정 파일과 화면 상태 맞추기.
//!
//! TOML 파일이 기준입니다.
//! - 화면에서 값을 바꾸면 잠깐 모았다가([`SAVE_DEBOUNCE`]) 파일에 씁니다.
//! - 파일이 밖에서 바뀌면 다시 읽어 화면에 반영합니다. 우리가 방금 쓴 내용 그대로면 무시합니다.
//! - 파일을 읽을 수 없으면 마지막으로 정상이던 값을 유지하고 편집을 잠급니다.

use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use sulsul_core::config::{
    Config, ConfigError, ConfigIssue, ConfigStore, ConfigWatcher, LoadedConfig,
};
use sulsul_core::toml;

/// 마지막 변경 뒤 이만큼 조용하면 파일에 씁니다.
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(350);

/// 설정 파일을 읽지 못한 이유
#[derive(Debug)]
pub struct FileProblem {
    pub error: ConfigError,
    pub kind: ProblemKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    /// TOML 문법이 깨짐. 저장하면 기존 파일은 `.bak`으로 옮겨집니다.
    Syntax,
    /// 문법은 맞지만 값의 형식이 다름. 저장하면 그 값만 바뀝니다.
    Value,
    /// 파일을 열 수 없음
    Io,
}

pub struct ConfigSync {
    store: ConfigStore,
    /// 화면에 보이는 설정. 파일을 읽지 못하는 동안에는 마지막으로 정상이던 값입니다.
    pub config: Config,
    /// 파일에 들어 있다고 알고 있는 설정. 파일 변경이 우리 저장 때문인지 가려내는 데 씁니다.
    on_disk: Config,
    issues: Vec<ConfigIssue>,
    problem: Option<FileProblem>,
    save_error: Option<ConfigError>,
    watch_error: Option<ConfigError>,
    dirty_since: Option<Instant>,
    events: Option<Receiver<()>>,
    _watcher: Option<ConfigWatcher>,
}

impl ConfigSync {
    /// 설정 파일을 읽습니다(없으면 기본 파일을 만듭니다).
    ///
    /// 파일에 없앤 설정 키가 남아 있으면 곧바로 한 번 저장해 파일에서 지우도록 예약합니다.
    pub fn open(store: ConfigStore) -> Self {
        let (config, issues, removed_keys, problem) = match store.load_or_create() {
            Ok(LoadedConfig {
                config,
                issues,
                removed_keys,
            }) => (config, issues, removed_keys, None),
            Err(error) => {
                tracing::warn!("could not load config: {error}");
                let problem = classify(&store, error);
                (Config::default(), Vec::new(), Vec::new(), Some(problem))
            }
        };
        let mut sync = Self {
            store,
            on_disk: config.clone(),
            config,
            issues,
            problem,
            save_error: None,
            watch_error: None,
            dirty_since: None,
            events: None,
            _watcher: None,
        };
        sync.clean_up_removed_keys(&removed_keys);
        sync
    }

    /// 파일 변경 감시를 시작합니다. 변경이 생기면 화면을 다시 그리게 합니다.
    pub fn start_watching(&mut self, ctx: egui::Context) {
        let (tx, rx) = mpsc::channel();
        let watcher = self.store.watch(move || {
            // 받는 쪽이 사라졌다면 앱이 끝나는 중입니다.
            let _ = tx.send(());
            ctx.request_repaint();
        });
        match watcher {
            Ok(watcher) => {
                self.events = Some(rx);
                self._watcher = Some(watcher);
                self.watch_error = None;
            }
            Err(error) => {
                tracing::warn!("could not watch config file: {error}");
                self.watch_error = Some(error);
            }
        }
    }

    pub fn path(&self) -> &Path {
        self.store.path()
    }

    pub fn issues(&self) -> &[ConfigIssue] {
        &self.issues
    }

    pub fn problem(&self) -> Option<&FileProblem> {
        self.problem.as_ref()
    }

    pub fn save_error(&self) -> Option<&ConfigError> {
        self.save_error.as_ref()
    }

    pub fn watch_error(&self) -> Option<&ConfigError> {
        self.watch_error.as_ref()
    }

    /// 파일을 읽지 못해 편집이 잠겨 있는지
    pub fn locked(&self) -> bool {
        self.problem.is_some()
    }

    /// 아직 파일에 쓰지 않은 변경이 있는지
    pub fn has_pending_save(&self) -> bool {
        self.dirty_since.is_some()
    }

    /// 미뤄 둔 저장까지 남은 시간. 저장할 것이 없거나 잠겨 있으면 `None`입니다.
    pub fn save_due_in(&self) -> Option<Duration> {
        if self.locked() {
            return None;
        }
        self.dirty_since
            .map(|since| SAVE_DEBOUNCE.saturating_sub(since.elapsed()))
    }

    /// 화면에서 `config`를 바꾼 뒤 부릅니다. 조금 뒤 [`Self::poll`]이 파일에 씁니다.
    pub fn mark_dirty(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    /// 매 프레임 부릅니다. 파일 변경 반영과 미뤄 둔 저장을 처리하고,
    /// `config`가 파일 내용으로 바뀌었으면 `true`를 돌려줍니다.
    pub fn poll(&mut self) -> bool {
        let mut replaced = false;
        let file_changed = self
            .events
            .as_ref()
            .is_some_and(|events| events.try_iter().count() > 0);
        if file_changed {
            replaced |= self.reload();
        }
        let save_due = self
            .dirty_since
            .is_some_and(|since| since.elapsed() >= SAVE_DEBOUNCE);
        if save_due && !self.locked() {
            replaced |= self.save_now();
        }
        replaced
    }

    /// 파일을 다시 읽습니다. `config`가 바뀌었으면 `true`를 돌려줍니다.
    ///
    /// 파일에 없앤 설정 키가 들어 있으면 지우도록 저장을 예약합니다.
    pub fn reload(&mut self) -> bool {
        match self.read() {
            Ok(loaded) => {
                self.problem = None;
                self.issues = loaded.issues;
                let replaced = self.apply_reloaded(loaded.config);
                self.clean_up_removed_keys(&loaded.removed_keys);
                replaced
            }
            Err(problem) => {
                tracing::warn!("could not reload config: {}", problem.error);
                self.problem = Some(problem);
                false
            }
        }
    }

    /// 다시 읽은 설정을 화면에 반영합니다. `config`가 바뀌었으면 `true`를 돌려줍니다.
    fn apply_reloaded(&mut self, config: Config) -> bool {
        if config == self.on_disk {
            // 우리가 저장한 내용 그대로입니다. 화면에서 고치던 값을 건드리지 않습니다.
            return false;
        }
        tracing::info!("config file changed outside the app; reloading");
        self.on_disk = config.clone();
        self.config = config;
        self.dirty_since = None;
        self.save_error = None;
        true
    }

    /// 파일에 남은 없앤 설정 키를 지우도록 저장을 예약합니다. 저장은 기존 흐름([`Self::poll`])을 탑니다.
    fn clean_up_removed_keys(&mut self, removed_keys: &[String]) {
        if removed_keys.is_empty() {
            return;
        }
        tracing::info!(
            "config file has removed keys ({}); saving to clean up",
            removed_keys.join(", ")
        );
        if self.dirty_since.is_none() {
            self.mark_dirty();
        }
    }

    /// 지금 화면 값을 파일에 씁니다. 저장 뒤 파일을 다시 읽어 화면과 맞추며,
    /// 그 결과 `config`가 달라졌으면 `true`를 돌려줍니다.
    ///
    /// 파일을 읽지 못하는 상태에서 부르면 화면 값으로 덮어씁니다
    /// (문법이 깨진 파일은 [`ConfigStore::save`]가 `.bak`으로 옮겨 둡니다).
    pub fn save_now(&mut self) -> bool {
        self.dirty_since = None;
        if let Err(error) = self.store.save(&self.config) {
            tracing::error!("could not save config: {error}");
            self.save_error = Some(error);
            return false;
        }
        self.save_error = None;
        match self.read() {
            Ok(loaded) => {
                self.problem = None;
                self.issues = loaded.issues;
                self.on_disk = loaded.config.clone();
                // 저장은 기존 파일에 값을 합치는 방식이라, 화면에서 뺀 항목이 파일에는
                // 남아 있을 수 있습니다. 기준은 파일이므로 읽은 값을 따릅니다.
                let replaced = loaded.config != self.config;
                self.config = loaded.config;
                replaced
            }
            Err(problem) => {
                tracing::warn!(
                    "config was saved but cannot be read back: {}",
                    problem.error
                );
                self.problem = Some(problem);
                false
            }
        }
    }

    fn read(&self) -> Result<LoadedConfig, FileProblem> {
        self.store
            .load()
            .map_err(|error| classify(&self.store, error))
    }
}

fn classify(store: &ConfigStore, error: ConfigError) -> FileProblem {
    let kind = match &error {
        ConfigError::Parse { .. } => {
            let valid_toml = std::fs::read_to_string(store.path())
                .is_ok_and(|text| toml::from_str::<toml::Table>(&text).is_ok());
            if valid_toml {
                ProblemKind::Value
            } else {
                ProblemKind::Syntax
            }
        }
        _ => ProblemKind::Io,
    };
    FileProblem { error, kind }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use sulsul_core::config::HotkeyMode;

    struct Home {
        dir: tempfile::TempDir,
    }

    impl Home {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn store(&self) -> ConfigStore {
            ConfigStore::new(self.dir.path().join("config.toml"))
        }
    }

    /// 감시 스레드 없이도 `poll`이 저장하도록 디바운스 시간을 넘깁니다.
    fn flush(sync: &mut ConfigSync) -> bool {
        sync.dirty_since = Some(Instant::now() - SAVE_DEBOUNCE);
        sync.poll()
    }

    #[test]
    fn edits_are_saved_after_debounce() {
        let home = Home::new();
        let mut sync = ConfigSync::open(home.store());
        sync.config.hotkey.mode = HotkeyMode::PushToTalk;
        sync.mark_dirty();

        assert!(!sync.poll(), "디바운스가 끝나기 전에는 저장하지 않습니다");
        assert!(sync.has_pending_save());
        assert!(!flush(&mut sync));
        assert!(!sync.has_pending_save());

        let text = fs::read_to_string(sync.path()).unwrap();
        assert!(text.contains("mode = \"push_to_talk\""));
        assert!(text.contains("# 술술(sulsul) 설정 파일"), "주석 유지");
    }

    #[test]
    fn own_save_is_not_treated_as_external_change() {
        let home = Home::new();
        let mut sync = ConfigSync::open(home.store());
        sync.config.audio.max_record_seconds = 30;
        sync.mark_dirty();
        flush(&mut sync);

        // 저장 직후 사용자가 또 고친 값은 감시 이벤트(다시 읽기)로 사라지면 안 됩니다.
        sync.config.audio.max_record_seconds = 45;
        sync.mark_dirty();
        assert!(!sync.reload());
        assert_eq!(sync.config.audio.max_record_seconds, 45);
        assert!(sync.has_pending_save());
    }

    #[test]
    fn external_edit_replaces_config() {
        let home = Home::new();
        let mut sync = ConfigSync::open(home.store());
        let text = fs::read_to_string(sync.path()).unwrap();
        fs::write(
            sync.path(),
            text.replace("theme = \"dark\"", "theme = \"light\""),
        )
        .unwrap();

        assert!(sync.reload());
        assert_eq!(sync.config.general.theme, sulsul_core::config::Theme::Light);
    }

    #[test]
    fn broken_file_keeps_last_good_config_and_locks() {
        let home = Home::new();
        let mut sync = ConfigSync::open(home.store());
        sync.config.audio.max_record_seconds = 77;
        sync.mark_dirty();
        flush(&mut sync);

        fs::write(sync.path(), "[audio\nmax_record_seconds = 5\n").unwrap();
        assert!(!sync.reload());
        assert!(sync.locked());
        assert_eq!(sync.problem().unwrap().kind, ProblemKind::Syntax);
        assert_eq!(sync.config.audio.max_record_seconds, 77);

        // 잠긴 동안에는 미뤄 둔 저장이 깨진 파일을 덮어쓰지 않습니다.
        sync.config.audio.max_record_seconds = 78;
        sync.mark_dirty();
        flush(&mut sync);
        assert!(sync.locked());
        assert!(!sync.path().with_extension("toml.bak").exists());

        // "현재 화면 값으로 덮어쓰기"
        sync.save_now();
        assert!(!sync.locked());
        assert!(sync.path().with_extension("toml.bak").exists());
        assert_eq!(sync.config.audio.max_record_seconds, 78);
    }

    #[test]
    fn wrong_value_type_is_not_a_syntax_problem() {
        let home = Home::new();
        let mut sync = ConfigSync::open(home.store());
        fs::write(sync.path(), "[audio]\nmax_record_seconds = \"long\"\n").unwrap();
        assert!(!sync.reload());
        assert_eq!(sync.problem().unwrap().kind, ProblemKind::Value);

        sync.save_now();
        assert!(!sync.locked());
        assert!(
            !sync.path().with_extension("toml.bak").exists(),
            "문법이 맞는 파일은 백업 없이 값만 고칩니다"
        );
    }

    #[test]
    fn fresh_file_needs_no_save() {
        let home = Home::new();
        let sync = ConfigSync::open(home.store());
        assert!(!sync.has_pending_save());
        assert!(sync.issues().is_empty(), "{:?}", sync.issues());
    }

    /// 없앤 키가 남은 예전 파일을 열면 경고 없이 바로 한 번 저장해 파일을 정리합니다.
    #[test]
    fn open_cleans_up_removed_keys() {
        let home = Home::new();
        let store = home.store();
        fs::write(
            store.path(),
            "[general]\nstart_minimized = true\n# 내가 넣은 키\nmy_key = 1\n\n\
             [output]\nmode = \"paste\"\nrestore_delay_ms = 150\n\n[hotkey]\nkey = \"F9\"\n",
        )
        .unwrap();

        let mut sync = ConfigSync::open(store);
        let paths: Vec<_> = sync.issues().iter().map(|i| i.path.as_str()).collect();
        assert_eq!(paths, ["general.my_key"], "직접 넣은 키만 알려야 합니다");
        assert!(sync.has_pending_save(), "정리 저장이 예약되어야 합니다");
        assert!(!flush(&mut sync));

        let text = fs::read_to_string(sync.path()).unwrap();
        assert!(!text.contains("start_minimized"), "{text}");
        assert!(
            !text.contains("mode = \"paste\"") && !text.contains("restore_delay_ms"),
            "{text}"
        );
        assert!(text.contains("# 내가 넣은 키\nmy_key = 1"), "{text}");
        assert_eq!(sync.config.hotkey.key, "F9");

        let reopened = ConfigSync::open(home.store());
        assert!(
            !reopened.has_pending_save(),
            "정리한 뒤에는 다시 저장하지 않습니다"
        );
    }

    /// 실행 중에 없앤 키가 든 파일로 바뀌어도 저장을 예약해 정리합니다.
    #[test]
    fn reload_cleans_up_removed_keys() {
        let home = Home::new();
        let mut sync = ConfigSync::open(home.store());
        let text = fs::read_to_string(sync.path()).unwrap();
        // 새 파일에도 `[output]` 표가 있으므로 그 안에 예전 키를 넣습니다.
        assert!(text.contains("[output]\n"), "{text}");
        let old = text.replacen("[output]\n", "[output]\nmode = \"type\"\n", 1);
        fs::write(sync.path(), old).unwrap();

        assert!(!sync.reload(), "설정 값은 그대로입니다");
        assert!(sync.issues().is_empty(), "{:?}", sync.issues());
        assert!(sync.has_pending_save());
        flush(&mut sync);
        let cleaned = fs::read_to_string(sync.path()).unwrap();
        assert!(!cleaned.contains("mode = \"type\""), "{cleaned}");
        assert!(!sync.reload());
        assert!(!sync.has_pending_save());
    }

    #[test]
    fn fixed_file_unlocks() {
        let home = Home::new();
        let mut sync = ConfigSync::open(home.store());
        let good = fs::read_to_string(sync.path()).unwrap();
        fs::write(sync.path(), "nope = [").unwrap();
        sync.reload();
        assert!(sync.locked());

        fs::write(sync.path(), good).unwrap();
        assert!(!sync.reload(), "내용이 그대로면 화면 값은 바뀌지 않습니다");
        assert!(!sync.locked());
    }
}
