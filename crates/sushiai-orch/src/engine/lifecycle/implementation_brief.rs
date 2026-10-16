use crate::engine::decision::completed_implement_attempts;
use crate::engine::messages;
use crate::engine::state::App;
use crate::task::attempt::Stage;
use crate::task::state::Task;
use std::path::PathBuf;
use std::sync::Arc;

pub(in crate::engine) struct FullBrief {
    pub(in crate::engine) text: String,
    pub(in crate::engine) messages: String,
    pub(in crate::engine) advice: Option<String>,
}

pub(in crate::engine) fn build(
    app: &Arc<App>,
    task: &Task,
    parent_failure: Option<&str>,
    display: (&str, &str),
) -> FullBrief {
    let (status_short, diff_stat) = display;
    let task_id = &task.id;
    let worktree = PathBuf::from(&task.worktree);
    let brief_text = crate::engine::attempts::brief::build_brief(task, status_short, diff_stat);
    let brief_text = match &task.brief_check.conflict {
        Some(conflict) if completed_implement_attempts(task) == 0 => {
            crate::engine::attempts::context::with_block_before_report(
                &brief_text,
                &crate::engine::attempts::context::conflict_block(conflict),
            )
        }
        _ => brief_text,
    };
    let brief_text = crate::engine::attempts::context::with_block_before_report(
        &brief_text,
        &crate::engine::attempts::context::where_to_look_block(&worktree, &task.paths),
    );
    let brief_text = crate::engine::attempts::context::with_block_before_report(
        &brief_text,
        &crate::engine::attempts::dependencies::landed_dependencies_block(
            task,
            &app.repo_tasks(&task.repo),
            None,
        ),
    );
    let brief_text = if completed_implement_attempts(task) == 0 {
        crate::engine::attempts::context::with_block_before_report(
            &brief_text,
            &crate::engine::attempts::dependencies::relay_block(task, &app.repo_tasks(&task.repo)),
        )
    } else {
        brief_text
    };
    // A parent's first attempt gets what failed when it tried to finish.
    let brief_text = match &parent_failure {
        Some(failure)
            if completed_implement_attempts(task) == 0 && task.queue.carry_conflicts.is_empty() =>
        {
            crate::engine::attempts::context::with_block_before_report(
                &brief_text,
                &crate::engine::attempts::context::parent_failure_block(failure),
            )
        }
        _ => brief_text,
    };
    // The messages sent to this task since its last attempt are
    // delivered here, with the attempt about to start.
    let messages = messages::brief_block_for_task(app, task_id);
    let brief_text =
        crate::engine::attempts::context::with_block_before_report(&brief_text, &messages);
    let advice = task
        .attempts
        .iter()
        .rev()
        .find(|a| a.stage == Stage::Implement)
        .and_then(|a| a.advice.clone());
    let brief_text = match advice.as_deref() {
        Some(advice) => crate::engine::attempts::context::with_block_before_report(
            &brief_text,
            &crate::engine::attempts::advisor::advisor_block(advice),
        ),
        _ => brief_text,
    };

    FullBrief {
        text: brief_text,
        messages,
        advice,
    }
}

#[cfg(test)]
#[path = "implementation_brief/tests.rs"]
mod tests;
