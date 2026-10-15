use super::preflight::SelectedRun;
use super::{ExecutionStep, RunReceipt};
use crate::engine::attempts::best_of::outcome::BestOfExtra;
use crate::engine::attempts::resume::RunBrief;
use crate::engine::decision::implement_attempt_count;
use crate::engine::lifecycle::attempt_start;
use crate::engine::lifecycle::implementation_brief;
use crate::engine::permissions::tools::{task_tools, TaskTools};
use crate::engine::state::App;
use crate::policy::best_of::second_route;
use crate::policy::routing::Route;
use crate::task::state::Task;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) struct BriefedAttempt {
    pub(super) run: RunReceipt,
    pub(super) brief: RunBrief,
    pub(super) tools: TaskTools,
    pub(super) secondary: Option<Route>,
    pub(super) extra: BestOfExtra,
}

pub(super) fn prepare(
    app: &Arc<App>,
    task: &mut Task,
    selected: SelectedRun,
    changes: (String, String),
    parent: Option<&str>,
) -> Result<BriefedAttempt, ExecutionStep> {
    let worktree = PathBuf::from(&task.worktree);
    let mut full = implementation_brief::build(app, task, parent, (&changes.0, &changes.1));
    let tools = task_tools(app, &selected.settings, task, &worktree);
    for line in &tools.notes {
        if !task.decisions.contains(line) {
            task.record_decision(line.clone());
        }
    }
    full.text = crate::engine::attempts::context::with_block_before_report(
        &full.text,
        &crate::engine::permissions::brief::tools_block(&tools, false),
    );
    let mut extra = BestOfExtra::default();
    let secondary = match if selected.selection.carry_conflicts.is_empty() {
        second_route(
            task,
            &selected.settings,
            &selected.selection.route,
            implement_attempt_count(task) == 0,
        )
    } else {
        Ok(None)
    } {
        Ok(route) => route,
        Err(line) => {
            extra.decisions.push(line);
            None
        }
    };
    let brief = RunBrief::build(task, &selected.selection, full, secondary.is_some());
    let run = match RunReceipt::admit(app, task, selected) {
        Ok(Some(run)) => run,
        Ok(None) => return Err(ExecutionStep::Retry),
        Err(error) => {
            eprintln!("orch: starting attempt for {} failed: {error}", task.id);
            return Err(ExecutionStep::Stop);
        }
    };
    Ok(BriefedAttempt {
        run,
        brief,
        tools,
        secondary,
        extra,
    })
}

impl RunReceipt {
    fn admit(
        app: &Arc<App>,
        task: &mut Task,
        selected: SelectedRun,
    ) -> Result<Option<Self>, String> {
        let route = &selected.selection.route;
        let reason = format!("tier {} -> route {}", task.tier.as_str(), route.id);
        let Some(index) = attempt_start::start(app, task, route, &reason)? else {
            return Ok(None);
        };
        let attempt_n = task.attempts[index].n;
        let run_dir = app.store.run_dir(&task.id, attempt_n);
        let _ = std::fs::create_dir_all(&run_dir);
        Ok(Some(Self {
            settings: selected.settings,
            attempt_n,
            idx: index,
            route: selected.selection.route,
            worktree: PathBuf::from(&task.worktree),
            base_sha: task.base_sha.clone(),
            run_dir,
        }))
    }
}
