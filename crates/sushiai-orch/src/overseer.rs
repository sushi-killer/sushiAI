//! The overseer log: `<data>/overseer.jsonl` (mode 0600), one line per action the
//! orchestrator agent took when orchd woke it, across all repos. It keeps the
//! last `MAX_LINES` lines and is rewritten atomically on every append. An entry
//! holds one line of text, at most `MAX_TEXT_CHARS`: the agent's answer, the
//! first line of its reply, a pull request URL or a short error. It is the
//! agent's own words, so it can hold anything the agent wrote in that line.

use crate::store;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_LINES: usize = 2000;
pub const MAX_TEXT_CHARS: usize = 200;
const DEFAULT_LIMIT: usize = 200;

static APPEND: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub ts: i64,
    pub repo: String,
    /// `woke`, `answered`, `pr`, `asked_owner`, `digest`, `done`, `skipped` or `failed`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl Entry {
    /// An entry whose text is cut to its first line, at most `MAX_TEXT_CHARS`.
    pub fn new(kind: &str, repo: &str, task_id: Option<&str>, text: &str) -> Entry {
        let line = text.lines().map(str::trim).find(|l| !l.is_empty());
        let line = line.unwrap_or("");
        let text = if line.chars().count() > MAX_TEXT_CHARS {
            let cut: String = line.chars().take(MAX_TEXT_CHARS - 1).collect();
            format!("{cut}\u{2026}")
        } else {
            line.to_string()
        };
        Entry {
            ts: crate::model::now_ms(),
            repo: repo.to_string(),
            kind: kind.to_string(),
            task_id: task_id.map(str::to_string),
            text,
            cost_usd: None,
        }
    }
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("overseer.jsonl")
}

fn read_lines(data_dir: &Path) -> Vec<Entry> {
    std::fs::read_to_string(path(data_dir))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Appends `entry`, trims to the last `MAX_LINES` and rewrites the file atomically.
pub fn append(data_dir: &Path, entry: &Entry) -> std::io::Result<()> {
    let _guard = APPEND.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = read_lines(data_dir);
    entries.push(entry.clone());
    let skip = entries.len().saturating_sub(MAX_LINES);
    let mut out = String::new();
    for e in &entries[skip..] {
        out.push_str(&serde_json::to_string(e).map_err(std::io::Error::other)?);
        out.push('\n');
    }
    store::write_text_private(&path(data_dir), &out)
}

/// Newest first, optionally one repo, at most `limit` (default 200, max 2000).
pub fn read(data_dir: &Path, repo: Option<&str>, limit: Option<usize>) -> Vec<Entry> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LINES);
    read_lines(data_dir)
        .into_iter()
        .rev()
        .filter(|e| repo.is_none_or(|r| e.repo == r))
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("overseer-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn text_is_the_first_line_cut_to_200_chars() {
        let e = Entry::new(
            "answered",
            "/r",
            Some("t1"),
            &format!("\n{}\nsecond", "x".repeat(500)),
        );
        assert_eq!(e.text.chars().count(), MAX_TEXT_CHARS);
        assert!(!e.text.contains('\n'));
    }

    #[test]
    fn the_file_keeps_the_last_2000_lines() {
        let dir = temp();
        for i in 0..MAX_LINES + 5 {
            append(&dir, &Entry::new("woke", "/r", None, &format!("n{i}"))).unwrap();
        }
        let all = read(&dir, None, Some(MAX_LINES * 2));
        assert_eq!(all.len(), MAX_LINES);
        assert_eq!(all[0].text, format!("n{}", MAX_LINES + 4));
        assert_eq!(all.last().unwrap().text, "n5");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_filters_by_repo_newest_first_and_limits() {
        let dir = temp();
        for (repo, n) in [("/a", "1"), ("/b", "2"), ("/a", "3")] {
            append(&dir, &Entry::new("woke", repo, None, n)).unwrap();
        }
        let a = read(&dir, Some("/a"), None);
        assert_eq!(
            a.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            ["3", "1"]
        );
        assert_eq!(read(&dir, None, Some(1)).len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
