use crate::prompts::clip::truncate_chars;
use crate::task::questions::Assumption;
use crate::task::questions::WAIVED;
use crate::task::questions::WAIVER_BY;
use crate::task::state::Task;

/// First line of the body of a pull request for work no review read.
pub(crate) const UNREVIEWED_LINE: &str = "Unreviewed: no review route gave a verdict";

/// The PR body: goal, criteria as a checklist, then the report summary.
/// What the pull request and the decision say of a subtask that landed
/// unreviewed.
fn unreviewed_part_text(id: &str, title: &str) -> String {
    format!("subtask \"{title}\" ({id}) landed unreviewed: no review route gave a verdict")
}

/// The decision a parent records for a subtask that landed unreviewed.
pub(crate) fn unreviewed_part_line(child: &Task) -> String {
    format!(
        "Orchestrator: {}",
        unreviewed_part_text(&child.id, &child.title)
    )
}

const WAIVED_HEADING: &str = "## Waived base failures";

/// The section that tells the reviewer which failing base checks the task
/// was let through on (empty when none): put back by code after any drafted
/// body, so no turn can drop it.
pub(crate) fn waived_section(task: &Task) -> String {
    let waived: Vec<&Assumption> = task
        .assumptions
        .iter()
        .filter(|a| a.by == WAIVER_BY && a.answer == WAIVED && !a.overturned)
        .collect();
    if waived.is_empty() {
        return String::new();
    }
    let mut section = format!("\n{WAIVED_HEADING}\n\n");
    for a in waived {
        let command = a.commands.join(", ");
        section.push_str(&format!("- {command}: {}\n", a.evidence.trim()));
    }
    section
}

/// `body` without any waived-failures section of its own, so the one `code`
/// writes is the only one.
pub(crate) fn without_waived_section(body: &str) -> String {
    let mut kept = Vec::new();
    let mut skipping = false;
    for line in body.lines() {
        if line.trim_end() == WAIVED_HEADING {
            skipping = true;
        } else if skipping && line.starts_with("## ") {
            skipping = false;
        }
        if !skipping {
            kept.push(line);
        }
    }
    let mut text = kept.join("\n");
    if body.ends_with('\n') && !text.is_empty() {
        text.push('\n');
    }
    text
}

pub(crate) fn pr_body(task: &Task) -> String {
    let mark = if task.unreviewed { " " } else { "x" };
    let mut body = String::new();
    if task.unreviewed {
        body.push_str(&format!("{UNREVIEWED_LINE}\n\n"));
        for part in &task.loop_state.unreviewed_parts {
            body.push_str(&format!(
                "- Orchestrator: {}\n",
                unreviewed_part_text(&part.id, &part.title)
            ));
        }
        if !task.loop_state.unreviewed_parts.is_empty() {
            body.push('\n');
        }
    }
    body.push_str(&format!("{}\n", task.goal.trim()));
    if !task.criteria.is_empty() {
        body.push_str("\n## Criteria\n\n");
        for c in &task.criteria {
            body.push_str(&format!("- [{mark}] {}\n", c.trim()));
        }
    }
    if let Some(report) = task.report.as_deref().filter(|r| !r.trim().is_empty()) {
        body.push_str("\n## Report\n\n");
        body.push_str(report.trim());
        body.push('\n');
    }
    body.push_str(&waived_section(task));
    body
}

/// Longest pull request body sent to GitHub, in characters.
const MAX_BODY_CHARS: usize = 20_000;

/// Replaces each secret value with `${NAME}` and the home directory with `~`.
/// The body that is published: the unreviewed line and the waived-failures
/// section are put back by code, whatever the drafter or the body check wrote.
pub(crate) fn final_body(
    task: &Task,
    checked: String,
    secrets: &std::collections::HashMap<String, String>,
    home: Option<&str>,
    commits: &std::collections::BTreeMap<String, String>,
) -> String {
    let checked = super::validation::without_section(&checked);
    let source =
        if task.unreviewed && !checked.starts_with(crate::delivery::pr::body::UNREVIEWED_LINE) {
            format!(
                "{}\n\n{checked}",
                crate::delivery::pr::body::UNREVIEWED_LINE
            )
        } else {
            checked
        };
    let recorded = super::validation::section(task, commits) + &waived_section(task);
    let waived = redact_pr_text(&recorded, secrets, home);
    let source = crate::delivery::pr::body::without_waived_section(&source);
    truncate_chars(
        &redact_pr_text(&source, secrets, home),
        MAX_BODY_CHARS.saturating_sub(waived.chars().count()),
    ) + &waived
}

pub(crate) fn redact_pr_text(
    text: &str,
    secrets: &std::collections::HashMap<String, String>,
    home: Option<&str>,
) -> String {
    let mut named: Vec<_> = secrets.iter().filter(|(_, v)| !v.is_empty()).collect();
    named.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    let mut out = named
        .into_iter()
        .fold(text.to_string(), |t, (name, value)| {
            t.replace(value.as_str(), &format!("${{{name}}}"))
        });
    if let Some(home) = home
        .map(|h| h.trim_end_matches('/'))
        .filter(|h| !h.is_empty())
    {
        out = out.replace(home, "~");
    }
    out
}
