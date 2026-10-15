use crate::engine::plan_gate;
use crate::engine::state::App;
use crate::engine::verification::scoping::scoped_checks_for;
use crate::task::Task;
use std::{path::Path, sync::Arc};

pub(in crate::engine::planning) fn planner_brief(
    app: &Arc<App>,
    task: &Task,
    retry: u32,
    past_work: &str,
    resumed: bool,
) -> String {
    let request_text = task.request.clone().unwrap_or_default();
    let brief_text = match (retry == 0, task.parent.is_some()) {
        (true, false) => crate::engine::planning::prompt::build_plan_brief(
            &request_text,
            &task.variant(),
            past_work,
        ),
        (false, false) => crate::engine::planning::prompt::build_plan_retry_brief(
            &request_text,
            &task.variant(),
            past_work,
        ),
        (true, true) => crate::engine::planning::prompt::build_subtask_plan_brief(
            &request_text,
            &task.variant(),
            past_work,
        ),
        (false, true) => crate::engine::planning::prompt::build_subtask_plan_retry_brief(
            &request_text,
            &task.variant(),
            past_work,
        ),
    };
    let brief_text = crate::engine::attempts::context::with_block_before_report(
        &brief_text,
        &crate::engine::attempts::dependencies::landed_dependencies_block(
            task,
            &app.repo_tasks(&task.repo),
            None,
        ),
    );
    // The single replan carries the gate's findings in its brief; the
    // request itself is never edited.
    let brief_text = if task.plan_gate.replanned {
        crate::engine::attempts::context::with_block_before_report(
            &brief_text,
            &plan_gate::prompt::findings_block(&task.plan_gate.findings),
        )
    } else {
        brief_text
    };
    let tools = crate::engine::permissions::tools::task_tools(
        app,
        &app.settings.read().unwrap().clone(),
        task,
        Path::new(&task.worktree),
    );
    let brief_text = crate::engine::attempts::context::with_block_before_report(
        &brief_text,
        &crate::engine::permissions::brief::tools_block(&tools, true),
    );
    let brief_text = crate::engine::attempts::context::with_block_before_report(
        &brief_text,
        &crate::engine::planning::prompt::scoped_checks_block(
            &scoped_checks_for(&app.settings.read().unwrap(), task),
            &task.repo,
        ),
    );
    // A resumed session holds the request already: it continues.

    if resumed {
        crate::engine::planning::prompt::build_plan_continue_brief(
            &task.variant(),
            task.parent.is_none(),
        )
    } else {
        brief_text
    }
}
