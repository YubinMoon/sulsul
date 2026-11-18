//! 단축키 받아쓰기 기록.
//!
//! 단축키 받아쓰기가 글을 만들 때마다 받아쓴 원문과 (다듬었으면) 다듬은 글을 하나씩 남깁니다.
//! 기록은 설정 파일이 아니라 앱 데이터 폴더의 [`AppPaths::history_file`](JSON)에 두고, 최근
//! [`MAX_ENTRIES`]개만 남깁니다(넘치면 오래된 것부터 지움).
//!
//! 파일 읽기·쓰기는 작업 스레드(`sulsul-history`)가 맡아 화면 스레드를 막지 않습니다. 작업 스레드는
//! 시작하자마자 파일을 읽어 넘기고, 그 뒤로는 화면 스레드가 보낸 목록 사본을 씁니다(밀린 사본이
//! 여럿이면 마지막 것만). 파일이 깨져 있으면 옆에 `history.json.bak`으로 옮겨 두고 빈 기록으로 시작합니다.
//!
//! [`AppPaths::history_file`]: sulsul_core::AppPaths::history_file

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::clock;

/// 남기는 기록의 최대 개수
pub const MAX_ENTRIES: usize = 200;
/// 기록 파일 형식의 판
const FORMAT_VERSION: u32 = 1;

/// 받아쓰기 한 번의 기록
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// 화면에서 기록을 구분하는 번호. 파일에는 저장하지 않고 목록에 넣을 때 매깁니다.
    #[serde(skip)]
    pub id: u64,
    /// 받아쓴 시각 (유닉스 시각, 밀리초)
    pub time: i64,
    /// 받아쓴 원문
    pub transcript: String,
    /// 글 다듬기 결과
    #[serde(default, skip_serializing_if = "Refinement::is_none")]
    pub refine: Refinement,
}

impl Entry {
    pub fn new(at: SystemTime, transcript: impl Into<String>, refine: Refinement) -> Self {
        Self {
            id: 0,
            time: clock::unix_millis(at),
            transcript: transcript.into(),
            refine,
        }
    }

    /// 다듬은 글 (다듬었을 때만)
    pub fn refined_text(&self) -> Option<&str> {
        match &self.refine {
            Refinement::Done { text, .. } => Some(text),
            Refinement::None | Refinement::Failed { .. } => None,
        }
    }
}

/// 글 다듬기 결과
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Refinement {
    /// 다듬지 않음 (글 다듬기를 꺼 둠)
    #[default]
    None,
    /// 다듬은 글
    Done {
        text: String,
        /// 쓴 프롬프트 이름
        #[serde(default)]
        prompt: String,
    },
    /// 다듬기를 시도했다가 실패함
    Failed {
        /// 쓰려던 프롬프트 이름
        #[serde(default)]
        prompt: String,
        /// 실패 원인 (화면 문구로 바꾼 것)
        #[serde(default)]
        reason: String,
    },
}

impl Refinement {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// 쓴(쓰려던) 프롬프트 이름. 다듬지 않았거나 이름이 없으면 `None`.
    pub fn prompt(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Done { prompt, .. } | Self::Failed { prompt, .. } => {
                Some(prompt.as_str()).filter(|name| !name.trim().is_empty())
            }
        }
    }
}

/// 기록 목록. 화면 스레드가 들고 있고, 바꿀 때마다 작업 스레드에 저장을 맡깁니다.
#[derive(Default)]
pub struct History {
    /// 오래된 것부터
    entries: Vec<Entry>,
    next_id: u64,
    /// 파일을 아직 다 읽지 않았으면 그 결과를 받을 곳
    loading: Option<Receiver<Vec<Entry>>>,
    /// 파일에 쓰는 작업 스레드. 없으면 기록을 메모리에만 둡니다.
    saver: Option<Saver>,
}

struct Saver {
    tx: Sender<Vec<Entry>>,
    thread: JoinHandle<()>,
}

