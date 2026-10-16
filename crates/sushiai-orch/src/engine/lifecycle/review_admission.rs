use crate::clock::now_ms;
use crate::engine::decision::failure;
use crate::engine::lifecycle::execution::RunReceipt;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::engine::verification::healing::answers::{heal_answers, AnswerHealing};
use crate::policy::paths::matches_any_protected;
use crate::task::{
    attempts, AskedBy, Failure, FailureKind, Question, QuestionKind, Task, TaskStatus,
};
use std::sync::Arc;

pub(super) enum AdmissionEnd {
    Changed,
    Stopped,
    Failed(Failure),
}

pub(super) async fn prepare(
    app: &Arc<App>,
    task: &mut Task,
    run: &RunReceipt,
    signals: &TaskSignals,
    permit: &mut Option<SlotPermit>,
) -> Result<(), AdmissionEnd> {
    protected(app, task, run, &mut QuestionSlot::new(signals, permit)).await?;
    if let Err(error) =
        apply_pending_amendment(app, task, &signals.pending_amend, &signals.cancel).await
    {
        eprintln!("orch: applying amendment to {} failed: {error}", task.id);
        return Err(AdmissionEnd::Stopped);
    }
    match heal_answers(app, task, run.attempt_n, &signals.cancel).await {
        Ok(AnswerHealing::Ready) => Ok(()),
        Ok(AnswerHealing::Retry) => Err(AdmissionEnd::Changed),
        Ok(AnswerHealing::Stopped) | Err(_) => {
            interrupt_attempt(app, task, run.idx);
            Err(AdmissionEnd::Stopped)
        }
    }
}

async fn protected(
    app: &Arc<App>,
    task: &mut Task,
    run: &RunReceipt,
    slot: &mut QuestionSlot<'_>,
) -> Result<(), AdmissionEnd> {
    let Some(path) = task.attempts[run.idx]
        .changed_files
        .iter()
        .find(|file| matches_any_protected(file, &run.settings.protected_paths))
        .cloned()
    else {
        return Ok(());
    };
    attempts::end(task, run.idx, attempts::AttemptEnd::Blocked, now_ms());
    task.ask(
        Question::new(
            format!("Change touches protected path {path}: approve or reject?"),
            vec!["approve".into(), "reject".into()],
            QuestionKind::ProtectedPath,
            AskedBy::Implement,
            now_ms(),
        ),
        task.updated_at,
    );
    task.resume_phase(TaskStatus::Waiting, task.updated_at);
    task.touch(now_ms());
    match slot.answer(app, task).await {
        Err(WaitEnd::Changed) => Err(AdmissionEnd::Changed),
        Err(WaitEnd::Stopped) => Err(AdmissionEnd::Stopped),
        Ok(answer) if answer == "approve" => Ok(()),
        Ok(_) => Err(AdmissionEnd::Failed(failure(
            FailureKind::Protected,
            format!("Owner rejected change to protected path {path}"),
        ))),
    }
}

#[cfg(test)]
mod tests;
