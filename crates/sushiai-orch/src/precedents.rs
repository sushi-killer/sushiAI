//! The overseer's decision memory: `<data>/precedents.json` (mode 0600), one
//! record per distinct answer to a task question, keyed by the task's repo
//! root. Every answer source (owner, lead, overseer, chat, policy rule, waiting
//! default, judge) lands here through [`sync`], which `Store::save_task` calls:
//! a task's `questionHistory` is the one place every answer is recorded, so
//! the save is the one choke point. Entries older than the file itself are not
//! back-filled.
//!
//! A precedent holds the question kind, the facts that make a later question
//! "the same" (check commands, signals read from the attempt's check results,
//! changed files, goal), the answer, who gave it and a one-line why. A check's
//! output tail is never stored: it can hold secrets. Records merge on
//! repo + kind + failure signature + signals + commands (or, for a free
//! question, its normalized text and options) + answer + answerer; `count` and
//! `lastAt` grow. A repo keeps [`MAX_PER_REPO`], newest kept.
//!
//! When the owner gave the same answer twice for one key, one open [`Proposal`]
//! (a one-line rule) is stored for the owner to accept in the Orchestrator
//! panel. Nothing adopts a rule on its own, and a dismissed key is never
//! proposed again.

use crate::model::{AnsweredBy, AnsweredQuestion, Question, QuestionKind, Task};
use crate::{engine, overseer, store};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const MAX_PER_REPO: usize = 200;
/// Matching owner precedents shown to an overseer turn.
pub const MAX_PROMPT: usize = 5;
const MAX_FILES: usize = 20;
const MAX_TASKS_SEEN: usize = 500;
const MAX_PROPOSALS: usize = 50;
const GOAL_CHARS: usize = 200;
const QUESTION_CHARS: usize = 200;
const WHY_CHARS: usize = 160;
pub const RULE_CHARS: usize = 160;
/// How many of the newest owner records for a key must agree to auto-answer.
const AGREE_WINDOW: usize = 3;
/// A file created now still takes answers given just before.
const SINCE_GRACE_MS: i64 = 60_000;