impl History {
    /// `path`의 기록을 작업 스레드에서 읽기 시작합니다. 다 읽으면 `wake`를 부릅니다(화면을 다시 그리게 함).
    /// 작업 스레드를 띄우지 못하면 파일 없이 메모리에만 둡니다.
    pub fn open(path: PathBuf, wake: impl Fn() + Send + 'static) -> Self {
        let (save_tx, save_rx) = mpsc::channel::<Vec<Entry>>();
        let (loaded_tx, loaded_rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("sulsul-history".into())
            .spawn(move || {
                let _ = loaded_tx.send(load(&path));
                wake();
                while let Ok(mut entries) = save_rx.recv() {
                    // 밀린 사본이 있으면 마지막 것만 씁니다.
                    while let Ok(newer) = save_rx.try_recv() {
                        entries = newer;
                    }
                    match save(&path, &entries) {
                        Ok(()) => tracing::debug!(entries = entries.len(), "history saved"),
                        Err(error) => {
                            tracing::warn!("could not save the dictation history: {error}")
                        }
                    }
                }
            });
        match spawned {
            Ok(thread) => Self {
                entries: Vec::new(),
                next_id: 0,
                loading: Some(loaded_rx),
                saver: Some(Saver {
                    tx: save_tx,
                    thread,
                }),
            },
            Err(error) => {
                tracing::warn!(
                    "could not start the history thread; history will not be saved: {error}"
                );
                Self::default()
            }
        }
    }

    /// 파일을 다 읽었으면 목록에 반영합니다. 매 프레임 부릅니다.
    pub fn poll(&mut self) {
        let Some(rx) = &self.loading else {
            return;
        };
        match rx.try_recv() {
            Ok(loaded) => self.finish_loading(loaded),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                tracing::warn!("the history thread stopped before loading the history");
                self.finish_loading(Vec::new());
            }
        }
    }

    /// 아직 파일을 읽는 중인지. 읽는 동안에는 목록을 보여 주거나 지우지 않습니다.
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    /// 기록 (오래된 것부터)
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// 새 기록을 더합니다. 파일을 읽는 중이면 다 읽은 뒤 그 뒤에 붙여 저장합니다.
    pub fn push(&mut self, mut entry: Entry) {
        entry.id = self.take_id();
        self.entries.push(entry);
        trim(&mut self.entries);
        self.save();
    }

    /// 기록 하나를 지웁니다. 지웠으면 `true`.
    pub fn remove(&mut self, id: u64) -> bool {
        if self.is_loading() {
            return false;
        }
        let before = self.entries.len();
        self.entries.retain(|entry| entry.id != id);
        let removed = self.entries.len() != before;
        if removed {
            self.save();
        }
        removed
    }

    /// 기록을 모두 지웁니다.
    pub fn clear(&mut self) {
        if self.is_loading() {
            return;
        }
        self.entries.clear();
        self.save();
    }

    /// 밀린 저장을 마치고 작업 스레드를 끝냅니다 (앱을 끝낼 때). 파일을 읽는 중이었으면 다 읽고
    /// 그사이 들어온 기록까지 합쳐 저장합니다.
    pub fn shutdown(&mut self) {
        if let Some(rx) = self.loading.as_ref() {
            let loaded = rx.recv().unwrap_or_default();
            self.finish_loading(loaded);
        }
        if let Some(saver) = self.saver.take() {
            drop(saver.tx);
            if saver.thread.join().is_err() {
                tracing::warn!("the history thread panicked");
            }
        }
    }

