use super::StaleEvidence;
use crate::engine::decision::failure;
use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::review::evidence::{evidence_scope, EvidenceScope};
use crate::engine::state::App;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::healing::evidence::capture_screenshots;
use crate::engine::verification::runner::CheckRunner;
use crate::harness::cancel::CancelToken;
use crate::task::{Failure, FailureKind, Task};
use std::sync::Arc;

pub(in crate::engine::lifecycle) enum EvidenceEnd {
    Failed(Failure),
    Stopped,
}

pub(in crate::engine::lifecycle) async fn validate(
    app: &Arc<App>,
    task: &mut Task,
    run: &RunReceipt,
    stale: Option<StaleEvidence>,
    cancel: &CancelToken,
) -> Result<(), EvidenceEnd> {
    let sandbox = app.task_sandbox(&task.repo, &run.worktree, &run.run_dir);
    let (id, base) = (task.id.clone(), task.base_sha.clone());
    capture_screenshots(
        CheckRunner::from_context(
            CheckContext::new(app, Some(&id)),
            &run.worktree,
            sandbox.as_ref(),
            &base,
            cancel,
        ),
        task,
        run.idx,
        &run.run_dir,
    )
    .await;
    if cancel.is_cancelled() {
        interrupt_attempt(app, task, run.idx);
        return Err(EvidenceEnd::Stopped);
    }
    let visual = task.visual_criteria_texts();
    let scope = evidence_scope(task);
    let missing = scope.missing(&task.attempts[run.idx].evidence);
    let absent = match scope {
        EvidenceScope::Named(_) => !missing.is_empty(),
        _ => task.attempts[run.idx].evidence.is_empty(),
    };
    if visual.is_empty() || !absent {
        return Ok(());
    }
    if let Some(command) = &task.brief_check.screenshot {
        return Err(EvidenceEnd::Failed(failure(
            FailureKind::Evidence,
            missing_detail(command, &visual, &missing, stale.as_ref()),
        )));
    }
    let note = format!(
        "Orchestrator: no screenshot command is set and no image was saved, so the visual criteria are judged without images: {}",
        visual.join("; ")
    );
    let follow = visual
        .iter()
        .map(|criterion| format!("Check by eye, no image evidence was captured: {criterion}"))
        .collect();
    if !task.decisions.contains(&note) {
        task.record_decision(note);
        task.add_review_follow_ups(follow);
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
    Ok(())
}

fn missing_detail(
    command: &str,
    visual: &[&str],
    missing: &[String],
    stale: Option<&StaleEvidence>,
) -> String {
    let criteria = visual
        .iter()
        .map(|criterion| format!("- {criterion}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut detail = if missing.is_empty() {
        format!(
            "The screenshot command `{command}` ran, but these criteria are visual and no image was saved under artifacts/ during this attempt:\n{criteria}\nSave one screenshot per criterion under artifacts/ (for example artifacts/<name>.png) and look at it before finishing."
        )
    } else {
        format!(
            "The screenshot command `{command}` ran, but these criteria are visual and the images they name were not saved during this attempt: {}\n{criteria}\nSave each named file and look at it before finishing; other images under artifacts/ are not evidence.",
            missing.join(", ")
        )
    };
    if let Some(stale) = stale {
        detail.push_str(&format!(
            "\nAttempt {}'s images were not reused because these files changed since: {}",
            stale.attempt,
            stale.paths.join(", ")
        ));
    }
    detail
}
