use crate::prompts::text::untrusted_block;
use crate::prompts::{brief_text, fill};
use crate::task::messages::{Message, MessageKind};

/// Added to the brief of the attempt after one whose edits to protected files
/// were discarded.
pub fn discarded_block(files: &[String]) -> String {
    let mut out = fill(brief_text("discarded"), &[("files", &files.join(", "))]);
    out.push_str("\n\n");
    out
}

/// How an implement attempt reaches the other agents, and the messages
/// delivered to it with this attempt, each labelled with its sender. Every
/// message is another agent's text, so it goes in as data.
pub fn coordination_block(task_id: &str, inbox: &[(String, Message)]) -> String {
    let mut out = fill(brief_text("coordination"), &[("task_id", task_id)]);
    out.push_str("\n\n");
    if !inbox.is_empty() {
        out.push_str("### Messages for you\n\n");
        for (sender, message) in inbox {
            let kind = match message.kind {
                MessageKind::Reply => "reply",
                MessageKind::Question => "question",
                MessageKind::Message => "message",
            };
            out.push_str(&untrusted_block(
                &format!("{kind} {} from {sender}", message.id),
                &message.text,
            ));
            out.push('\n');
        }
    }
    out
}

/// Tells the implementer that a check found two of its requirements in
/// conflict: report it rather than trade one for the other.
pub fn conflict_block(conflict: &str) -> String {
    let mut out = fill(brief_text("conflict"), &[("conflict", conflict)]);
    out.push_str("\n\n");
    out
}

/// Names the work a carry left conflicted for the task's ordinary attempt.
pub fn carry_conflict_block(files: &[String], task_id: &str) -> String {
    let mut out = fill(
        brief_text("carry_conflict"),
        &[
            ("files", &files.join(", ")),
            ("wip", &crate::delivery::git::carry::wip_ref(task_id)),
        ],
    );
    out.push_str("\n\n");
    out
}

/// The measured failure behind the PR's designated ordinary repair step.
pub fn pr_check_repair_block(repair: &crate::task::delivery::PrCheckRepair) -> String {
    let mut out = fill(
        brief_text("pr_check_repair"),
        &[(
            "failure",
            &untrusted_block(
                "The failed check, named tests and output",
                &repair.failure.detail,
            ),
        )],
    );
    out.push_str("\n\n");
    out
}

/// Tells the implementer of a parent task why finishing the parent failed
/// after every subtask landed on its branch: a check, or landing on the base.
pub fn parent_failure_block(detail: &str) -> String {
    let mut out = fill(
        brief_text("parent_failure"),
        &[("failure", &untrusted_block("The failure", detail))],
    );
    out.push_str("\n\n");
    out
}

const WHERE_MAX_PATHS: usize = 8;

const WHERE_MAX_LINES: usize = 40;

const WHERE_MAX_EXCERPT: usize = 2048;

const WHERE_MAX_DIR_ENTRIES: usize = 20;

const WHERE_MAX_BLOCK: usize = 8 * 1024;

/// The head of a text file: at most 40 lines and 2 KB, cut at a char
/// boundary. `None` for a binary file or one that cannot be read.
fn file_head(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(WHERE_MAX_EXCERPT as u64 + 4)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.contains(&0) {
        return None;
    }
    let text = match String::from_utf8(buf) {
        Ok(text) => text,
        // The read may cut a multi-byte char at the end; anything else is binary.
        Err(e) if e.utf8_error().error_len().is_none() => {
            let valid = e.utf8_error().valid_up_to();
            String::from_utf8(e.into_bytes()[..valid].to_vec()).ok()?
        }
        Err(_) => return None,
    };
    let mut end = text.len().min(WHERE_MAX_EXCERPT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = &text[..end];
    let cut = head
        .match_indices('\n')
        .nth(WHERE_MAX_LINES - 1)
        .map_or(head.len(), |(i, _)| i);
    Some(head[..cut].to_string())
}

/// `## Where to look`: the planner's paths for this task, so the first
/// attempt starts from them instead of exploring. A file gets the head of its
/// text, a directory its entry names. A path that is absolute, has `..`, is
/// missing, binary, or resolves outside the worktree is left out.
pub fn where_to_look_block(worktree: &std::path::Path, paths: &[String]) -> String {
    let Ok(root) = worktree.canonicalize() else {
        return String::new();
    };
    let heading = format!("{}\n\n", brief_text("where_to_look"));
    let mut out = String::new();
    for rel in paths.iter().take(WHERE_MAX_PATHS) {
        let p = std::path::Path::new(rel);
        if rel.is_empty()
            || p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue;
        }
        let Ok(real) = root.join(p).canonicalize() else {
            continue;
        };
        if !real.starts_with(&root) {
            continue;
        }
        let entry = if real.is_dir() {
            let Ok(read) = std::fs::read_dir(&real) else {
                continue;
            };
            let mut names: Vec<String> = read
                .filter_map(|e| e.ok())
                .map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if e.path().is_dir() {
                        format!("{name}/")
                    } else {
                        name
                    }
                })
                .collect();
            names.sort();
            names.truncate(WHERE_MAX_DIR_ENTRIES);
            format!("- `{rel}/`: {}\n\n", names.join(", "))
        } else {
            let Some(head) = file_head(&real) else {
                continue;
            };
            format!(
                "- `{rel}`\n{}\n",
                untrusted_block("Start of the file", &head)
            )
        };
        if heading.len() + out.len() + entry.len() > WHERE_MAX_BLOCK {
            break;
        }
        out.push_str(&entry);
    }
    if out.is_empty() {
        return String::new();
    }
    format!("{heading}{out}")
}

/// Puts `block` in front of the brief's report format, which [`build_brief`] ends with.
pub fn with_block_before_report(brief: &str, block: &str) -> String {
    match brief.rfind("## Report format") {
        Some(at) => format!("{}{block}{}", &brief[..at], &brief[at..]),
        None => format!("{brief}\n{block}"),
    }
}

#[cfg(test)]
#[path = "tests/context.rs"]
mod tests;
