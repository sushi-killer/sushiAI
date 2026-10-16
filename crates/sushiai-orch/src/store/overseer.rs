use crate::store;
use crate::task::notes::overseer::Entry;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_LINES: usize = 2000;
const DEFAULT_LIMIT: usize = 200;
static APPEND: Mutex<()> = Mutex::new(());

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

/// Appends `entry` as one line. The file is rewritten only when it passes twice
/// `MAX_LINES`, down to the last `MAX_LINES`, so an append costs one write.
pub fn append(data_dir: &Path, entry: &Entry) -> std::io::Result<()> {
    let _guard = APPEND.lock().unwrap_or_else(|e| e.into_inner());
    append_line(data_dir, entry)
}

/// Appends a task's terminal entry once, even if loop finalization is retried.
pub fn append_task_once(data_dir: &Path, entry: &Entry) -> std::io::Result<bool> {
    let _guard = APPEND.lock().unwrap_or_else(|e| e.into_inner());
    if read_lines(data_dir)
        .iter()
        .any(|existing| existing.kind == entry.kind && existing.task_id == entry.task_id)
    {
        return Ok(false);
    }
    append_line(data_dir, entry)?;
    Ok(true)
}

/// Lines in each log file as this process last counted them. Another writer
/// only delays a trim; the trim itself reads the file again.
static LINES: Mutex<Vec<(PathBuf, usize)>> = Mutex::new(Vec::new());

fn append_line(data_dir: &Path, entry: &Entry) -> std::io::Result<()> {
    let file = path(data_dir);
    let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut counts = LINES.lock().unwrap_or_else(|e| e.into_inner());
    let at = match counts.iter().position(|(p, _)| *p == file) {
        Some(at) => at,
        None => {
            let lines = std::fs::read(&file)
                .map(|bytes| bytes.iter().filter(|b| **b == b'\n').count())
                .unwrap_or(0);
            counts.push((file.clone(), lines));
            counts.len() - 1
        }
    };
    store::files::append_text_private(&file, &line)?;
    counts[at].1 += 1;
    if counts[at].1 > 2 * MAX_LINES {
        let entries = read_lines(data_dir);
        write_entries(data_dir, &entries)?;
        counts[at].1 = entries.len().min(MAX_LINES);
    }
    Ok(())
}

fn write_entries(data_dir: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let skip = entries.len().saturating_sub(MAX_LINES);
    let mut out = String::new();
    for e in &entries[skip..] {
        out.push_str(&serde_json::to_string(e).map_err(std::io::Error::other)?);
        out.push('\n');
    }
    store::files::write_text_private(&path(data_dir), &out)
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
mod tests;
