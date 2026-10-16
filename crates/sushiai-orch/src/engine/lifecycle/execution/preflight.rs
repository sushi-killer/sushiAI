use super::ExecutionStep;
use crate::clock::now_ms;
use crate::engine::advisor::{run_advisor_before_retry, AdviceTrigger};
use crate::engine::availability::wait_for_harness;
use crate::engine::cost::wait_while_over_budget;
use crate::engine::decision::last_review_repeated;
use crate::engine::ladder;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::lifecycle::selection::{route, Selection};
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::engine::verification::healing::answers::{heal_answers, AnswerHealing};
use crate::policy::settings::Settings;
use crate::task::state::Task;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) struct SelectedRun {
    pub(super) settings: Settings,
    pub(super) selection: Selection,
}

pub(super) async fn prepare(
    app: &Arc<App>,
    task: &mut Task,
    signals: &TaskSignals,
    permit: &mut Option<SlotPermit>,
) -> Result<(SelectedRun, (String, String)), ExecutionStep> {
    let task_id = task.id.clone();
    if let Err(end) = wait_for_harness(app, task, &mut QuestionSlot::new(signals, permit)).await {
        return Err(match end {
            WaitEnd::Changed => ExecutionStep::Retry,
            WaitEnd::Stopped => {
                *permit = None;
                app.finish_task_loop(&task_id);
                ExecutionStep::Stop
            }
        });
    }
    let settings = app.settings.read().unwrap().clone();
    let Some(selection) = route(app, task, &settings) else {
        return Err(ExecutionStep::Retry);
    };
    let selected = SelectedRun {
        settings,
        selection,
    };
    let changes = worktree_changes(task).await;
    if let Err(end) = advise(app, task, &selected, signals, permit).await {
        return Err(match end {
            WaitEnd::Changed => ExecutionStep::Retry,
            WaitEnd::Stopped => {
                *permit = None;
                app.finish_task_loop(&task_id);
                ExecutionStep::Stop
            }
        });
    }
    let attempt_n = crate::task::attempts::next_number(task);
    match heal_answers(app, task, attempt_n, &signals.cancel).await {
        Ok(AnswerHealing::Ready) => Ok((selected, changes)),
        Ok(AnswerHealing::Retry) => Err(ExecutionStep::Retry),
        Ok(AnswerHealing::Stopped) | Err(_) => {
            *permit = None;
            mark_stopped_if_not_already(app, &task_id).await;
            app.finish_task_loop(&task_id);
            Err(ExecutionStep::Stop)
        }
    }
}

async fn worktree_changes(task: &Task) -> (String, String) {
    let worktree = PathBuf::from(&task.worktree);
    let base = task.base_sha.clone();
    tokio::task::spawn_blocking(move || {
        (
            crate::git::changes::status_short(&worktree).unwrap_or_default(),
            crate::git::changes::diff_stat(&worktree, &base).unwrap_or_default(),
        )
    })
    .await
    .unwrap_or_default()
}

async fn advise(
    app: &Arc<App>,
    task: &mut Task,
    selected: &SelectedRun,
    signals: &TaskSignals,
    permit: &mut Option<SlotPermit>,
) -> Result<(), WaitEnd> {
    let repeated = last_review_repeated(task) || ladder::approach_pending(task);
    if task.variant().advisor || repeated {
        let trigger = if repeated {
            AdviceTrigger::RepeatedFinding
        } else {
            AdviceTrigger::Retry(&selected.selection.route)
        };
        if let Some(cost) =
            run_advisor_before_retry(app, task, &selected.settings, trigger, &signals.cancel).await
        {
            task.charge(cost);
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
            wait_while_over_budget(
                app,
                task,
                "implement",
                &mut QuestionSlot::new(signals, permit),
            )
            .await?;
        }
    }
    Ok(())
}
