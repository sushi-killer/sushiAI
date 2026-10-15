//! The overseer's state file: `<data>/overseer/<hash of repo>.md` (mode 0600).
//! Every overseer wake runs on a fresh harness session, so this small file
//! stands in for the owner's chat history. It has three sections:
//!
//! - `## Tasks`: the repo's open tasks (at most `MAX_TASKS`) and the wake's own
//!   task in any status, one line each;
//! - `## Plan`: title, goal and criteria of the task the wake is for;
//! - `## Decisions`: one line per finished wake, oldest first, at most
//!   `MAX_DECISIONS`.
//!
//! orchd rewrites Tasks and Plan when a wake starts and adds one Decisions line
//! when it ends. Check output is never written here.

use crate::store;
use crate::task::notes::wake::WakeDecision;
use crate::task::{Task, TaskStatus};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_TASKS: usize = 30;
pub const MAX_DECISIONS: usize = 50;
const MAX_OUTCOME_CHARS: usize = 200;
const DECISIONS_HEADING: &str = "\n## Decisions\n";

static STATE: Mutex<()> = Mutex::new(());

/// The state file of `repo`.
pub fn path(data_dir: &Path, repo: &str) -> PathBuf {
    data_dir
        .join("overseer")
        .join(format!("{}.md", crate::policy::keys::simple_hash(repo)))
}

