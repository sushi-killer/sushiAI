use crate::prompts::text::{truncate_chars, untrusted_block};
use crate::prompts::{brief_text, fill};
use crate::task::attempt::{AttemptStatus, Stage};
use crate::task::state::{Task, TaskStatus};

const LANDED_ENTRY_MAX: usize = 1500;

const LANDED_DECISIONS_MAX: usize = 5;

const LANDED_FILES_MAX: usize = 20;

const LANDED_LABEL: &str = "Result of this dependency";

fn landed_entry(dep: &Task) -> String {
    let passed = dep
        .attempts
        .iter()
        .rev()
        .find(|a| a.stage == Stage::Implement && a.status == AttemptStatus::Passed);
    let mut text = format!("Title: {}", dep.title);
    if let Some(summary) = passed.and_then(|a| a.summary.as_deref()) {
        text.push_str(&format!("\nSummary: {summary}"));
    }
    let decisions: Vec<&String> = dep
        .decisions
        .iter()
        .filter(|d| d.starts_with("Agent:") || d.starts_with("Owner:") || d.starts_with("Lead:"))
        .collect();
    let skip = decisions.len().saturating_sub(LANDED_DECISIONS_MAX);
    if !decisions[skip..].is_empty() {
        text.push_str("\nDecisions:");
        for d in &decisions[skip..] {
            text.push_str(&format!("\n{d}"));
        }
    }
    if let Some(handoff) = passed.and_then(|a| a.handoff.as_deref()) {
        if !handoff.trim().is_empty() {
            text.push_str(&format!("\nHandoff: {handoff}"));
        }
    }
    if let Some(a) = passed.filter(|a| !a.changed_files.is_empty()) {
        text.push_str("\nChanged files:");
        for f in a.changed_files.iter().take(LANDED_FILES_MAX) {
            text.push_str(&format!("\n{f}"));
        }
        if a.changed_files.len() > LANDED_FILES_MAX {
            text.push_str(&format!(
                "\n+{} more",
                a.changed_files.len() - LANDED_FILES_MAX
            ));
        }
    }
    let heading = format!("### {}\n", dep.id);
    let overhead = heading.len() + untrusted_block(LANDED_LABEL, "").len();
    // The cap is measured in bytes; `truncate_chars` cuts at a char boundary and
    // appends a 3-byte ellipsis, so a byte budget also bounds the character count.
    let room = LANDED_ENTRY_MAX.saturating_sub(overhead);
    let text = if text.len() > room {
        truncate_chars(&text, room.saturating_sub(3))
    } else {
        text
    };
    let mut out = heading;
    out.push_str(&untrusted_block(LANDED_LABEL, &text));
    out.push('\n');
    out
}

/// What the dependencies of `task` that are done delivered: title, summary,
/// decisions, handoff and changed files of each, so a dependent starts from
/// its results instead of rediscovering them. With `landed_after`, only
/// dependencies whose last attempt ended after that time (a resume delta
/// only needs what landed since the previous attempt). Empty when nothing
/// qualifies. Every agent-written line sits in an untrusted block; the text
/// is clipped before it is wrapped so the closing tag stays whole.
pub fn landed_dependencies_block(task: &Task, all: &[Task], landed_after: Option<i64>) -> String {
    let mut entries = String::new();
    for id in &task.depends_on {
        let Some(dep) = all.iter().find(|t| &t.id == id) else {
            continue;
        };
        if dep.status != TaskStatus::Done {
            continue;
        }
        let ended = dep.attempts.last().and_then(|a| a.ended_at);
        if landed_after.is_some_and(|after| ended.is_none_or(|e| e <= after)) {
            continue;
        }
        entries.push_str(&landed_entry(dep));
    }
    if entries.is_empty() {
        return String::new();
    }
    format!("{}\n\n{entries}", brief_text("landed_dependencies"))
}

/// The previous link of a relay: its branch is what this task continues, so
/// its handoff comes first, whether or not it is marked done.
pub fn relay_block(task: &Task, all: &[Task]) -> String {
    let Some(prev) = task
        .queue
        .relay_of
        .as_deref()
        .and_then(|id| all.iter().find(|t| t.id == id))
    else {
        return String::new();
    };
    format!(
        "{}\n\n{}",
        fill(brief_text("relay"), &[("title", &prev.title)]),
        landed_entry(prev)
    )
}

#[cfg(test)]
#[path = "tests/dependencies.rs"]
mod tests;