static LOCK: Mutex<()> = Mutex::new(());
/// Per data dir and task, the newest `answeredAt` already synced.
static DONE: OnceLock<Mutex<BTreeMap<(PathBuf, String), i64>>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Precedent {
    pub id: String,
    pub kind: QuestionKind,
    /// What a later question must equal to match (see [`key_of`]).
    pub key: String,
    /// A short readable title of the question (never command output).
    pub title: String,
    pub signals: Vec<String>,
    pub commands: Vec<String>,
    pub files: Vec<String>,
    pub goal: String,
    /// The normalized question, kept only for a free question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    pub answer: String,
    /// `owner`, `lead`, `overseer`, `chat`, `policy` or `judge`.
    pub by: String,
    pub why: String,
    pub count: u32,
    pub first_at: i64,
    pub last_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalStatus {
    Open,
    Accepted,
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    pub key: String,
    pub kind: QuestionKind,
    pub answer: String,
    /// One line, at most [`RULE_CHARS`].
    pub rule: String,
    pub status: ProposalStatus,
    pub created_at: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoMemory {
    #[serde(default)]
    precedents: Vec<Precedent>,
    #[serde(default)]
    proposals: Vec<Proposal>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    /// When the file began; older answers are not back-filled.
    #[serde(default)]
    since: i64,
    /// Answers at or before this time were recorded already, or belong to a
    /// task dropped from `tasks`: a resave of such a task adds nothing.
    #[serde(default)]
    floor: i64,
    /// Per task, the newest `answeredAt` recorded, so a resave adds nothing.
    #[serde(default)]
    tasks: BTreeMap<String, i64>,
    #[serde(default)]
    repos: BTreeMap<String, RepoMemory>,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("precedents.json")
}

fn load(data_dir: &Path) -> File {
    store::read_json(&path(data_dir))
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn save(data_dir: &Path, file: &File) {
    if let Err(e) = store::write_json_private(&path(data_dir), file) {
        eprintln!("orch: saving precedents failed: {e}");
    }
}

fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn one_line(text: &str, max: usize) -> String {
    cut(&text.split_whitespace().collect::<Vec<_>>().join(" "), max)
}

fn by_name(by: AnsweredBy) -> &'static str {
    match by {
        AnsweredBy::Owner => "owner",
        AnsweredBy::Policy => "policy",
        AnsweredBy::Judge => "judge",
        AnsweredBy::Orchestrator => "chat",
        AnsweredBy::Lead => "lead",
        AnsweredBy::Overseer => "overseer",
    }
}

fn is_free(kind: QuestionKind) -> bool {
    matches!(
        kind,
        QuestionKind::AgentQuestion | QuestionKind::PlanQuestion
    )
}

/// A command or question text with its whitespace collapsed and lower-cased
/// for comparison.
fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// What a later question must equal to match this one: kind, failure
/// signature, signals and the sorted normalized commands; a free question
/// (agent or plan) adds its normalized text and sorted options in place of
/// the commands. The kind and option parts share their format with the
/// decision key (`engine::owner_notice::decision_key`).
pub fn key_of(
    kind: QuestionKind,
    failure_signature: Option<&str>,
    signals: &[String],
    commands: &[String],
    text: &str,
    options: &[String],
) -> String {
    let mut signals = signals.to_vec();
    signals.sort();
    let (what, opts) = if is_free(kind) {
        (normalized(text), engine::option_part(options))
    } else {
        let mut commands: Vec<String> = commands.iter().map(|c| normalized(c)).collect();
        commands.sort();
        (commands.join("\u{1f}"), String::new())
    };
    format!(
        "{}|{}|{}|{what}|{opts}",
        engine::kind_name(kind),
        failure_signature.unwrap_or(""),
        signals.join(",")
    )
}

/// The key of a question asked now on `task`.
pub fn question_key(question: &Question, task: &Task) -> String {
    key_of(
        question.kind,
        question.failure_signature.as_deref(),
        &signals_of(task),
        &question.commands,
        &question.text,
        &question.options,
    )
}

// -- signals -----------------------------------------------------------------

/// The names of failing tests a check's output tail lists.
fn failing_lines(tail: &str) -> Vec<&str> {
    tail.lines()
        .map(str::trim)
        .filter(|l| {
            (l.starts_with("test ") && l.ends_with("FAILED"))
                || l.starts_with("not ok ")
                || l.starts_with("FAIL ")
                || l.starts_with("--- FAIL")
        })
        .collect()
}

fn failing_tests(tail: &str) -> BTreeSet<String> {
    failing_lines(tail)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// File-looking words (`a/b.ts`) in the failing lines of a tail.
fn failing_paths(tail: &str) -> Vec<String> {
    const EXTS: [&str; 10] = [
        "rs", "ts", "tsx", "js", "cjs", "mjs", "py", "go", "rb", "java",
    ];
    failing_lines(tail)
        .into_iter()
        .flat_map(|l| l.split_whitespace())
        .map(|w| w.trim_matches(|c: char| ",;:()[]\"'".contains(c)))
        .filter(|w| {
            w.rsplit_once('.')
                .is_some_and(|(stem, ext)| !stem.is_empty() && EXTS.contains(&ext))
        })
        .map(|w| w.trim_start_matches("./").to_string())
        .collect()
}

/// Signals read from the check results of the task's latest attempt that ran
/// checks: `timeout`, `install` (the environment, not the code), `flaky` (a
/// different test fails in each run of one command) and `outside_diff`
/// (failing tests live in files the attempt did not change). Sorted.
pub fn signals_of(task: &Task) -> Vec<String> {
    let mut signals = BTreeSet::new();
    let Some(at) = task.attempts.iter().rposition(|a| !a.verify.is_empty()) else {
        return Vec::new();
    };
    let attempt = &task.attempts[at];
    for outcome in attempt.verify.iter().filter(|o| o.code != Some(0)) {
        if outcome.load_timeout || outcome.code.is_none() {
            signals.insert("timeout");
        }
        if engine::is_environment_result(outcome) {
            signals.insert("install");
        }
        let now = failing_tests(&outcome.tail);
        let before = task.attempts[..at]
            .iter()
            .rev()
            .flat_map(|a| a.verify.iter())
            .find(|o| o.command == outcome.command && o.code != Some(0))
            .map(|o| failing_tests(&o.tail));
        if let Some(before) = before {
            if !now.is_empty() && !before.is_empty() && now != before {
                signals.insert("flaky");
            }
        }
        let paths = failing_paths(&outcome.tail);
        if !paths.is_empty()
            && !paths.iter().any(|p| {
                attempt
                    .changed_files
                    .iter()
                    .any(|f| f.ends_with(p.as_str()) || p.ends_with(f.as_str()))
            })
        {
            signals.insert("outside_diff");
        }
    }
    signals.into_iter().map(str::to_string).collect()
}

// -- recording ---------------------------------------------------------------

fn build(task: &Task, entry: &AnsweredQuestion) -> Precedent {
    let signals = signals_of(task);
    let mut commands: Vec<String> = entry
        .commands
        .iter()
        .map(|c| c.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    commands.sort();
    let files = task
        .attempts
        .iter()
        .rev()
        .find(|a| !a.verify.is_empty())
        .map(|a| a.changed_files.iter().take(MAX_FILES).cloned().collect())
        .unwrap_or_default();
    let why = task
        .assumptions
        .iter()
        .rev()
        .find(|a| a.question == entry.question && a.answer == entry.answer)
        .map(|a| one_line(&a.evidence, WHY_CHARS))
        .unwrap_or_default();
    Precedent {
        id: uuid::Uuid::new_v4().to_string(),
        kind: entry.kind,
        key: key_of(
            entry.kind,
            entry.failure_signature.as_deref(),
            &signals,
            &commands,
            &entry.question,
            &entry.options,
        ),
        title: overseer::question_title(entry.kind, &entry.question),
        signals,
        commands,
        files,
        goal: one_line(&task.goal, GOAL_CHARS),
        question: is_free(entry.kind).then(|| one_line(&entry.question, QUESTION_CHARS)),
        answer: entry.answer.trim().to_string(),
        by: by_name(entry.answered_by).to_string(),
        why,
        count: 1,
        first_at: entry.answered_at,
        last_at: entry.answered_at,
    }
}

fn rule_text(p: &Precedent) -> String {
    let with = if p.commands.is_empty() {
        String::new()
    } else {
        format!(" ({})", p.commands.join(", "))
    };
    one_line(
        &format!(
            "When \"{}\"{with} comes up, answer \"{}\".",
            p.title, p.answer
        ),
        RULE_CHARS,
    )
}

fn merge(memory: &mut RepoMemory, new: Precedent) {
    if let Some(old) = memory
        .precedents
        .iter_mut()
        .find(|p| p.key == new.key && p.answer == new.answer && p.by == new.by)
    {
        old.count += 1;
        old.last_at = old.last_at.max(new.last_at);
        if !new.why.is_empty() {
            old.why = new.why;
        }
    } else {
        memory.precedents.push(new);
    }
    if memory.precedents.len() > MAX_PER_REPO {
        memory
            .precedents
            .sort_by_key(|p| std::cmp::Reverse(p.last_at));
        memory.precedents.truncate(MAX_PER_REPO);
    }
}

/// Opens a proposal for an owner answer given twice for one key, unless the
/// key has a proposal already (open, accepted or dismissed).
fn propose(memory: &mut RepoMemory, key: &str, answer: &str) {
    let Some(p) = memory
        .precedents
        .iter()
        .find(|p| p.key == key && p.answer == answer && p.by == "owner" && p.count >= 2)
    else {
        return;
    };
    if memory.proposals.iter().any(|q| q.key == key) {
        return;
    }
    let proposal = Proposal {
        id: uuid::Uuid::new_v4().to_string(),
        key: key.to_string(),
        kind: p.kind,
        answer: answer.to_string(),
        rule: rule_text(p),
        status: ProposalStatus::Open,
        created_at: p.last_at,
    };
    memory.proposals.push(proposal);
    if memory.proposals.len() > MAX_PROPOSALS {
        memory.proposals.remove(0);
    }
}

/// Forgets which answers this process synced, as a restart does.
#[cfg(test)]
pub fn forget_synced() {
    if let Some(done) = DONE.get() {
        done.lock().unwrap().clear();
    }
}

/// Records the answers `task` gained since its last sync. Cheap when there is
/// nothing new. Never fails a save: an IO error is logged.
pub fn sync(data_dir: &Path, task: &Task) {
    let Some(newest) = task.question_history.last().map(|e| e.answered_at) else {
        return;
    };
    let done = DONE.get_or_init(Mutex::default);
    let cache_key = (data_dir.to_path_buf(), task.id.clone());
    if done
        .lock()
        .unwrap()
        .get(&cache_key)
        .is_some_and(|d| *d >= newest)
    {
        return;
    }
    let _guard = LOCK.lock().unwrap();
    let mut file = load(data_dir);
    let fresh_file = file.since == 0;
    if fresh_file {
        file.since = crate::model::now_ms() - SINCE_GRACE_MS;
    }
    let seen = file.tasks.get(&task.id).copied().unwrap_or(0);
    let mut changed = fresh_file;
    let seen = seen.max(file.floor);
    for entry in task.question_history.iter().filter(|e| {
        e.answered_at > seen && e.answered_at >= file.since && !engine::owner_only(e.kind)
    }) {
        let new = build(task, entry);
        let (key, answer) = (new.key.clone(), new.answer.clone());
        let memory = file.repos.entry(task.repo.clone()).or_default();
        merge(memory, new);
        propose(memory, &key, &answer);
        changed = true;
    }
    if file.tasks.get(&task.id).copied().unwrap_or(0) < newest {
        file.tasks.insert(task.id.clone(), newest);
        if file.tasks.len() > MAX_TASKS_SEEN {
            let oldest = file
                .tasks
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(id, _)| id.clone());
            if let Some(id) = oldest {
                if let Some(at) = file.tasks.remove(&id) {
                    file.floor = file.floor.max(at);
                }
            }
        }
        changed = true;
    }
    if changed {
        save(data_dir, &file);
    }
    done.lock().unwrap().insert(cache_key, newest);
}

// -- reading and editing -------------------------------------------------------

/// A repo's precedents (newest first) and its open proposals.
pub fn list(data_dir: &Path, repo: &str) -> (Vec<Precedent>, Vec<Proposal>) {
    let _guard = LOCK.lock().unwrap();
    let mut memory = load(data_dir).repos.remove(repo).unwrap_or_default();
    memory
        .precedents
        .sort_by_key(|p| std::cmp::Reverse(p.last_at));
    let open = memory
        .proposals
        .into_iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .collect();
    (memory.precedents, open)
}

/// Removes one precedent; an unknown id is an error.
pub fn remove(data_dir: &Path, repo: &str, id: &str) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap();
    let mut file = load(data_dir);
    let memory = file.repos.entry(repo.to_string()).or_default();
    let before = memory.precedents.len();
    memory.precedents.retain(|p| p.id != id);
    if memory.precedents.len() == before {
        return Err(format!("unknown precedent: {id}"));
    }
    save(data_dir, &file);
    Ok(())
}

/// Closes an open proposal as accepted or dismissed and returns it; an unknown
/// or already closed id is an error.
pub fn close_proposal(
    data_dir: &Path,
    repo: &str,
    id: &str,
    accepted: bool,
) -> Result<Proposal, String> {
    let _guard = LOCK.lock().unwrap();
    let mut file = load(data_dir);
    let proposal = file
        .repos
        .entry(repo.to_string())
        .or_default()
        .proposals
        .iter_mut()
        .find(|p| p.id == id && p.status == ProposalStatus::Open)
        .ok_or_else(|| format!("unknown proposal: {id}"))?;
    proposal.status = if accepted {
        ProposalStatus::Accepted
    } else {
        ProposalStatus::Dismissed
    };
    let closed = proposal.clone();
    save(data_dir, &file);
    Ok(closed)
}

/// The owner's records for `key`, newest first.
fn owner_records<'a>(memory: &'a RepoMemory, key: &str) -> Vec<&'a Precedent> {
    let mut found: Vec<&Precedent> = memory
        .precedents
        .iter()
        .filter(|p| p.by == "owner" && p.key == key)
        .collect();
    found.sort_by_key(|p| std::cmp::Reverse(p.last_at));
    found
}