fn status_label(status: TaskStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// `text` as one line: its first line, cut to `max` characters.
fn one_line(text: &str, max: usize) -> String {
    text.trim()
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(max)
        .collect()
}

fn task_line(task: &Task) -> String {
    format!(
        "- {} {} ({})",
        task.id,
        one_line(&task.title, 120),
        status_label(task.status)
    )
}

fn plan_block(task: &Task) -> String {
    let mut out = format!(
        "### {} ({})\n\nTitle: {}\n\nGoal: {}\n\nCriteria:\n",
        task.id,
        status_label(task.status),
        one_line(&task.title, 200),
        task.goal.trim()
    );
    if task.criteria.is_empty() {
        out.push_str("- none\n");
    }
    for criterion in &task.criteria {
        out.push_str(&format!("- {}\n", criterion.trim()));
    }
    out
}

/// The `- ` lines of the Decisions section of `text`.
fn decisions_of(text: &str) -> Vec<String> {
    let Some(at) = text.rfind(DECISIONS_HEADING) else {
        return Vec::new();
    };
    text[at + DECISIONS_HEADING.len()..]
        .lines()
        .filter(|l| l.starts_with("- "))
        .map(str::to_string)
        .collect()
}

/// The file text for these parts.
fn render(tasks: &[String], plan: &[String], decisions: &[String]) -> String {
    let mut out = String::from("## Tasks\n");
    if tasks.is_empty() {
        out.push_str("none\n");
    }
    for line in tasks {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("\n## Plan\n");
    if plan.is_empty() {
        out.push_str("none\n");
    }
    for block in plan {
        out.push('\n');
        out.push_str(block);
    }
    out.push_str(DECISIONS_HEADING);
    for line in decisions {
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn read_text(data_dir: &Path, repo: &str) -> String {
    std::fs::read_to_string(path(data_dir, repo)).unwrap_or_default()
}

fn write(data_dir: &Path, repo: &str, text: &str) {
    let file = path(data_dir, repo);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = store::files::write_text_private(&file, text);
}

/// Starts a wake: rewrites Tasks and Plan from `all` (every task of the host)
/// for `repo` and the tasks the wake is for (`wake_ids`, empty for none),
/// keeps Decisions and returns the whole file.
pub fn begin(data_dir: &Path, repo: &str, all: &[Task], wake_ids: &[String]) -> String {
    let _guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let own: Vec<&Task> = wake_ids
        .iter()
        .filter_map(|id| all.iter().find(|t| &t.id == id))
        .collect();
    let mut lines: Vec<String> = Vec::new();
    for task in &own {
        if !lines.contains(&task_line(task)) {
            lines.push(task_line(task));
        }
    }
    let open = |t: &&Task| {
        t.repo == repo
            && !t.archived
            && !matches!(
                t.status,
                TaskStatus::Done | TaskStatus::Failed | TaskStatus::Stopped
            )
            && !wake_ids.contains(&t.id)
    };
    lines.extend(all.iter().filter(open).map(task_line));
    lines.truncate(MAX_TASKS);
    let mut plan: Vec<String> = Vec::new();
    for task in &own {
        let block = plan_block(task);
        if !plan.contains(&block) {
            plan.push(block);
        }
    }
    let decisions = decisions_of(&read_text(data_dir, repo));
    let text = render(&lines, &plan, &decisions);
    write(data_dir, repo, &text);
    text
}

/// Adds the line for a wake that started and ended:
/// `- <UTC time> <reason> <task id or ->: <outcome>`. Keeps the newest
/// `MAX_DECISIONS` lines.
pub fn add_decision(data_dir: &Path, repo: &str, decision: &WakeDecision<'_>) {
    let _guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let read = read_text(data_dir, repo);
    let text = if read.is_empty() {
        render(&[], &[], &[])
    } else {
        read
    };
    let at = text.rfind(DECISIONS_HEADING).unwrap_or(text.len());
    let mut decisions = decisions_of(&text);
    let secs = decision.at.div_euclid(1000);
    let time = format!(
        "{}T{:02}:{:02}Z",
        crate::policy::calendar::day(decision.at),
        secs.rem_euclid(86_400) / 3600,
        secs.rem_euclid(3600) / 60
    );
    let outcome = one_line(decision.outcome, MAX_OUTCOME_CHARS);
    decisions.push(format!(
        "- {time} {} {}: {outcome}",
        decision.reason,
        decision.task_id.unwrap_or("-")
    ));
    let skip = decisions.len().saturating_sub(MAX_DECISIONS);
    let mut out = text[..at].trim_end_matches('\n').to_string();
    out.push_str("\n\n## Decisions\n");
    for line in &decisions[skip..] {
        out.push_str(line);
        out.push('\n');
    }
    write(data_dir, repo, &out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_has_tasks_plan_and_decisions_in_order() {
        let text = render(
            &["- t1 Title (running)".into()],
            &["### t1 (running)\n\nGoal: g\n".into()],
            &["- 2026-10-11T10:00Z done t1: ok".into()],
        );
        let tasks = text.find("## Tasks\n- t1").unwrap();
        let plan = text.find("## Plan\n").unwrap();
        let decisions = text.find("## Decisions\n- 2026").unwrap();
        assert!(tasks < plan && plan < decisions, "{text}");
        assert_eq!(decisions_of(&text).len(), 1);
        let empty = render(&[], &[], &[]);
        assert!(empty.contains("## Tasks\nnone\n"));
        assert!(empty.contains("## Plan\nnone\n"));
    }

    #[test]
    fn the_decisions_keep_the_newest_fifty_and_survive_a_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let repo = "/r";
        begin(dir.path(), repo, &[], &[]);
        for n in 0..55 {
            add_decision(
                dir.path(),
                repo,
                &WakeDecision {
                    at: 0,
                    reason: "done",
                    task_id: Some("t1"),
                    outcome: &format!("run {n}"),
                },
            );
        }
        let text = begin(dir.path(), repo, &[], &[]);
        let lines = decisions_of(&text);
        assert_eq!(lines.len(), MAX_DECISIONS);
        assert!(lines[0].ends_with("run 5"), "{}", lines[0]);
        assert!(lines[49].ends_with("run 54"), "{}", lines[49]);
        assert!(lines[0].starts_with("- 1970-01-01T00:00Z done t1: "));
    }

    #[test]
    fn a_long_multi_line_outcome_becomes_one_short_line() {
        let dir = tempfile::tempdir().unwrap();
        let long = format!("{}\nsecond line", "x".repeat(300));
        add_decision(
            dir.path(),
            "/r",
            &WakeDecision {
                at: 0,
                reason: "questions",
                task_id: None,
                outcome: &long,
            },
        );
        let lines = decisions_of(&read_text(dir.path(), "/r"));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains(" questions -: xxx"));
        assert!(!lines[0].contains("second"));
        assert!(lines[0].chars().count() < 260);
    }
}