    /// 읽은 기록 뒤에 읽는 동안 들어온 기록을 붙입니다.
    fn finish_loading(&mut self, mut loaded: Vec<Entry>) {
        self.loading = None;
        for entry in &mut loaded {
            entry.id = self.take_id();
        }
        let arrived = std::mem::replace(&mut self.entries, loaded);
        let changed = !arrived.is_empty();
        self.entries.extend(arrived);
        trim(&mut self.entries);
        if changed {
            self.save();
        }
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// 지금 목록을 작업 스레드에 넘겨 씁니다. 파일을 읽기 전에는 덮어쓰지 않게 미룹니다.
    fn save(&self) {
        if self.is_loading() {
            return;
        }
        if let Some(saver) = &self.saver
            && saver.tx.send(self.entries.clone()).is_err()
        {
            tracing::warn!("the history thread stopped; history will not be saved");
        }
    }
}

impl Drop for History {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 넘치는 오래된 기록을 지웁니다.
fn trim(entries: &mut Vec<Entry>) {
    let overflow = entries.len().saturating_sub(MAX_ENTRIES);
    entries.drain(..overflow);
}

#[derive(Serialize)]
struct FileOut<'a> {
    version: u32,
    entries: &'a [Entry],
}

#[derive(Deserialize)]
struct FileIn {
    #[serde(default)]
    entries: Vec<serde_json::Value>,
}

/// 기록 파일을 읽습니다 (오래된 것부터). 파일이 없거나 읽지 못하면 빈 기록입니다. 파일이 깨져 있으면
/// 옆으로 옮겨 두고 빈 기록으로 시작합니다. 기록 하나만 알아볼 수 없으면 그것만 빼고 읽습니다.
pub fn load(path: &Path) -> Vec<Entry> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            tracing::warn!(path = %path.display(), "could not read the dictation history: {error}");
            return Vec::new();
        }
    };
    let file: FileIn = match serde_json::from_slice(&bytes) {
        Ok(file) => file,
        Err(error) => {
            let backup = sibling(path, "bak");
            tracing::warn!(
                path = %path.display(),
                backup = %backup.display(),
                "the dictation history file is broken; starting with an empty history: {error}"
            );
            if let Err(error) = std::fs::rename(path, &backup) {
                tracing::warn!("could not move the broken history file aside: {error}");
            }
            return Vec::new();
        }
    };
    let mut entries: Vec<Entry> = file
        .entries
        .into_iter()
        .filter_map(|value| match serde_json::from_value(value) {
            Ok(entry) => Some(entry),
            Err(error) => {
                tracing::warn!("skipped an unreadable dictation history entry: {error}");
                None
            }
        })
        .collect();
    trim(&mut entries);
    entries
}

/// 기록을 파일에 씁니다. 임시 파일에 다 쓴 뒤 바꿔 넣으므로, 쓰다 멈춰도 원래 파일이 깨지지 않습니다.
pub fn save(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(&FileOut {
        version: FORMAT_VERSION,
        entries,
    })?;
    let temp = sibling(path, "tmp");
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, path)
}

