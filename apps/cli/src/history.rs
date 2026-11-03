//! Dictation history, kept only on this Mac: one JSON object per line in
//! `~/Library/Application Support/MuskFlow/history.jsonl`. Shown in the MuskFlow window.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Unix milliseconds when the text was typed; also the entry's id.
    pub at: u64,
    /// What was typed.
    pub text: String,
    /// What the speech model heard.
    pub raw: String,
    /// App the text went into, when known.
    #[serde(default)]
    pub app: Option<String>,
    /// "dictate" or "command".
    pub mode: String,
    /// Seconds of speech.
    pub spoke_secs: f32,
    /// Key-up → text on screen, in milliseconds.
    pub latency_ms: u32,
}

impl Entry {
    pub fn words(&self) -> usize {
        self.text.split_whitespace().count()
    }
}

pub struct History {
    path: PathBuf,
}

impl History {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join("Library/Application Support/MuskFlow/history.jsonl")
    }

    pub fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }

    pub fn append(&self, entry: &Entry) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("could not open {}", self.path.display()))?;
        writeln!(f, "{}", serde_json::to_string(entry)?)?;
        Ok(())
    }

    /// Every entry, newest first. Unreadable lines are skipped.
    pub fn load(&self) -> Vec<Entry> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        let mut entries: Vec<Entry> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        entries.reverse();
        entries
    }

    /// Removes one entry (by `at`), or all of them with `None`.
    pub fn delete(&self, at: Option<u64>) -> Result<()> {
        let keep: Vec<Entry> = match at {
            None => Vec::new(),
            Some(at) => {
                let mut v = self.load();
                v.retain(|e| e.at != at);
                v.reverse();
                v
            }
        };
        write_all(&self.path, &keep)
    }
}

fn write_all(path: &Path, entries: &[Entry]) -> Result<()> {
    let mut out = String::new();
    for e in entries {
        out.push_str(&serde_json::to_string(e)?);
        out.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(at: u64, text: &str) -> Entry {
        Entry {
            at,
            text: text.into(),
            raw: text.into(),
            app: None,
            mode: "dictate".into(),
            spoke_secs: 1.0,
            latency_ms: 300,
        }
    }

    #[test]
    fn appends_loads_newest_first_and_deletes() {
        let dir = std::env::temp_dir().join(format!("mf-history-{}", History::now_ms()));
        let h = History::new(dir.join("history.jsonl"));
        h.append(&entry(1, "first")).unwrap();
        h.append(&entry(2, "second words here")).unwrap();
        let all = h.load();
        assert_eq!(all.iter().map(|e| e.at).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(all[0].words(), 3);
        h.delete(Some(2)).unwrap();
        assert_eq!(h.load().len(), 1);
        h.delete(None).unwrap();
        assert!(h.load().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
