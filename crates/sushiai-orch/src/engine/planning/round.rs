use crate::clock::now_ms;
use crate::engine::availability::wait_for_harness;
use crate::engine::cost::wait_while_over_budget;
use crate::engine::lines::variant_route_line;
use crate::engine::planning::records::write_planner_run_files;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::task::Stage;
use crate::task::TaskStatus;
use std::path::PathBuf;
use std::sync::Arc;

pub(in crate::engine) struct PlannerRound {
    pub(in crate::engine::planning) app: Arc<App>,
    pub(in crate::engine::planning) task: crate::task::Task,
    pub(in crate::engine::planning) route: crate::policy::routing::Route,
    pub(in crate::engine::planning) settings: crate::policy::settings::Settings,
    pub(in crate::engine::planning) index: usize,
    pub(in crate::engine::planning) run_dir: PathBuf,
    pub(in crate::engine::planning) mcp_path: PathBuf,
    pub(in crate::engine::planning) settings_path: PathBuf,
    pub(in crate::engine::planning) key_path: PathBuf,
    pub(in crate::engine::planning) past_work: String,
}

impl PlannerRound {
    pub(in crate::engine::planning) fn new(
        app: &Arc<App>,
        mut task: crate::task::Task,
        route: crate::policy::routing::Route,
        settings: crate::policy::settings::Settings,
    ) -> Self {
        let attempt_n = task.attempts.iter().map(|a| a.n).max().unwrap_or(0) + 1;
        let run_dir = app.store.run_dir(&task.id, attempt_n).join("plan");
        let _ = std::fs::create_dir_all(&run_dir);

        let idx =
            crate::task::attempts::start(&mut task, Stage::Plan, &route, "drafting", now_ms());
        app.save_task_logged(&task);
        app.broadcast_task(&task);

        let (mcp_path, settings_path, key_path) =
            write_planner_run_files(app, &route, &settings, &run_dir);

        // Once per round: the retry brief carries the same section.
        let past_work = {
            let all_tasks = app.store.list_tasks().unwrap_or_default();
            let notes = app
                .store
                .load_repo_notes()
                .unwrap_or_default()
                .remove(&task.repo)
                .unwrap_or_default();
            crate::engine::past_work::past_work_section(&task, &notes, &all_tasks)
        };

        Self {
            app: app.clone(),
            task,
            route,
            settings,
            index: idx,
            run_dir,
            mcp_path,
            settings_path,
            key_path,
            past_work,
        }
    }
}
pub(in crate::engine::planning) async fn prepare_round(
    app: &Arc<App>,
    task_id: &str,
    choice: &(String, String),
    signals: &crate::engine::run_control::TaskSignals,
    permit: &mut Option<SlotPermit>,
) -> Option<PlannerRound> {
    let cancel = &signals.cancel;
    let pending_answer = &signals.pending_answer;
    if cancel.is_cancelled() {
        return None;
    }
    // The very first round already holds the slot `run_task_loop`
    // acquired before calling in; a round that starts fresh right after
    // releasing one to park on a clarifying question needs its own, or
    // it would run the next harness session with no permit at all.
    if permit.is_none() {
        let acquired = tokio::select! {
            _ = cancel.cancelled() => None,
            p = app.slots.clone().acquire_owned() => p.ok(),
        };
        match acquired {
            Some(p) => *permit = Some(p),
            None => return None,
        }
    }

    let mut task = match app.store.load_task(task_id) {
        Ok(Some(t)) => t,
        _ => return None,
    };
    crate::engine::verification::base::amendments::apply_pending_amendment(
        app,
        &mut task,
        &signals.pending_amend,
        cancel,
    )
    .await;
    if !wait_while_over_budget(
        app,
        task_id,
        &mut task,
        "plan",
        TaskStatus::Drafting,
        pending_answer,
        cancel,
        permit,
    )
    .await
    {
        return None;
    }

    if !wait_for_harness(
        app,
        task_id,
        &mut task,
        TaskStatus::Drafting,
        pending_answer,
        cancel,
        permit,
    )
    .await
    {
        return None;
    }
    let settings = app.settings.read().unwrap().clone();
    let route = select_route(app, &mut task, &settings, choice)?;
    Some(PlannerRound::new(app, task, route, settings))
}

fn select_route(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    settings: &crate::policy::settings::Settings,
    choice: &(String, String),
) -> Option<crate::policy::routing::Route> {
    let (planner, decision) = choice;
    let variant = task.variant();
    if !task.decisions.contains(decision) {
        task.record_decision(decision.clone());
    }
    let Some(route) = settings.routes.iter().find(|r| r.id == *planner).cloned() else {
        // An unconfigured planner id is treated exactly like planner ==
        // "" -- never a silent fallback to some route the owner never
        // chose for this.
        if let Ok(Some(mut t)) = app.store.load_task(&task.id) {
            t.mark_failed();
            t.touch(now_ms());
            app.save_task_logged(&t);
            app.broadcast_task(&t);
        }
        return None;
    };

    let (route, swap) = app.usable_route(settings, route)?;
    if let Some(line) = swap {
        if !task.decisions.contains(&line) {
            task.record_decision(line);
        }
    }
    if variant.planner_route.is_some() {
        let line = variant_route_line("planner", &route.id);
        if !task.decisions.contains(&line) {
            task.record_decision(line);
        }
    }

    Some(route)
}
