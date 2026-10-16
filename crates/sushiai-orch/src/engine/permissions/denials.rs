use crate::clock::now_ms;
use crate::engine::decision::record_failure;
use crate::engine::leases::relative_to_worktree;
use crate::engine::permissions::answers::already_answered;
use crate::engine::permissions::answers::apply_answer;
use crate::engine::permissions::answers::permission_question;
use crate::engine::state::App;
use crate::harness;
use crate::policy::permissions::normalized;
use crate::policy::permissions::question_text;
use crate::policy::permissions::Subject;
use crate::policy::permissions::EDIT_TOOLS;
use crate::policy::permissions::WEB_TOOLS;
use crate::task::FailureKind;
use crate::task::Task;
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::path::Path;

/// The calls a finished run was refused that orchd never decided: the tool or
/// path, so they can be asked about once instead of retried.
/// A write the run was refused outside its worktree, into a temp directory:
/// scratch work that needs no owner and loses nothing.
fn scratch_file<'a>(worktree: &Path, denial: &'a harness::PermissionDenial) -> Option<&'a str> {
    let file = ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|k| denial.input.get(k).and_then(|v| v.as_str()))?;
    // The system temp dir and the POSIX ones, each also by its resolved path
    // (a symlinked /tmp resolves elsewhere on some systems).
    let roots = [std::env::temp_dir(), "/tmp".into(), "/var/tmp".into()];
    let in_temp = roots.iter().any(|root| {
        Path::new(file).starts_with(root)
            || root
                .canonicalize()
                .is_ok_and(|real| Path::new(file).starts_with(real))
    });
    (EDIT_TOOLS.contains(&denial.tool.as_str())
        && Path::new(file).is_absolute()
        && relative_to_worktree(worktree, file).is_none()
        && in_temp)
        .then_some(file)
}

/// The decision lines for scratch writes the run was refused: recorded for
/// the next attempt, which works inside its worktree instead.
pub(in crate::engine) fn scratch_lines(
    worktree: &Path,
    outcome: &harness::RunOutcome,
) -> Vec<String> {
    let files: BTreeSet<&str> = outcome
        .permission_denials
        .iter()
        .filter_map(|d| scratch_file(worktree, d))
        .collect();
    files
        .into_iter()
        .map(|file| {
            format!(
                "Orchestrator: a write to {file} outside the worktree was refused; keep scratch files inside the worktree and out of the commit"
            )
        })
        .collect()
}

pub(in crate::engine) fn unhandled_denials(
    handled: &BTreeSet<String>,
    worktree: &Path,
    outcome: &harness::RunOutcome,
    protected: &[String],
) -> Vec<Subject> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for denial in &outcome.permission_denials {
        let tool = denial.tool.as_str();
        if scratch_file(worktree, denial).is_some() {
            continue;
        }
        let subject = if EDIT_TOOLS.contains(&tool) {
            let input = Some(&denial.input);
            let file = ["file_path", "notebook_path", "path"]
                .iter()
                .find_map(|k| input.and_then(|i| i.get(k)).and_then(|v| v.as_str()));
            match file {
                Some(f) => Subject::Path(
                    relative_to_worktree(worktree, f)
                        .map(|rel| normalized(&rel))
                        .unwrap_or_else(|| f.to_string()),
                ),
                None => continue,
            }
        } else if (tool.starts_with("mcp__") && !tool.starts_with("mcp__sushiai-messages__"))
            || WEB_TOOLS.contains(&tool)
        {
            Subject::Tool(tool.to_string())
        } else {
            continue;
        };
        // A planner test or a check-run file is restored by orchd, never asked
        // about: the owner has nothing to allow.
        let is_protected = matches!(&subject, Subject::Path(rel) if protected.contains(rel));
        if is_protected || handled.contains(subject.name()) || !seen.insert(subject.rule()) {
            continue;
        }
        out.push(subject);
    }
    out
}

/// Records the refusal and raises its one owner question. A repeated,
/// already answered refusal ends the task instead of starting another attempt.
pub(in crate::engine) fn prepare_denials(
    app: &App,
    task: &mut Task,
    idx: usize,
    subjects: &[Subject],
) -> bool {
    let names = question_text(subjects);
    let names = names
        .trim_start_matches("Allow ")
        .trim_end_matches(" for this task?")
        .to_string();
    record_failure(
        task,
        idx,
        FailureKind::Blocked,
        format!("The run was refused {names}; no retry can fix that."),
    );
    if already_answered(task, subjects) {
        task.record_decision(format!(
            "Permission: {names} was refused again after the owner's answer; stopping instead of another attempt on the same cause"
        ));
        task.mark_failed();
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        return false;
    }
    task.ask(permission_question(
        subjects,
        "The run tried this and was refused; nothing was done. Asking once instead of starting another attempt.",
    ), now_ms());
    true
}

pub(in crate::engine) fn finish_denials(
    app: &App,
    task: &mut Task,
    subjects: &[Subject],
    answer: &str,
) {
    apply_answer(app, task, subjects, answer);
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
}
