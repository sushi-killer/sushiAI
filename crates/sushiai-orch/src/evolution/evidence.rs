use super::clustering::Cluster;
use crate::prompts::evolution::proposal::ProposalExcerpt;
use crate::store::Store;
use crate::task::evolution::signals::{Signal, StoredSignal};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) fn evolution_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("evolution")
}

pub(super) fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}

pub(super) fn already_detected(dir: &Path, task_id: &str) -> bool {
    std::fs::read_to_string(dir.join("detected.jsonl"))
        .map(|text| text.lines().any(|l| l.trim().trim_matches('"') == task_id))
        .unwrap_or(false)
}

/// One `events.jsonl` line per element; a line that does not parse is `Null`
/// so an index plus one stays the file's line number.
pub(super) fn load_events(store: &Store, task_id: &str, n: u32) -> Option<Vec<serde_json::Value>> {
    let text = std::fs::read_to_string(store.run_dir(task_id, n).join("events.jsonl")).ok()?;
    Some(
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap_or(serde_json::Value::Null))
            .collect(),
    )
}

pub(super) fn read_signals(data_dir: &Path) -> Vec<StoredSignal> {
    let text =
        std::fs::read_to_string(evolution_dir(data_dir).join("signals.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            let ts = v.get("ts").and_then(|t| t.as_i64()).unwrap_or(0);
            Some(StoredSignal {
                signal: serde_json::from_value(v).ok()?,
                ts,
            })
        })
        .collect()
}

/// Signals of tasks that still exist and are not archived: what a proposer
/// may still learn from. Measuring adopted proposals reads every signal.
pub(super) fn read_live_signals(data_dir: &Path) -> Vec<StoredSignal> {
    let mut live: HashMap<String, bool> = HashMap::new();
    let mut is_live = |task_id: &str| {
        *live.entry(task_id.to_string()).or_insert_with(|| {
            std::fs::read_to_string(data_dir.join("tasks").join(task_id).join("task.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .is_some_and(|task| task.get("archived").and_then(|a| a.as_bool()) != Some(true))
        })
    };
    read_signals(data_dir)
        .into_iter()
        .filter(|s| is_live(&s.signal.task_id))
        .collect()
}

/// Up to `MAX_PROPOSAL_EXCERPT_LINES` raw lines around an excerpt ref, or
/// `None` when the ref does not resolve to a readable file under `data_dir`.
pub(super) fn read_excerpt(data_dir: &Path, signal: &Signal) -> Option<ProposalExcerpt> {
    let r = &signal.excerpt_ref;
    let relative = Path::new(&r.file);
    if !relative
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    let root = data_dir.canonicalize().ok()?;
    let path = root.join(relative).canonicalize().ok()?;
    if !path.starts_with(&root) {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let all: Vec<&str> = text.lines().collect();
    let max = crate::prompts::evolution::proposal::MAX_PROPOSAL_EXCERPT_LINES;
    let from = r.from_line.max(1);
    let to = r.to_line.max(from);
    let len = to - from + 1;
    let (start, count) = if len >= max {
        (from + (len - max) / 2, max)
    } else {
        (from.saturating_sub((max - len) / 2).max(1), max)
    };
    let lines: Vec<String> = all
        .iter()
        .skip(start - 1)
        .take(count)
        .map(|l| l.to_string())
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(ProposalExcerpt {
        task_id: signal.task_id.clone(),
        file: r.file.clone(),
        from_line: start,
        lines,
    })
}

/// Excerpts of the cluster's newest occurrences.
pub(super) fn excerpts(data_dir: &Path, cluster: &Cluster) -> Vec<ProposalExcerpt> {
    let mut newest: Vec<&StoredSignal> = cluster.occurrences.clone();
    newest.reverse();
    newest.sort_by_key(|o| std::cmp::Reverse(o.ts));
    newest
        .into_iter()
        .filter_map(|o| read_excerpt(data_dir, &o.signal))
        .take(crate::prompts::evolution::proposal::MAX_PROPOSAL_OCCURRENCES)
        .collect()
}

#[cfg(test)]
mod tests;
