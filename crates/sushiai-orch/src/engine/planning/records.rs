use crate::clock::now_ms;
use crate::engine::cost::settle_unfinished_cost;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::planning::run::PlanOutcome;
use crate::engine::protected::keep_planned_tests;
use crate::engine::review::write_readonly_claude_settings_with_profile;
use crate::engine::state::App;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::task::AttemptStatus;
use crate::task::Stage;
use crate::task::Task;
use crate::verify::options::split_verify_commands;
use crate::verify::repository::record_plan_check_sources;
use crate::verify::repository::whole_repo_suite;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// Whether a task still needs to run (or re-run) the plan stage, derived
/// purely from data -- never from `task.status` -- so a stop-then-start
/// while drafting, an owner answer that arrives with no live loop to catch
/// it (daemon restart), or a manual `task.start` on a task recovery left
/// `stopped`/`failed` mid-draft, all correctly go back through planning
/// instead of quietly falling through to implement with blank fields. Once
/// one plan attempt has *passed*, this is permanently `false` for the rest
/// of the task's life -- planning runs at most once, ever. A task its parent
/// planner drafted completely (`planned_by_parent`) never runs it: the
/// parent's draft is its plan.
pub(in crate::engine) fn needs_planning(task: &Task) -> bool {
    task.request.is_some()
        && !task.planned_by_parent
        && !task
            .attempts
            .iter()
            .any(|a| a.stage == Stage::Plan && a.status == AttemptStatus::Passed)
}

/// Common cleanup before returning `PlanOutcome::Ended`: if the plan
/// attempt at `idx` is still `running` (cut short by cancellation between
/// its own steps, rather than already concluded `passed`/`failed` by the
/// caller), mark it `interrupted`; either way, make sure the task itself
/// ends up `stopped` unless it's already some other terminal status.
pub(in crate::engine) async fn end_plan_stage(
    app: &Arc<App>,
    task_id: &str,
    idx: Option<usize>,
) -> PlanOutcome {
    if let Some(idx) = idx {
        if let Ok(Some(mut t)) = app.store.load_task(task_id) {
            if t.attempts
                .get(idx)
                .is_some_and(|a| a.status == AttemptStatus::Running)
            {
                crate::task::attempts::end(
                    &mut t,
                    idx,
                    crate::task::attempts::AttemptEnd::Interrupted,
                    now_ms(),
                );
                let prices = app.settings.read().unwrap().prices.clone();
                settle_unfinished_cost(&mut t, idx, &app.store, &prices);
                t.touch(now_ms());
                app.save_task_logged(&t);
                app.broadcast_task(&t);
            }
        }
    }
    mark_stopped_if_not_already(app, task_id).await;
    PlanOutcome::Ended
}

/// The run files of a read-only planner session: an empty MCP config and,
/// for Claude, read-only settings with the route's profile. The caller
/// deletes the key file when the run ends.
pub(in crate::engine) fn write_planner_run_files(
    app: &Arc<App>,
    route: &Route,
    settings: &Settings,
    run_dir: &Path,
) -> (PathBuf, PathBuf, PathBuf) {
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        // Same lean shape as review (read-only, no Stop hook -- no
        // token is ever registered for a drafting session), but the
        // planner route can still carry a profile (env/API key) the
        // same way an implement route can.
        write_readonly_claude_settings_with_profile(
            route,
            app,
            &key_path,
            settings,
            &deny_read,
            &settings_path,
        );
    }
    (mcp_path, settings_path, key_path)
}

/// Writes a parsed draft into the task and ends the plan attempt `idx` as
/// passed; the first draft and a corrected one go through the same code.
pub(in crate::engine) fn store_draft(
    task: &mut Task,
    idx: usize,
    draft: &crate::engine::planning::draft::PlanDraft,
    settings: &Settings,
) {
    let worktree = PathBuf::from(&task.worktree);
    task.record_planned_brief(crate::task::planning::operations::PlannedBrief {
        title: draft.title.clone(),
        goal: draft.goal.clone(),
        tier: draft.tier,
        paths: draft
            .paths
            .iter()
            .map(|path| path.trim().to_string())
            .filter(|path| !path.is_empty())
            .collect(),
        screenshot: draft
            .screenshot
            .clone()
            .map(|command| command.trim().to_string())
            .filter(|command| !command.is_empty()),
    });
    // A whole-repo suite is no per-attempt check: it runs once, as a
    // finalVerify, on the final tree. Before the split, so it also applies
    // when review is off.
    let (suites, kept): (Vec<String>, Vec<String>) = draft
        .verify
        .iter()
        .cloned()
        .partition(|c| whole_repo_suite(&worktree, c));
    // With review off nobody would check a moved entry: keep it as is.
    let (verify, judged) = if task.variant().review_route_id(settings).is_empty() {
        (kept, Vec::new())
    } else {
        split_verify_commands(&worktree, &kept)
    };
    // What the planner says the task edits: a hint that orders sibling
    // subtasks. It never holds a lease; only files really changed do.
    let mut criteria = draft.criteria.clone();
    criteria.extend(
        judged
            .into_iter()
            .map(|command| format!("Checked by review (not a shell command): {command}")),
    );
    let mut finals: Vec<_> = draft
        .final_verify
        .iter()
        .filter(|command| !command.trim().is_empty())
        .cloned()
        .collect();
    for suite in suites {
        if !finals.contains(&suite) {
            finals.push(suite.clone());
        }
        task.record_decision(format!(
            "Orchestrator: verify entry {suite} is a whole-repo suite; moved to finalVerify"
        ));
    }
    crate::task::checks::plan::install(
        task,
        crate::task::checks::plan::CheckPlan {
            criteria,
            visual_criteria: draft.visual_criteria.clone(),
            verify,
            final_verify: finals,
            checks: draft.checks.clone(),
            held_out: draft.held_out.clone(),
        },
    );
    let (tests, dropped) = keep_planned_tests(&draft.tests);
    crate::task::checks::edits::replace_planner_files(task, tests);
    for line in dropped {
        if !task.decisions.contains(&line) {
            task.record_decision(line);
        }
    }
    record_plan_check_sources(task, &worktree);
    task.record_screenshot_command(
        draft
            .screenshot
            .clone()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty()),
    );
    crate::task::attempts::end(
        task,
        idx,
        crate::task::attempts::AttemptEnd::Passed,
        now_ms(),
    );
    task.record_planned_summary(idx, &draft.title);
    task.touch(now_ms());
}
