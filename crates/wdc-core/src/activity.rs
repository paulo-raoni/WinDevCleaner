//! The agent activity log: what each connected agent did, shown live in the app's activity
//! panel and appended to `<data>\activity.jsonl` so the human keeps a record afterwards.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

/// Status of an activity entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Waiting,
    Done,
    Failed,
    Info,
}

/// What an entry is about.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum What {
    Connected,
    Disconnected { reason: String },
    Paused,
    Resumed,
    /// A tool call; `target` is the folder(s) it is about.
    Call { tool: String, target: String },
}

/// One line of the activity panel.
#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub id: u64,
    pub unix_ms: u64,
    /// Local time of day, for display.
    pub clock: String,
    pub client: String,
    #[serde(flatten)]
    pub what: What,
    pub status: Status,
    /// Short result for display ("6.1 GiB in 12,345 files").
    pub result: Option<String>,
    /// Machine-readable result fields (totals of a scan, for example).
    pub data: Option<serde_json::Value>,
}

/// The log: entries in memory for the panel, every change appended to a JSONL file.
pub struct Log {
    entries: Mutex<Vec<Entry>>,
    next_id: Mutex<u64>,
    file: Option<PathBuf>,
}

/// Entries kept in memory for the panel.
const MAX_ENTRIES: usize = 1000;

impl Log {
    /// A log persisted to `<data_dir>\activity.jsonl`.
    pub fn new(data_dir: &Path) -> Log {
        let _ = std::fs::create_dir_all(data_dir);
        Log { entries: Mutex::new(Vec::new()), next_id: Mutex::new(1), file: Some(data_dir.join("activity.jsonl")) }
    }

    /// A log kept only in memory.
    pub fn in_memory() -> Log {
        Log { entries: Mutex::new(Vec::new()), next_id: Mutex::new(1), file: None }
    }

    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    /// Adds an entry and returns its id.
    pub fn push(&self, client: &str, what: What, status: Status) -> u64 {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            let id = *n;
            *n += 1;
            id
        };
        let entry = Entry {
            id,
            unix_ms: crate::time::now_unix_ms(),
            clock: crate::time::local_clock(),
            client: client.to_string(),
            what,
            status,
            result: None,
            data: None,
        };
        self.persist(&entry);
        let mut entries = self.entries.lock().unwrap();
        entries.push(entry);
        if entries.len() > MAX_ENTRIES {
            let extra = entries.len() - MAX_ENTRIES;
            entries.drain(..extra);
        }
        id
    }

    /// Updates an entry's status and result.
    pub fn update(&self, id: u64, status: Status, result: Option<String>, data: Option<serde_json::Value>) {
        let updated = {
            let mut entries = self.entries.lock().unwrap();
            entries.iter_mut().rev().find(|e| e.id == id).map(|e| {
                e.status = status;
                if result.is_some() {
                    e.result = result;
                }
                if data.is_some() {
                    e.data = data;
                }
                e.clone()
            })
        };
        if let Some(e) = updated {
            self.persist(&e);
        }
    }

    /// A copy of the entries, oldest first.
    pub fn entries(&self) -> Vec<Entry> {
        self.entries.lock().unwrap().clone()
    }

    fn persist(&self, e: &Entry) {
        let Some(path) = &self.file else { return };
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            if let Ok(line) = serde_json::to_string(e) {
                let _ = writeln!(f, "{line}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_are_persisted_as_new_lines() {
        let dir = std::env::temp_dir().join(format!("wdc-act-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let log = Log::new(&dir);
        let id = log.push("Claude Code", What::Call { tool: "scan".into(), target: "C:\\x".into() }, Status::Running);
        log.update(id, Status::Done, Some("1 KiB".into()), Some(serde_json::json!({"files": 1})));
        let text = std::fs::read_to_string(dir.join("activity.jsonl")).unwrap();
        let lines: Vec<serde_json::Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["status"], "running");
        assert_eq!(lines[1]["status"], "done");
        assert_eq!(lines[1]["kind"], "call");
        assert_eq!(lines[1]["tool"], "scan");
        assert_eq!(lines[1]["client"], "Claude Code");
        assert_eq!(log.entries()[0].status, Status::Done);
        let _ = std::fs::remove_dir_all(dir);
    }
}
