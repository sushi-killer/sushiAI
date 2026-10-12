//! The overseer log: `<data>/overseer.jsonl` (mode 0600), one line per action the
//! orchestrator agent took when orchd woke it, across all repos. It keeps the
//! last `MAX_LINES` lines and is rewritten atomically on every append. An entry
//! holds one line of text, at most `MAX_TEXT_CHARS`: the agent's answer, the
//! first line of its reply, a pull request URL or a short error. It is the
//! agent's own words, so it can hold anything the agent wrote in that line.
//! The optional `detail` holds the text behind the line, at most
//! `MAX_DETAIL_CHARS`: the agent's words, plus for an answer the question
//! prose. A check's raw output tail is never stored (it can hold secrets).
//! The file stays 0600.
//!
//! The optional `decision` ties an entry to the owner decisions it told the
//! owner about (`asked_owner` and `digest`) or held back (`skipped` deferred to
//! the morning): one or more decision keys, separated by a newline. A key is
//! `<task id>|<question kind>|<sorted options joined by US (0x1f)>`, built by
//! `engine::owner_notice::decision_key`. An entry without `decision` (every
//! older line) never counts as a notice. `owner_notice::verdict` reads this
//! field from the whole log, so the "told once" memory survives a daemon
//! restart and spans overseer sessions.

use crate::model::{Question, QuestionKind};
use crate::store;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_LINES: usize = 2000;
pub const MAX_TEXT_CHARS: usize = 200;
pub const MAX_DETAIL_CHARS: usize = 1500;
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
    /// The full text behind `text` (older entries have none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Decision keys this entry told the owner about, newline separated (see
    /// the module docs); older entries have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max - 1).collect();
    format!("{kept}\u{2026}")
}

/// Whether a line reads like a shell command or source code, not a title.
fn looks_like_code(line: &str) -> bool {
    let first = line.split_whitespace().next().unwrap_or("");
    const COMMANDS: [&str; 14] = [
        "node", "npm", "npx", "cargo", "git", "python", "python3", "sh", "bash", "zsh", "$", "cd",
        "pnpm", "yarn",
    ];
    line.starts_with("```")
        || line.starts_with('$')
        || line.starts_with("./")
        || line.starts_with('/')
        || line.starts_with("--")
        || COMMANDS.contains(&first)
        || line.contains(" --")
        || line.contains("&&")
        || line.contains(" | ")
        || line.ends_with(['{', ';', '}'])
}

/// A readable one-line title for a question: its kind's label, or for a free
/// question its first line that is not a command.
pub fn question_title(kind: QuestionKind, text: &str) -> String {
    let label = match kind {
        QuestionKind::AttemptsFailing => "Attempts failing",
        QuestionKind::ReviewNoVerdict => "Review gave no verdict",
        QuestionKind::DependencyEnded => "Dependency ended",
        QuestionKind::Impossible => "Task impossible",
        QuestionKind::PreexistingFailure => "Base check failing",
        QuestionKind::Budget => "Task budget",
        QuestionKind::ProtectedPath => "Protected path",
        QuestionKind::Permission => "Permission",
        QuestionKind::ReviewDispute => "Review dispute",
        QuestionKind::PlanQuestion => "Plan question",
        QuestionKind::DailyBudget => "Daily budget",
        QuestionKind::HarnessMissing => "No agent CLI",
        QuestionKind::AgentQuestion => "",
    };
    if !label.is_empty() {
        return label.to_string();
    }
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !looks_like_code(l))
        .map_or_else(|| "Question".to_string(), |l| cut(l, 80))
}

/// The question without the command output it quotes: a base-check question
/// reads `<command>, <command> fail on base <sha>`, the commands read from
/// the question and the short sha from the text.
fn question_prose(question: &Question) -> String {
    const ON_BASE: &str = " on base ";
    if question.kind == QuestionKind::PreexistingFailure {
        if let Some(at) = question.text.find(ON_BASE) {
            // The header ends where the quoted output starts.
            let rest = &question.text[at + ON_BASE.len()..];
            let end = rest.find(':').unwrap_or(rest.len());
            let sha = rest[..end].trim();
            if question.commands.is_empty() {
                return format!("{}{ON_BASE}{sha}", &question.text[..at]);
            }
            let verb = if question.commands.len() == 1 {
                "fails"
            } else {
                "fail"
            };
            return format!("{} {verb}{ON_BASE}{sha}", question.commands.join(", "));
        }
    }
    question.text.clone()
}

impl Entry {
    /// An `answered` entry: "<title> -> <answer>", with the whole question and
    /// answer as the detail.
    pub fn answered(repo: &str, task_id: &str, question: Option<&Question>, answer: &str) -> Entry {
        let answer = answer.trim();
        let (kind, asked) = question.map_or((QuestionKind::AgentQuestion, ""), |q| {
            (q.kind, q.text.as_str())
        });
        let prose = question.map_or_else(String::new, question_prose);
        let title = question_title(kind, asked);
        let entry = Entry::new(
            "answered",
            repo,
            Some(task_id),
            &format!("{title} \u{2192} {answer}"),
        );
        let prose = prose.trim();
        entry.with_detail(&format!("{prose}\n\nAnswer: {answer}"))
    }