/// The one answer the owner's newest [`AGREE_WINDOW`] answers for `key` agree
/// on, with the time of the latest. Records fold equal answers into a count, so
/// the window is read newest record first (by `last_at`), each record
/// standing for its `count` answers. None without two owner answers, or when
/// the window holds two different answers.
pub fn owner_answer(data_dir: &Path, repo: &str, key: &str) -> Option<(String, i64)> {
    let _guard = LOCK.lock().unwrap();
    let file = load(data_dir);
    let found = owner_records(file.repos.get(repo)?, key);
    let newest = found.first()?;
    let mut window = 0usize;
    for p in &found {
        if p.answer != newest.answer && window < AGREE_WINDOW {
            return None;
        }
        window += (p.count.max(1) as usize).min(AGREE_WINDOW.saturating_sub(window));
        if window >= AGREE_WINDOW {
            break;
        }
    }
    (window >= 2).then(|| (newest.answer.clone(), newest.last_at))
}

/// Up to [`MAX_PROMPT`] owner precedents of `repo` for `key`, newest first.
pub fn owner_matches(data_dir: &Path, repo: &str, key: &str) -> Vec<Precedent> {
    let _guard = LOCK.lock().unwrap();
    let file = load(data_dir);
    file.repos
        .get(repo)
        .map(|m| {
            owner_records(m, key)
                .into_iter()
                .take(MAX_PROMPT)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}
