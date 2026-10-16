use super::InspectionEnd;
use crate::engine::lifecycle::changes::record_changed_files;
use crate::engine::lifecycle::native_result::ObservedRun;
use crate::engine::permissions::denials::{
    finish_denials, prepare_denials, scratch_lines, unhandled_denials,
};
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::state::App;
use crate::task::Task;
use std::sync::Arc;

pub(super) async fn settle(
    app: &Arc<App>,
    task: &mut Task,
    observed: &ObservedRun,
    slot: &mut QuestionSlot<'_>,
) -> Result<(), InspectionEnd> {
    let protected: Vec<String> = task
        .planner_tests
        .protected
        .iter()
        .flat_map(|files| files.keys().cloned())
        .collect();
    for line in scratch_lines(&observed.run.worktree, &observed.outcome) {
        if !task.decisions.contains(&line) {
            task.record_decision(line);
        }
    }
    let refused = unhandled_denials(
        &observed.handled,
        &observed.run.worktree,
        &observed.outcome,
        &protected,
    );
    if refused.is_empty() {
        return Ok(());
    }
    record_changed_files(task, &observed.run).await;
    if !prepare_denials(app, task, observed.run.idx, &refused) {
        return Err(InspectionEnd::Stopped);
    }
    let answer = slot.answer(app, task).await?;
    finish_denials(app, task, &refused, &answer);
    Err(InspectionEnd::Retry(0))
}