    /// Keeps the full `detail` (cut to `MAX_DETAIL_CHARS`) next to the line.
    pub fn with_detail(mut self, detail: &str) -> Entry {
        let detail = detail.trim();
        if !detail.is_empty() {
            self.detail = Some(cut(detail, MAX_DETAIL_CHARS));
        }
        self
    }

    /// Records the decision keys the entry is about (none: left unset).
    pub fn with_decision(mut self, keys: &[String]) -> Entry {
        if !keys.is_empty() {
            self.decision = Some(keys.join("\n"));
        }
        self
    }

    /// The decision keys of the entry; empty for an entry without any.
    pub fn decision_keys(&self) -> impl Iterator<Item = &str> {
        self.decision.as_deref().unwrap_or("").lines()
    }

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
            detail: None,
            decision: None,
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
    write_entries(data_dir, &entries)
}

/// Appends a task's terminal entry once, even if loop finalization is retried.
pub fn append_task_once(data_dir: &Path, entry: &Entry) -> std::io::Result<bool> {
    let _guard = APPEND.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = read_lines(data_dir);
    if entries
        .iter()
        .any(|existing| existing.kind == entry.kind && existing.task_id == entry.task_id)
    {
        return Ok(false);
    }
    entries.push(entry.clone());
    write_entries(data_dir, &entries)?;
    Ok(true)
}

fn write_entries(data_dir: &Path, entries: &[Entry]) -> std::io::Result<()> {
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
    fn answered_reads_as_a_title_and_keeps_the_whole_exchange() {
        let cmd = "node --input-type=module -e \"import assert from 'node:assert/strict'\"";
        let mut q = Question::new(
            format!("{cmd} already fails on base 0123456: TOKEN=abc123 leaked output"),
            vec![],
            QuestionKind::PreexistingFailure,
            crate::model::AskedBy::Verify,
        );
        q.commands = vec![cmd.to_string(), "npm test".to_string()];
        let e = Entry::answered("/r", "t1", Some(&q), " keep all ");
        assert_eq!(e.text, "Base check failing \u{2192} keep all");
        let d = e.detail.unwrap();
        assert!(d.starts_with("node --input-type") && d.ends_with("\n\nAnswer: keep all"));
        assert!(
            d.contains(&format!("{cmd}, npm test fail on base 0123456")) && !d.contains("TOKEN"),
            "{d}"
        );

        // A free question skips command lines and never starts with one.
        let q = Question::new(
            format!("{cmd}\nShould the retry limit be 3?"),
            vec![],
            QuestionKind::AgentQuestion,
            crate::model::AskedBy::Verify,
        );
        let e = Entry::answered("/r", "t1", Some(&q), "yes");
        assert_eq!(e.text, "Should the retry limit be 3? \u{2192} yes");
        let only_code = Question::new(
            "npm test",
            vec![],
            QuestionKind::AgentQuestion,
            crate::model::AskedBy::Verify,
        );
        assert_eq!(
            Entry::answered("/r", "t", Some(&only_code), "ok").text,
            "Question \u{2192} ok"
        );
    }

    #[test]
    fn detail_is_capped_and_old_entries_without_it_still_load() {
        let e = Entry::new("done", "/r", None, "a").with_detail(&"y".repeat(9000));
        assert_eq!(e.detail.as_ref().unwrap().chars().count(), MAX_DETAIL_CHARS);
        assert!(Entry::new("done", "/r", None, "a")
            .with_detail("  ")
            .detail
            .is_none());
        let old: Entry =
            serde_json::from_str(r#"{"ts":1,"repo":"/r","kind":"woke","text":"x"}"#).unwrap();
        assert_eq!(old.detail, None);
        assert!(!serde_json::to_string(&old).unwrap().contains("detail"));
    }

    #[test]
    fn an_old_line_without_a_decision_loads_and_has_no_keys() {
        let old: Entry = serde_json::from_str(
            r#"{"ts":1,"repo":"/r","kind":"asked_owner","taskId":"t","text":"x"}"#,
        )
        .unwrap();
        assert_eq!(old.decision, None);
        assert_eq!(old.decision_keys().count(), 0);
        assert!(!serde_json::to_string(&old).unwrap().contains("decision"));
        let keys = ["a|b|c".to_string(), "d|e|".to_string()];
        let e = Entry::new("asked_owner", "/r", None, "x").with_decision(&keys);
        let back: Entry = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.decision_keys().collect::<Vec<_>>(), ["a|b|c", "d|e|"]);
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