/// `history.json` 옆의 `history.json.<suffix>`
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant, UNIX_EPOCH};

    use sulsul_core::AppPaths;

    use super::*;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn sample() -> Vec<Entry> {
        vec![
            Entry::new(at(1), "그냥 받아쓴 글", Refinement::None),
            Entry::new(
                at(2),
                "다듬기 전 글",
                Refinement::Done {
                    text: "다듬은 글.".into(),
                    prompt: "기본 다듬기".into(),
                },
            ),
            Entry::new(
                at(3),
                "다듬지 못한 글",
                Refinement::Failed {
                    prompt: "기본 다듬기".into(),
                    reason: "API 키가 맞지 않습니다.".into(),
                },
            ),
        ]
    }

    fn texts(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.transcript.as_str()).collect()
    }

    /// 작업 스레드가 파일을 다 읽을 때까지 기다립니다.
    fn wait_loaded(history: &mut History) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while history.is_loading() {
            assert!(Instant::now() < deadline, "기록을 불러오지 못했습니다");
            std::thread::sleep(Duration::from_millis(5));
            history.poll();
        }
    }

    #[test]
    fn saved_history_loads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = AppPaths::under(dir.path()).history_file();
        save(&path, &sample()).unwrap();
        assert_eq!(load(&path), sample());

        // 다듬지 않은 기록에는 다듬기 결과를 적지 않습니다.
        let json = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], 1);
        assert!(value["entries"][0].get("refine").is_none(), "{json}");
        assert_eq!(value["entries"][1]["refine"]["status"], "done");
        assert_eq!(value["entries"][2]["refine"]["status"], "failed");
        assert!(
            value["entries"][0].get("id").is_none(),
            "번호는 저장하지 않습니다"
        );
    }

    #[test]
    fn missing_file_is_an_empty_history() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(&dir.path().join("history.json")).is_empty());
    }

    #[test]
    fn broken_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        std::fs::write(&path, "{ 이건 JSON이 아님").unwrap();
        assert!(load(&path).is_empty());
        assert!(!path.exists(), "깨진 파일은 옆으로 옮깁니다");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("history.json.bak")).unwrap(),
            "{ 이건 JSON이 아님"
        );
    }

    #[test]
    fn unreadable_entry_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let json = r#"{
            "version": 1,
            "entries": [
                { "time": 1000, "transcript": "앞" },
                { "time": "언제", "transcript": 3 },
                { "time": 2000, "transcript": "뒤", "refine": { "status": "done", "text": "뒤." } },
                { "time": 3000, "transcript": "모르는 상태", "refine": { "status": "someday" } }
            ]
        }"#;
        std::fs::write(&path, json).unwrap();
        let entries = load(&path);
        assert_eq!(texts(&entries), ["앞", "뒤"]);
        assert_eq!(entries[1].refined_text(), Some("뒤."));
        assert_eq!(
            entries[1].refine.prompt(),
            None,
            "빈 프롬프트 이름은 없는 것으로 봅니다"
        );
    }

    #[test]
    fn oldest_entries_are_dropped_past_the_limit() {
        let mut history = History::default();
        for n in 0..MAX_ENTRIES + 5 {
            history.push(Entry::new(at(n as u64), n.to_string(), Refinement::None));
        }
        let entries = history.entries();
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0].transcript, "5", "오래된 것부터 지웁니다");
        assert_eq!(
            entries[MAX_ENTRIES - 1].transcript,
            (MAX_ENTRIES + 4).to_string()
        );

        // 파일에 더 많이 들어 있어도 최근 것만 읽습니다.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        let many: Vec<Entry> = (0..MAX_ENTRIES + 3)
            .map(|n| Entry::new(at(n as u64), n.to_string(), Refinement::None))
            .collect();
        save(&path, &many).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), MAX_ENTRIES);
        assert_eq!(loaded[0].transcript, "3");
    }

    #[test]
    fn changes_are_saved_by_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let path = AppPaths::under(dir.path()).history_file();
        save(&path, &sample()).unwrap();

        let mut history = History::open(path.clone(), || {});
        wait_loaded(&mut history);
        assert_eq!(texts(history.entries()), texts(&sample()));

        history.push(Entry::new(at(4), "새 글", Refinement::None));
        let second = history.entries()[1].id;
        assert!(history.remove(second));
        assert!(!history.remove(second), "이미 지운 기록");
        history.shutdown();
        assert_eq!(
            texts(&load(&path)),
            ["그냥 받아쓴 글", "다듬지 못한 글", "새 글"]
        );

        let mut history = History::open(path.clone(), || {});
        wait_loaded(&mut history);
        history.clear();
        history.shutdown();
        assert!(load(&path).is_empty());
        assert!(path.exists(), "전체 지우기는 빈 기록 파일을 남깁니다");
    }

    #[test]
    fn entries_added_while_loading_follow_the_loaded_ones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        save(&path, &sample()).unwrap();

        let mut history = History::open(path.clone(), || {});
        // 불러오기를 기다리지 않고 바로 더합니다. 읽기 전이라 파일을 덮어쓰지 않습니다.
        history.push(Entry::new(at(9), "읽는 동안", Refinement::None));
        history.shutdown();
        assert_eq!(
            texts(&load(&path)),
            [
                "그냥 받아쓴 글",
                "다듬기 전 글",
                "다듬지 못한 글",
                "읽는 동안"
            ]
        );
        // 읽는 동안 들어온 기록과 읽은 기록의 번호가 겹치지 않습니다.
        let mut ids: Vec<u64> = history.entries().iter().map(|e| e.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn broken_file_does_not_stop_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        std::fs::write(&path, [0xff, 0x00, 0x13]).unwrap();
        let mut history = History::open(path.clone(), || {});
        wait_loaded(&mut history);
        assert!(history.entries().is_empty());
        history.push(Entry::new(at(1), "새로 시작", Refinement::None));
        history.shutdown();
        assert_eq!(texts(&load(&path)), ["새로 시작"]);
    }
}
