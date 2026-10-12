use super::recovery::{follow_run, resumable_attempt};
use super::time_budget;
use super::*;

/// The per-task attempt loop (spec "Engine", steps 1-10). Runs as its own
/// tokio task from `task.start` until the task reaches `done`/`failed`, is
/// stopped, or the daemon shuts down; `waiting` parks it on `pending_answer`
/// rather than exiting, so the in-memory attempt-budget extension (step 9)
/// survives across a wait/answer cycle.
pub(super) async fn run_task_loop(
    app: Arc<App>,
    task_id: String,
    pending_answer: Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    pending_amend: Arc<StdMutex<Option<Amendment>>>,
    cancel: CancelToken,
    auto_start_after_plan: bool,
) {
    let mut attempt_budget = app.settings.read().unwrap().max_attempts;
    let mut budget_loaded = false;
    // Set once a parent's finish failed after every subtask landed: the
    // failure its own implement attempt has to fix. While it is set (and once
    // the parent has an attempt) the parent never goes back to `run_parent`.
    let mut parent_failure: Option<String> = None;
    let mut parent_conflict_only = false;

    'attempts: loop {
        if cancel.is_cancelled() {
            mark_stopped_if_not_already(&app, &task_id).await;
            app.finish_task_loop(&task_id);
            return;
        }

        let mut task = match app.store.load_task(&task_id) {
            Ok(Some(t)) => t,
            _ => {
                app.finish_task_loop(&task_id);
                return;
            }
        };
        if task.queue.park.is_some() {
            app.finish_task_loop(&task_id);
            return;
        }
        if !budget_loaded {
            budget_loaded = true;
            if let Some(n) = task.variant().max_attempts {
                attempt_budget = n;
            }
        }

        // A finished task started again is no longer finished.
        if task.metrics.is_some() && !matches!(task.status, TaskStatus::Done | TaskStatus::Failed) {
            task.metrics = None;
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
        }

        // A usage or rate limit pause that outlived the loop (a restart) is
        // served first, holding no slot; the mark is cleared when it ends.
        if let Some(until) = task.queue.paused_until {
            let wait = (until - now_ms()).max(0) as u64;
            let resumed = tokio::select! {
                _ = cancel.cancelled() => false,
                _ = tokio::time::sleep(Duration::from_millis(wait)) => true,
            };
            if !resumed {
                mark_stopped_if_not_already(&app, &task_id).await;
                app.finish_task_loop(&task_id);
                return;
            }
            task.queue.paused_until = None;
            task.updated_at = now_ms();
            app.save_task_logged(&task);
            app.broadcast_task(&task);
        }

        // A worktree removed while the task was archived comes back before
        // anything touches it.
        let recreated = match app.ensure_worktree(&mut task).await {
            Ok(true) => {
                task.updated_at = now_ms();
                app.save_task_logged(&task);
                app.broadcast_task(&task);
                true
            }
            Ok(false) => false,
            Err(e) => {
                task.decisions.push(format!("Worktree: {e}"));
                task.status = TaskStatus::Failed;
                task.updated_at = now_ms();
                app.save_task_logged(&task);
                app.broadcast_task(&task);
                app.finish_task_loop(&task_id);
                return;
            }
        };
        // A new (or recreated) worktree runs its setup before anything else
        // uses it; a failing setup fails the task with no attempt.
        if let Err(e) = app.ensure_worktree_setup(&task, recreated, &cancel).await {
            if cancel.is_cancelled() {
                mark_stopped_if_not_already(&app, &task_id).await;
            } else {
                task.decisions.push(format!("Worktree setup: {e}"));
                task.status = TaskStatus::Failed;
                task.updated_at = now_ms();
                app.save_task_logged(&task);
                app.broadcast_task(&task);
            }
            app.finish_task_loop(&task_id);
            return;
        }

        apply_pending_amendment(&app, &mut task, &pending_amend, &cancel).await;

        // The task graph: a parent never implements, and a task whose
        // dependencies are not all done waits `queued` with no loop until
        // `advance_graph` starts it again. Drafting runs meanwhile.
        // A subtask is planned only once everything it depends on has landed,
        // so no planning is spent on a part that cannot start yet.
        if !needs_planning(&task) || task.parent.is_some() {
            let all = app.repo_tasks(&task.repo);
            if is_parent(&task, &all)
                && parent_failure.is_none()
                && (completed_implement_attempts(&task) == 0
                    || !matches!(waits_state(&task, &all), Waits::Ready | Waits::Nothing))
            {
                match run_parent(&app, &task_id, &pending_answer, &pending_amend, &cancel).await {
                    ParentEnd::Finished => {
                        app.finish_task_loop(&task_id);
                        return;
                    }
                    ParentEnd::Attempt {
                        failure,
                        budget,
                        conflict_only,
                    } => {
                        parent_conflict_only = conflict_only;
                        parent_failure = Some(failure);
                        attempt_budget = budget.unwrap_or(attempt_budget);
                        continue 'attempts;
                    }
                }
            }
            let parent_blocked = task
                .parent
                .as_deref()
                .and_then(|pid| all.iter().find(|t| t.id == pid))
                .is_some_and(|p| {
                    !matches!(own_waits_state(p, &all), Waits::Ready | Waits::Nothing)
                        || p.queue.backlog.is_some()
                        || app.is_suspended(p)
                        || matches!(p.status, TaskStatus::Stopped | TaskStatus::Failed)
                        // A parent parked on a question (its base check) starts
                        // no subtask until it is answered.
                        || (p.status == TaskStatus::Waiting
                            && completed_implement_attempts(&task) == 0)
                });
            if parent_blocked
                || (!task.depends_on.is_empty()
                    && !matches!(waits_state(&task, &all), Waits::Ready | Waits::Nothing))
            {
                if task.status != TaskStatus::Queued || task.queue.queue_reason.is_some() {
                    task.status = TaskStatus::Queued;
                    task.queue.queue_reason = None;
                    task.updated_at = now_ms();
                    app.save_task_logged(&task);
                    app.broadcast_task(&task);
                }
                app.finish_task_loop(&task_id);
                return;
            }
            // Paths another live task on this base holds: wait `queued`, no
            // slot taken, until that task lands, stops or fails.
            if !needs_planning(&task) {
                let force = task.status == TaskStatus::Landing
                    || completed_implement_attempts(&task) > 0
                    || parent_failure.is_some();
                if !pass_lease_gate(&app, &mut task, force) {
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        // An answered retry survives a daemon restart without another
        // attempt or another question on the same base.
        let checks: Vec<String> = task
            .verify
            .iter()
            .chain(&task.final_verify)
            .cloned()
            .collect();
        if base_retry_requested(&task, &checks, &task.base_sha) {
            let worktree = PathBuf::from(&task.worktree);
            match carry_after_base_moves(
                &app,
                &mut task,
                &worktree,
                &pending_answer,
                &cancel,
                &mut None,
            )
            .await
            {
                BaseRetry::Ready => {}
                BaseRetry::Conflict(detail) => {
                    if let Some(idx) = task
                        .attempts
                        .iter()
                        .rposition(|a| a.stage == Stage::Implement)
                    {
                        record_failure(&mut task, idx, FailureKind::Conflict, detail);
                    } else {
                        parent_conflict_only = true;
                        parent_failure = Some(detail);
                    }
                }
                BaseRetry::Stopped => {
                    mark_stopped_if_not_already(&app, &task_id).await;
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
            attempt_budget = attempt_budget.max(counted_attempts(&task) + 1);
            task.updated_at = now_ms();
            app.save_task_logged(&task);
            app.broadcast_task(&task);
        }

        // Concurrency limit: stays `queued` while waiting for a slot, and
        // `task.stop` (via `cancel`) works here too.
        let mut permit = tokio::select! {
            _ = cancel.cancelled() => {
                mark_stopped_if_not_already(&app, &task_id).await;
                app.finish_task_loop(&task_id);
                return;
            }
            permit = app.slots.clone().acquire_owned() => {
                match permit {
                    Ok(p) => Some(p),
                    Err(_) => { app.finish_task_loop(&task_id); return; }
                }
            }
        };

        // A task that passed everything and waited to land: only the landing
        // is tried again.
        if task.status == TaskStatus::Landing && !task.attempts.is_empty() {
            let idx = task.attempts.len() - 1;
            let attempt_n = task.attempts[idx].n;
            let run_dir = app.store.run_dir(&task_id, attempt_n);
            let _ = std::fs::create_dir_all(&run_dir);
            let worktree = PathBuf::from(&task.worktree);
            match finish_attempt(
                &app,
                &task_id,
                &mut task,
                idx,
                attempt_n,
                &worktree,
                &run_dir,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                Tail::Continue => {
                    continue;
                }
                Tail::Return => return,
            }
        }

        // The owner accepted the last attempt while no loop was live: only
        // its final checks and the commit are left.
        if let Some(idx) = pending_acceptance(&task) {
            let attempt_n = task.attempts[idx].n;
            let run_dir = app.store.run_dir(&task_id, attempt_n);
            let _ = std::fs::create_dir_all(&run_dir);
            let settings = app.settings.read().unwrap().clone();
            let worktree = PathBuf::from(&task.worktree);
            let a = &mut task.attempts[idx];
            a.status = AttemptStatus::Running;
            a.ended_at = None;
            a.failure = None;
            task.status = TaskStatus::Running;
            task.updated_at = now_ms();
            app.save_task_logged(&task);
            app.broadcast_task(&task);
            match finish_after_review(
                &app,
                &task_id,
                &mut task,
                idx,
                attempt_n,
                &worktree,
                &run_dir,
                &settings,
                None,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                Tail::Continue => {
                    continue;
                }
                Tail::Return => return,
            }
        }

        if needs_planning(&task) {
            // A task planned late (a backlog request) plans on the base as it
            // is now, when nothing has touched its worktree yet.
            // A subtask keeps the parent-head move done before attempt 1.
            if task.attempts.is_empty() && task.parent.is_none() {
                if let Some(base_ref) = task.base_ref.clone() {
                    let (wt, from, r) = (
                        PathBuf::from(&task.worktree),
                        task.base_sha.clone(),
                        base_ref.clone(),
                    );
                    let moved = tokio::task::spawn_blocking(move || {
                        git::reset_untouched_to_base(&wt, &r, &from)
                    })
                    .await;
                    if let Ok(Ok(Some(new_sha))) = moved {
                        task.decisions.push(format!(
                            "Rebase: planning from {base_ref} at {}",
                            short_sha(&new_sha)
                        ));
                        task.base_sha = new_sha;
                    }
                }
            }
            if task.status == TaskStatus::Queued {
                task.status = TaskStatus::Drafting;
            }
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
            let planned = run_plan_stage(
                &app,
                &task_id,
                &pending_answer,
                &cancel,
                &mut permit,
                auto_start_after_plan,
            )
            .await;
            if let Ok(Some(mut planned_task)) = app.store.load_task(&task_id) {
                apply_pending_amendment(&app, &mut planned_task, &pending_amend, &cancel).await;
            }
            match planned {
                PlanOutcome::Proceed => {
                    drop(permit);
                    continue;
                }
                PlanOutcome::StoppedForReview | PlanOutcome::Ended => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        // A child may have been attached while this task waited for a slot:
        // it must never implement the whole request itself then.
        if completed_implement_attempts(&task) == 0 && parent_failure.is_none() {
            let all = app.repo_tasks(&task.repo);
            if is_parent(&task, &all) {
                drop(permit);
                match run_parent(&app, &task_id, &pending_answer, &pending_amend, &cancel).await {
                    ParentEnd::Finished => {
                        app.finish_task_loop(&task_id);
                        return;
                    }
                    ParentEnd::Attempt {
                        failure,
                        budget,
                        conflict_only,
                    } => {
                        parent_conflict_only = conflict_only;
                        parent_failure = Some(failure);
                        attempt_budget = budget.unwrap_or(attempt_budget);
                        continue 'attempts;
                    }
                }
            }
        }

        // A parent-drafted child may have been drafted against files its
        // dependencies then changed: one cheap refresh of its commands and
        // paths, before its brief is checked.
        if completed_implement_attempts(&task) == 0
            && parent_failure.is_none()
            && task.planned_by_parent
            && !task.brief_check.refreshed
        {
            if refresh_after_dependencies(&app, &task_id, &cancel).await {
                drop(permit);
                mark_stopped_if_not_already(&app, &task_id).await;
                app.finish_task_loop(&task_id);
                return;
            }
            if let Ok(Some(reloaded)) = app.store.load_task(&task_id) {
                task = reloaded;
            }
        }

        // Explicit criteria are checked once, before the first attempt; a
        // planned brief was already checked in the plan stage.
        if completed_implement_attempts(&task) == 0
            && !task.brief_check.done
            && parent_failure.is_none()
        {
            if let BriefAction::Cancelled = check_brief(&app, &task_id, false, &cancel).await {
                drop(permit);
                mark_stopped_if_not_already(&app, &task_id).await;
                app.finish_task_loop(&task_id);
                return;
            }
            if let Ok(Some(reloaded)) = app.store.load_task(&task_id) {
                task = reloaded;
            }
        }

        // A criterion the task has no tool for is put to the owner before an
        // attempt is spent on it.
        if completed_implement_attempts(&task) == 0
            && parent_failure.is_none()
            && !task.brief_check.missing_tool_asked
        {
            if let Some(missing) = task.brief_check.missing_tool.clone() {
                task.question = Some(permissions::missing_tool_question(&task, &missing));
                task.status = TaskStatus::Waiting;
                task.updated_at = now_ms();
                let Some(answer) = wait_for_answer(
                    &app,
                    &task_id,
                    &mut task,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                else {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                };
                permissions::apply_missing_answer(&app, &mut task, &missing, &answer);
                task.brief_check.missing_tool_asked = true;
                task.brief_check.missing_tool = None;
                task.updated_at = now_ms();
                app.save_task_logged(&task);
                app.broadcast_task(&task);
                drop(permit);
                continue 'attempts;
            }
        }

        // Every check command runs on the base once, in the background:
        // attempt 1 starts at once and the job holds no slot. A command the
        // repository cannot run is rewritten, one broken for an unrelated
        // reason stops gating; the loop applies the ruling where it is
        // needed (a failing verify command, the gate before finalVerify).
        if completed_implement_attempts(&task) == 0
            && !task.brief_check.feasibility_done
            && parent_failure.is_none()
        {
            if heal_grep_dashes(&mut task) {
                app.verify_cache.lock().unwrap().remove(&task_id);
                task.updated_at = now_ms();
                app.save_task_logged(&task);
                app.broadcast_task(&task);
            }
            spawn_base_preflight(&app, &task, &cancel);
        }

        let status = task.status;
        if !wait_while_over_budget(
            &app,
            &task_id,
            &mut task,
            "implement",
            status,
            &pending_answer,
            &cancel,
            &mut permit,
        )
        .await
        {
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }

        // A task in a graph starts from its base branch as it is now, so a
        // dependent begins on top of the work it waited for (a subtask on
        // its parent's head, with every earlier sibling landed).
        if let Some(prev_id) = task.queue.relay_of.clone() {
            if completed_implement_attempts(&task) == 0 && task.queue.relay_base.is_none() {
                start_relay_link(&app, &mut task, &prev_id).await;
            }
        }
        if task.parent.is_some() {
            let _ = app.sync_graph_base(&task.id, false).await;
            app.carry_part_onto_parent(&mut task).await;
        } else if completed_implement_attempts(&task) == 0
            && parent_failure.is_none()
            && (!task.depends_on.is_empty() || task.queue.waited_on_lease)
            && task.queue.relay_of.is_none()
        {
            let wt = PathBuf::from(&task.worktree);
            carry_onto_newest_base(&mut task, &wt).await;
        }
        task.queue.waited_on_lease = false;
        if cancel.is_cancelled() {
            mark_stopped_if_not_already(&app, &task_id).await;
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }

        if completed_implement_attempts(&task) == 0 && needs_prepare(&task) {
            let wt = PathBuf::from(&task.worktree);
            if !prepare_tests(&app, &task_id, &mut task, &wt, &cancel).await {
                mark_stopped_if_not_already(&app, &task_id).await;
                drop(permit);
                app.finish_task_loop(&task_id);
                return;
            }
            app.save_task_logged(&task);
            app.broadcast_task(&task);
        }
        let dependencies_before = task.depends_on.clone();
        apply_pending_amendment(&app, &mut task, &pending_amend, &cancel).await;
        if task.depends_on != dependencies_before && completed_implement_attempts(&task) == 0 {
            continue 'attempts;
        }

        // A final check that already fails on the base is looked up after
        // the attempt, from the cache or the job started here: the first
        // attempt does not wait for it.
        if completed_implement_attempts(&task) == 0 && parent_failure.is_none() {
            start_final_base_jobs(&app, &task);
        }

        let mut planner_tier_used = false;
        let mut no_planner_tier = false;
        if completed_implement_attempts(&task) == 0 {
            match task.planned_tier {
                Some(tier) => {
                    task.tier = tier;
                    task.tier_fallback = None;
                    planner_tier_used = true;
                }
                None => {
                    task.tier = Tier::Standard;
                    task.tier_fallback = Some("no planner tier".to_string());
                    no_planner_tier = true;
                }
            }
        }

        // A run that outlived the daemon is followed, not started again: its
        // outcome goes through review and verify like any other pass.
        let resumed = resumable_attempt(&app, &task);
        let (
            settings,
            attempt_n,
            idx,
            route,
            worktree,
            base_sha,
            run_dir,
            key_path,
            mut registered,
            run_result,
            best_of,
        ) = if let Some(idx) = resumed {
            let settings = app.settings.read().unwrap().clone();
            let a = task.attempts[idx].clone();
            let route = settings
                .routes
                .iter()
                .find(|r| r.id == a.route_id)
                .cloned()
                .unwrap_or_else(|| Route {
                    id: a.route_id.clone(),
                    label: a.route_id.clone(),
                    harness: a.harness,
                    model: (!a.model.is_empty()).then(|| a.model.clone()),
                    effort: None,
                    profile_id: None,
                    account_id: None,
                    strength: None,
                });
            let run_dir = app.store.run_dir(&task_id, a.n);
            // The earlier daemon may have counted this run's spend before it
            // died (during review); the outcome below counts it again.
            if let Some(counted) = task.attempts[idx].cost_usd.take() {
                task.cost_usd -= counted;
            }
            // A review it died under is paid for too; the review runs again.
            settle_interrupted_review(&mut task, idx, &app.store, &settings.prices);
            task.status = TaskStatus::Running;
            task.updated_at = now_ms();
            app.save_task_logged(&task);
            app.broadcast_task(&task);
            // The agent still running holds the token of its Stop hook: the
            // earlier daemon left it with the run files.
            let worktree = PathBuf::from(&task.worktree);
            let registered =
                std::fs::read_to_string(&RunFiles::of(&run_dir.join("events.jsonl")).token)
                    .ok()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .map(|token| {
                        let ctx = HookContext::new(&task, a.n, &worktree, task.work_base());
                        app.register_hook(&token, &ctx, &run_dir);
                        (token, ctx)
                    });
            let guard = Guard::for_variant(
                &task.variant(),
                registered.as_ref().map(|(_, ctx)| ctx.hook_running.clone()),
            )
            .with_time_budget(settings.implement_budget_mins, a.started_at);
            let result = follow_run(&app, &task, idx, &cancel, guard).await;
            (
                settings,
                a.n,
                idx,
                route,
                worktree,
                task.base_sha.clone(),
                run_dir.clone(),
                run_dir.join("key"),
                registered,
                result,
                BestOfExtra::default(),
            )
        } else {
            let resume_status = task.status;
            if !wait_for_harness(
                &app,
                &task_id,
                &mut task,
                resume_status,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                drop(permit);
                app.finish_task_loop(&task_id);
                return;
            }
            let settings = app.settings.read().unwrap().clone();
            let attempt_n = task.attempts.iter().map(|a| a.n).max().unwrap_or(0) + 1;
            // A landing that conflicted only needs its markers resolved: that
            // runs on the cheap route, not as a full re-implementation.
            let carry_conflicts = task.queue.carry_conflicts.clone();
            let conflict_only = !carry_conflicts.is_empty()
                || last_failure_is_conflict(&task)
                || (parent_conflict_only && completed_implement_attempts(&task) == 0);
            let (route_id, overridden) = task.variant().implement_route_id(
                &settings,
                if conflict_only {
                    Tier::Mechanical
                } else {
                    task.tier
                },
            );
            if conflict_only {
                task.decisions.push(format!(
                "Orchestrator: the carry conflicted -> conflict-only attempt on the cheap route {route_id}"
            ));
            } else if overridden {
                let line = variant_route_line(&format!("tier {}", task.tier.as_str()), &route_id);
                if !task.decisions.contains(&line) {
                    task.decisions.push(line);
                }
            }
            if planner_tier_used {
                task.decisions.push(format!(
                    "Planner: tier {} -> route {route_id}",
                    task.tier.as_str()
                ));
            }
            if no_planner_tier {
                task.decisions.push(no_planner_tier_line(&route_id));
            }
            let route = settings
                .routes
                .iter()
                .find(|r| r.id == route_id)
                .cloned()
                .unwrap_or(Route {
                    id: "codex".to_string(),
                    label: "Codex".to_string(),
                    harness: Harness::Codex,
                    model: None,
                    effort: None,
                    profile_id: None,
                    account_id: None,
                    strength: None,
                });
            let Some((route, swap)) = app.usable_route(&settings, route) else {
                // No route runs on an installed harness: ask again.
                continue 'attempts;
            };
            if let Some(line) = swap {
                if !task.decisions.contains(&line) {
                    task.decisions.push(line);
                }
            }

            let worktree = PathBuf::from(&task.worktree);
            let base_sha = task.base_sha.clone();
            let wt2 = worktree.clone();
            let base2 = base_sha.clone();
            let (status_short, diff_stat) = tokio::task::spawn_blocking(move || {
                (
                    git::status_short(&wt2).unwrap_or_default(),
                    git::diff_stat(&wt2, &base2).unwrap_or_default(),
                )
            })
            .await
            .unwrap_or_default();

            // A finding the reviewer says came back gets the advisor whether or
            // not the variant asks for it.
            let repeated_finding = last_review_repeated(&task);
            if task.variant().advisor || repeated_finding {
                if let Some(cost) = run_advisor_before_retry(
                    &app,
                    &mut task,
                    &settings,
                    &route,
                    &worktree,
                    &base_sha,
                    repeated_finding,
                    &cancel,
                )
                .await
                {
                    task.cost_usd += cost;
                    task.updated_at = now_ms();
                    app.save_task_logged(&task);
                    app.broadcast_task(&task);
                    // The advisor's spend can be what reaches the budget.
                    let status = task.status;
                    if !wait_while_over_budget(
                        &app,
                        &task_id,
                        &mut task,
                        "implement",
                        status,
                        &pending_answer,
                        &cancel,
                        &mut permit,
                    )
                    .await
                    {
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                }
            }

            // An owner or policy answer that contradicts a criterion amends it
            // before the attempt is briefed.
            if heal_answers(&app, &mut task, attempt_n, &cancel)
                .await
                .is_err()
            {
                drop(permit);
                mark_stopped_if_not_already(&app, &task_id).await;
                app.finish_task_loop(&task_id);
                return;
            }
            let brief_text = brief::build_brief(&task, &status_short, &diff_stat);
            let brief_text = if conflict_only {
                brief::with_block_before_report(
                    &brief_text,
                    &brief::conflict_only_block(&carry_conflicts),
                )
            } else {
                brief_text
            };
            let brief_text = match &task.brief_check.conflict {
                Some(conflict) if completed_implement_attempts(&task) == 0 => {
                    brief::with_block_before_report(&brief_text, &brief::conflict_block(conflict))
                }
                _ => brief_text,
            };
            let brief_text = brief::with_block_before_report(
                &brief_text,
                &brief::landed_dependencies_block(&task, &app.repo_tasks(&task.repo), None),
            );
            let brief_text = if completed_implement_attempts(&task) == 0 {
                brief::with_block_before_report(
                    &brief_text,
                    &brief::relay_block(&task, &app.repo_tasks(&task.repo)),
                )
            } else {
                brief_text
            };
            // A parent's first attempt gets what failed when it tried to finish.
            let brief_text = match &parent_failure {
                Some(failure) if completed_implement_attempts(&task) == 0 => {
                    brief::with_block_before_report(
                        &brief_text,
                        &brief::parent_failure_block(failure),
                    )
                }
                _ => brief_text,
            };
            // The messages sent to this task since its last attempt are
            // delivered here, with the attempt about to start.
            let messages = messages::brief_block_for_task(&app, &task_id);
            let brief_text = brief::with_block_before_report(&brief_text, &messages);
            let advice = task
                .attempts
                .iter()
                .rev()
                .find(|a| a.stage == Stage::Implement)
                .and_then(|a| a.advice.clone());
            let brief_text = match advice.as_deref() {
                Some(advice) => {
                    brief::with_block_before_report(&brief_text, &brief::advisor_block(advice))
                }
                _ => brief_text,
            };

            let tools = permissions::task_tools(&app, &settings, &task, &worktree);
            for line in &tools.notes {
                if !task.decisions.contains(line) {
                    task.decisions.push(line.clone());
                }
            }
            let brief_text = brief::with_block_before_report(
                &brief_text,
                &permissions::tools_block(&tools, false),
            );
            let mut no_second = BestOfExtra::default();
            // Recorded after the reload below, which would drop a line pushed now.
            let second = match if conflict_only {
                Ok(None)
            } else {
                second_route(
                    &task,
                    &settings,
                    &route,
                    implement_attempt_count(&task) == 0,
                )
            } {
                Ok(second) => second,
                Err(line) => {
                    no_second.decisions.push(line);
                    None
                }
            };
            let full_brief = brief_text;
            let previous = resume_attempt(&task, &route, conflict_only, second.is_some());
            let resume = previous.and_then(|a| a.session_id.clone());
            let brief_text = match previous {
                Some(a) => brief::build_resume_brief(&task, a, &messages, advice.as_deref()),
                None => full_brief.clone(),
            };
            if let Some(session) = &resume {
                task.decisions.push(format!(
                    "Orchestrator: attempt {attempt_n} resumes session {session} on route {} after a failed check",
                    route.id
                ));
            }

            let reason = if conflict_only {
                format!("conflict only -> route {}", route.id)
            } else {
                format!("tier {} -> route {}", task.tier.as_str(), route.id)
            };
            let attempt = Attempt {
                n: attempt_n,
                stage: Stage::Implement,
                route_id: route.id.clone(),
                harness: route.harness,
                model: route.model.clone().unwrap_or_default(),
                reason,
                session_id: None,
                pgid: None,
                started_at: now_ms(),
                ended_at: None,
                status: AttemptStatus::Running,
                summary: None,
                handoff: None,
                disputes: vec![],
                changed_files: vec![],
                verify: vec![],
                gate_blocks: 0,
                prefix_tokens: None,
                review: None,
                failure: None,
                usage: None,
                cost_usd: None,
                cost_estimated: false,
                review_cost_usd: None,
                evidence: vec![],
                evidence_tree: None,
                review_tree: None,
                review_base: None,
                review_scope: None,
                evidence_base: None,
                evidence_from: None,
                advice: None,
                advisor_cost_usd: None,
                fingerprint: None,
                review_fingerprint: None,
                advisor_fingerprint: None,
                candidates: vec![],
                criteria_results: vec![],
                diff_stat: None,
            };
            if !carry_conflicts.is_empty() {
                task.queue.carry_conflict_attempt = Some(attempt_n);
                task.queue.carry_conflicts.clear();
            }
            task.attempts.push(attempt);
            let idx = task.attempts.len() - 1;
            refresh_criteria_results(&mut task, idx);
            task.status = TaskStatus::Running;
            task.updated_at = now_ms();
            app.save_task_logged(&task);
            app.broadcast_task(&task);

            let run_dir = app.store.run_dir(&task_id, attempt_n);
            let _ = std::fs::create_dir_all(&run_dir);
            let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
            let PreparedRun {
                mcp_path,
                settings_path,
                key_path,
                messages_server,
                registered,
                sandbox,
            } = prepare_run(
                &app, &task, &task_id, &route, &settings, &worktree, attempt_n, &run_dir, &tools,
            );

            let network_allowed = settings.codex_network;
            let codex_mcp = tools.codex_servers((sandbox.is_none()).then_some(&messages_server));
            let mut req = harness::RunRequest {
                harness: route.harness,
                worktree: &worktree,
                model: route.model.as_deref(),
                effort: route.effort.as_deref(),
                max_budget_usd: (task.variant().max_attempt_cost_usd > 0.0)
                    .then(|| task.variant().max_attempt_cost_usd),
                review: false,
                task_run: true,
                task_sandbox: sandbox.as_ref(),
                mcp_config: Some(&mcp_path),
                settings_path: Some(&settings_path),
                network_allowed,
                codex_mcp: &codex_mcp,
                images: &[],
                repo_settings: true,
                resume: resume.as_deref(),
            };

            let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
            let events_path = run_dir.join("events.jsonl");
            let a_run = run_harness(
                &app,
                &task_id,
                attempt_n,
                RunTrack::Resumable,
                &worktree,
                &req,
                CostTag::task("implement", &route.id),
                &brief_text,
                &events_path,
                &cancel,
                Guard::for_variant(
                    &task.variant(),
                    registered.as_ref().map(|(_, ctx)| ctx.hook_running.clone()),
                )
                .with_time_budget(
                    settings.implement_budget_mins,
                    task.attempts[idx].started_at,
                ),
            );
            let (mut run_result, mut best_of) = match second {
                Some(b_route) => {
                    run_best_of(
                        &app,
                        &task,
                        &task_id,
                        attempt_n,
                        a_run,
                        &route,
                        &b_route,
                        &settings,
                        &worktree,
                        &base_sha,
                        &run_dir,
                        &brief_text,
                        &deny_read,
                        &cancel,
                    )
                    .await
                }
                None => (a_run.await, no_second),
            };
            let rejected = match &run_result {
                Ok(outcome) => outcome.error.is_some() && outcome.session_id.is_none(),
                Err(RunError::Io(_) | RunError::NotFound(_)) => true,
                Err(RunError::Cancelled) => false,
            };
            if req.resume.is_some() && rejected && !cancel.is_cancelled() {
                best_of.decisions.push(format!(
                    "Orchestrator: attempt {attempt_n} resume rejected without a session init; falling back to a fresh run in the same attempt"
                ));
                req.resume = None;
                let _ = std::fs::rename(&events_path, run_dir.join("resume-rejected.events.jsonl"));
                let _ = std::fs::write(run_dir.join("brief.md"), &full_brief);
                // Subscription credentials are consumed when the CLI starts.
                if let Some(account) = route
                    .account_id
                    .as_ref()
                    .and_then(|id| app.secrets.read().unwrap().accounts.get(id).cloned())
                {
                    if account.kind == "subscription" {
                        let _ =
                            store::write_secret_file(&run_dir.join("oauth-token"), &account.value);
                    }
                }
                run_result = run_harness(
                    &app,
                    &task_id,
                    attempt_n,
                    RunTrack::Resumable,
                    &worktree,
                    &req,
                    CostTag::task("implement", &route.id),
                    &full_brief,
                    &events_path,
                    &cancel,
                    Guard::for_variant(
                        &task.variant(),
                        registered.as_ref().map(|(_, ctx)| ctx.hook_running.clone()),
                    )
                    .with_time_budget(
                        settings.implement_budget_mins,
                        task.attempts[idx].started_at,
                    ),
                )
                .await;
            }
            (
                settings, attempt_n, idx, route, worktree, base_sha, run_dir, key_path, registered,
                run_result, best_of,
            )
        };

        if matches!(run_result, Err(RunError::Cancelled)) && app.leaves_run_going(&task, idx) {
            // The run outlives this daemon and stays `running`; the next
            // daemon's loop follows it, with its token and key file as they
            // are. Only the in-memory token goes.
            if let Some((token, _)) = registered.take() {
                app.unload_hook(&token);
            }
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }
        let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
        let (gate_blocks, handled, staged_lines) = if let Some((tok, ctx)) = registered.take() {
            app.forget_hook(&tok, &run_dir);
            ctx.cancel.cancel();
            let applied = permissions::finish_staging(&ctx);
            (
                ctx.blocks.load(Ordering::SeqCst),
                ctx.handled.lock().unwrap().clone(),
                permissions::staging_lines(&applied),
            )
        } else {
            (0, Default::default(), Vec::new())
        };
        // Delete the per-run key file the moment the run ends (spec item
        // B); recovery also deletes it for an attempt interrupted by an
        // unclean shutdown.
        let _ = std::fs::remove_file(&key_path);
        let _ = std::fs::remove_file(run_dir.join("mcp.json"));

        // `run_harness` may have persisted `session_id`/`pgid` mid-run;
        // reload so we don't clobber that with our stale in-memory copy --
        // then reapply `gate_blocks`, which is never itself persisted
        // mid-run and so isn't on the reloaded copy at all.
        if let Ok(Some(reloaded)) = app.store.load_task(&task_id) {
            task = reloaded;
        }
        task.attempts[idx].gate_blocks = gate_blocks;
        task.decisions.extend(staged_lines);
        // Both candidates' spend counts, the loser's and the pick run's on
        // top of the winner's (added below with the attempt's own).
        task.cost_usd += best_of.cost;
        task.attempts[idx].candidates = best_of.candidates;
        record_check_decisions(&mut task, best_of.decisions);
        let b_won = best_of.winner.is_some();
        let route = match best_of.winner {
            Some(winner) => {
                let a = &mut task.attempts[idx];
                a.route_id = winner.id.clone();
                a.harness = winner.harness;
                a.model = winner.model.clone().unwrap_or_default();
                a.reason = format!("{}; best-of winner", a.reason);
                winner
            }
            None => route,
        };

        let outcome = match run_result {
            Ok(o) => o,
            Err(RunError::Cancelled) => {
                task.attempts[idx].status = AttemptStatus::Interrupted;
                task.attempts[idx].ended_at = Some(now_ms());
                settle_unfinished_cost(&mut task, idx, &app.store, &settings.prices);
                app.apply_cancellation(&mut task);
                if task.status == TaskStatus::Stopped {
                    app.close_stopped_task(&mut task);
                }
                task.updated_at = now_ms();
                app.save_task_logged(&task);
                app.broadcast_task(&task);
                drop(permit);
                app.finish_task_loop(&task_id);
                return;
            }
            Err(RunError::NotFound(msg)) => {
                // Nothing ran: the attempt is not spent. The harness is
                // marked missing so the next round routes around it.
                app.mark_harness_missing(route.harness);
                task.attempts.remove(idx);
                task.decisions
                    .push(format!("Orchestrator: {msg}; no attempt was used"));
                task.updated_at = now_ms();
                app.save_task_logged(&task);
                app.broadcast_task(&task);
                drop(permit);
                continue;
            }
            Err(RunError::Io(msg)) => {
                match fail_and_continue(
                    &app,
                    &task_id,
                    &mut task,
                    idx,
                    FailureKind::Error,
                    msg,
                    &mut attempt_budget,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                {
                    LoopSignal::Continue => {
                        drop(permit);
                        continue;
                    }
                    LoopSignal::Stop => {
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                }
            }
        };

        // The winner's session lives under the sibling worktree's path, so
        // a later resume in the task's worktree could never find it.
        task.attempts[idx].session_id = outcome.session_id.clone().filter(|_| !b_won);
        task.attempts[idx].fingerprint = outcome.fingerprint.clone();
        task.attempts[idx].usage = Some(Usage {
            input: outcome.usage_input,
            output: outcome.usage_output,
            cached: outcome.usage_cached,
        });
        if route.harness == Harness::Claude {
            task.attempts[idx].prefix_tokens = outcome.first_turn_tokens;
        }
        let cost = outcome.cost_usd;
        task.attempts[idx].cost_usd = cost;
        task.attempts[idx].cost_estimated = outcome.cost_estimated;
        if let Some(cost) = cost {
            task.cost_usd += cost;
        }

        if outcome.time_budget {
            let detail = outcome.error.clone().unwrap_or_default();
            let (wt, base) = (worktree.clone(), base_sha.clone());
            let changed = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &base).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            let session = outcome.session_id.as_deref().filter(|_| !b_won);
            let report = time_budget::report_after_budget(
                &app, &task_id, attempt_n, &route, &worktree, &run_dir, session, &changed, &cancel,
            )
            .await;
            let a = &mut task.attempts[idx];
            a.handoff = Some(report.handoff);
            a.summary = report.summary;
            a.changed_files = changed;
            if report.cost > 0.0 {
                a.cost_usd = Some(a.cost_usd.unwrap_or(0.0) + report.cost);
                task.cost_usd += report.cost;
            }
            record_diff_stat(&mut task, idx, &worktree).await;
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                FailureKind::TimeBudget,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue => {
                    drop(permit);
                    continue;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        if outcome.stalled || outcome.over_budget {
            let kind = if outcome.over_budget {
                FailureKind::Budget
            } else {
                FailureKind::Stall
            };
            let detail = outcome.error.clone().unwrap_or_default();
            let (wt, base) = (worktree.clone(), task.work_base().to_string());
            task.attempts[idx].changed_files = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &base).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            record_diff_stat(&mut task, idx, &worktree).await;
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                kind,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue => {
                    drop(permit);
                    continue;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        if let Some(detail) = outcome.looped.clone() {
            let (wt, base) = (worktree.clone(), task.work_base().to_string());
            task.attempts[idx].changed_files = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &base).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            record_diff_stat(&mut task, idx, &worktree).await;
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                FailureKind::Loop,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue => {
                    drop(permit);
                    continue;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        // A call the run was refused and orchd never decided: one owner
        // question, not another attempt that would be refused the same way.
        let refused = permissions::unhandled_denials(&handled, &worktree, &outcome);
        if !refused.is_empty() {
            let (wt, base) = (worktree.clone(), task.work_base().to_string());
            task.attempts[idx].changed_files = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &base).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            record_diff_stat(&mut task, idx, &worktree).await;
            match permissions::stop_for_denials(
                &app,
                &task_id,
                &mut task,
                idx,
                &refused,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue => {
                    drop(permit);
                    continue;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        let final_text = outcome.final_text.clone().unwrap_or_default();
        let report = brief::parse_report(&final_text);
        // Not a gate: the reviewer weighs it. A `partial` or missing report,
        // or a harness that errored after editing files, is exactly what a
        // reviewer should look at harder.
        let implementer_note = format!(
            "Implementer report: {}{}",
            match report.as_ref().map(|r| &r.outcome) {
                Some(brief::Outcome::Complete) => "complete",
                Some(brief::Outcome::Partial) => "partial",
                Some(brief::Outcome::Blocked) => "blocked",
                None => "missing (no parseable sushi-report)",
            },
            outcome
                .error
                .as_deref()
                .map(|e| format!("; the harness ended with an error: {}", tail_chars(e, 400)))
                .unwrap_or_default()
        );
        // What the implementer says it did and decided (e.g. "no lesson
        // this time", "ran the desktop smoke"): the reviewer weighs these
        // claims against the evidence instead of never hearing them.
        let agent_decisions = report
            .as_ref()
            .map(|r| brief::agent_decision_lines(&r.decisions))
            .unwrap_or_default();
        let implementer_note = match report.as_ref() {
            Some(r) if !r.summary.trim().is_empty() => {
                let account = r.summary.trim().to_string();
                format!(
                    "{implementer_note}\n\n{}",
                    brief::untrusted_block(
                        "The implementer's own account, a claim to check against the diff and verify results",
                        &account
                    )
                )
            }
            _ => implementer_note,
        };
        task.attempts[idx].summary = report.as_ref().map(|r| r.summary.clone());
        task.attempts[idx].disputes = report
            .as_ref()
            .map(|r| r.disputes.clone())
            .unwrap_or_default();
        task.attempts[idx].handoff = report
            .as_ref()
            .map(|r| r.handoff.trim().to_string())
            .filter(|h| !h.is_empty());
        // Agent-reported decisions are trusted less than the owner's: strip
        // any leading "Owner:" the agent might have echoed back, prefix
        // with "Agent:", and never duplicate an identical entry.
        for entry in &agent_decisions {
            if !task.decisions.contains(entry) {
                task.decisions.push(entry.clone());
            }
        }

        let wt3 = worktree.clone();
        let base3 = task.work_base().to_string();
        let changed = tokio::task::spawn_blocking(move || {
            git::changed_files(&wt3, &base3).unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        task.attempts[idx].changed_files = changed.clone();
        record_diff_stat(&mut task, idx, &worktree).await;

        // The planner's tests and the files the checks name: a change fails
        // the attempt outright, no owner question, and the files go back.
        if let Some(detail) = guard_protected(&task, &worktree, &changed).await {
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                FailureKind::Protected,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue => {
                    drop(permit);
                    continue;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        if let Some(claim) = brief::parse_impossible(&final_text, task.criteria.len()) {
            // Straight to the owner: no retry, no triage.
            let question = impossible_question(&task, &claim);
            record_failure(&mut task, idx, FailureKind::Blocked, question.text.clone());
            task.attempts[idx].status = AttemptStatus::Blocked;
            task.question = Some(question.clone());
            task.status = TaskStatus::Waiting;
            task.updated_at = now_ms();
            match wait_for_answer(
                &app,
                &task_id,
                &mut task,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                None => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                Some(answer) => {
                    if let Some(n) = impossible_drop_target(&question, &answer) {
                        let settings = app.settings.read().unwrap().clone();
                        let origin = origin_of_last_answer(&task);
                        let _ = drop_criterion(&settings, &mut task, n, origin);
                        task.updated_at = now_ms();
                        app.save_task_logged(&task);
                        app.broadcast_task(&task);
                    }
                    drop(permit);
                    continue;
                }
            }
        }

        let outcome_blocked = report
            .as_ref()
            .map(|r| r.outcome == brief::Outcome::Blocked)
            .unwrap_or(false);

        if outcome_blocked {
            let question_text = report
                .as_ref()
                .map(|r| r.question.clone())
                .unwrap_or_default();
            // Routed through the same signature/budget accounting as any
            // other failure (spec item 8), so a recurring "blocked"
            // question can't loop forever.
            record_failure(&mut task, idx, FailureKind::Blocked, question_text.clone());
            let protected = app.settings.read().unwrap().protected_paths.clone();
            let should_continue = advance_after_failure(&mut task, attempt_budget, &protected);
            if !should_continue {
                // Same "attempts keep failing" escalation as
                // `fail_and_continue`'s, inlined because a
                // should_continue==true blocked report goes on to ask the
                // owner below instead of looping.
                match wait_for_exhausted_answer(
                    &app,
                    &task_id,
                    &mut task,
                    idx,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                {
                    Some(_) => {
                        attempt_budget += 2;
                        drop(permit);
                        continue;
                    }
                    None => {
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                }
            }

            // `No tool: <capability>`: a retry cannot give the agent a tool,
            // so the owner decides, and only the owner.
            if let Some(capability) = permissions::missing_tool_claim(&question_text) {
                task.question = Some(permissions::no_tool_question(&app, &task, &capability));
                task.status = TaskStatus::Waiting;
                task.updated_at = now_ms();
                let Some(answer) = wait_for_answer(
                    &app,
                    &task_id,
                    &mut task,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                else {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                };
                permissions::apply_no_tool_answer(&app, &mut task, &capability, &answer);
                task.updated_at = now_ms();
                app.save_task_logged(&task);
                app.broadcast_task(&task);
                drop(permit);
                continue;
            }
            task.question = Some(Question::new(
                question_text,
                vec![],
                QuestionKind::AgentQuestion,
                AskedBy::Implement,
            ));
            task.question.as_mut().unwrap().failure_signature = task.attempts[idx]
                .failure
                .as_ref()
                .map(|f| f.signature.clone());
            task.status = TaskStatus::Waiting;
            task.updated_at = now_ms();
            match wait_for_answer_with_triage(
                &app,
                &task_id,
                &mut task,
                idx,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                Some(_) => {
                    drop(permit);
                    continue;
                }
                None => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        // A usage or rate limit is not a failed attempt: the task pauses and
        // resumes by itself.
        if changed.is_empty() {
            if let Some((error, wait)) = outcome.error.as_deref().and_then(|e| {
                rate_limit_wait_secs(e, task.queue.rate_pauses, now_ms() / 1000).map(|w| (e, w))
            }) {
                let resumed = pause_for_rate_limit(
                    &app,
                    &task_id,
                    &mut task,
                    idx,
                    error,
                    wait,
                    &cancel,
                    &mut permit,
                )
                .await;
                drop(permit);
                if !resumed {
                    app.finish_task_loop(&task_id);
                    return;
                }
                continue;
            }
        }
        // Only a run without an error ends a series of limit pauses.
        if outcome.error.is_none() {
            task.queue.rate_pauses = 0;
        }

        if changed.is_empty() {
            let (kind, detail) = match &outcome.error {
                Some(error) => (FailureKind::Error, error.clone()),
                None => (FailureKind::NoDeliverable, "No files changed.".to_string()),
            };
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                kind,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue => {
                    drop(permit);
                    continue;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        if cancel.is_cancelled() {
            interrupt_attempt(&app, &mut task, idx);
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }

        // The base branch may have moved while the agent worked (another
        // task merged, the owner committed): carry the work onto it so verify
        // and review judge the tree that would actually land. Conflicts go
        // back to the agent as a failure; it resolves them, not this code.
        let attempt_start_base = base_sha.clone();
        let mut base_sha = base_sha;
        let mut changed = changed;
        if let Some(base_ref) = task.base_ref.clone() {
            let (wt, mut snapshot, r) = (worktree.clone(), task.clone(), base_ref.clone());
            let carried = tokio::task::spawn_blocking(move || {
                let carried = carry_with_planner_tests(&mut snapshot, &wt, &r)?;
                Ok((carried, snapshot.planner_tests))
            })
            .await
            .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
            let carried = carried.map(|(carried, tests)| {
                task.planner_tests = tests;
                carried
            });
            match carried {
                Ok(git::Rebase::Unchanged) => {}
                Ok(git::Rebase::Moved { new_sha, rewritten }) => {
                    task.decisions.push(if rewritten {
                        format!(
                            "Rebase: {base_ref} was rewritten; carried the work's own changes onto it at {}",
                            short_sha(&new_sha)
                        )
                    } else {
                        format!(
                            "Rebase: carried the work onto {base_ref} at {}",
                            short_sha(&new_sha)
                        )
                    });
                    task.base_sha = new_sha.clone();
                    base_sha = new_sha;
                    let (wt, b) = (worktree.clone(), task.work_base().to_string());
                    changed = tokio::task::spawn_blocking(move || {
                        git::changed_files(&wt, &b).unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default();
                    task.attempts[idx].changed_files = changed.clone();
                    record_diff_stat(&mut task, idx, &worktree).await;
                    if changed.is_empty() {
                        // The base already holds this work: nothing left to commit.
                        task.decisions.push(format!(
                            "Rebase: {base_ref} already contains this work; nothing to commit"
                        ));
                        task.attempts[idx].status = AttemptStatus::Passed;
                        task.attempts[idx].ended_at = Some(now_ms());
                        task.status = TaskStatus::Done;
                        task.updated_at = now_ms();
                        app.write_report(&mut task).await;
                        app.release_worktree(&mut task, "task done").await;
                        task.updated_at = now_ms();
                        app.save_task_logged(&task);
                        app.broadcast_task(&task);
                        wake_orchestrator_for_done(&app, &mut task);
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                }
                Ok(git::Rebase::Skipped { reason }) => {
                    let note = format!("Rebase: not carried onto {base_ref} ({reason})");
                    if !task.decisions.contains(&note) {
                        task.decisions.push(note);
                    }
                }
                Ok(git::Rebase::Conflicts {
                    new_sha,
                    files,
                    rewritten,
                }) => {
                    if rewritten {
                        task.decisions
                            .push(rebase_conflict_note(&base_ref, &new_sha, &files, rewritten));
                    }
                    task.base_sha = new_sha.clone();
                    task.queue.carry_conflicts = files.clone();
                    let detail = conflict_detail(&base_ref, &new_sha, &files, &task_id);
                    match fail_and_continue(
                        &app,
                        &task_id,
                        &mut task,
                        idx,
                        FailureKind::Conflict,
                        detail,
                        &mut attempt_budget,
                        &pending_answer,
                        &cancel,
                        &mut permit,
                    )
                    .await
                    {
                        LoopSignal::Continue => {
                            drop(permit);
                            continue;
                        }
                        LoopSignal::Stop => {
                            drop(permit);
                            app.finish_task_loop(&task_id);
                            return;
                        }
                    }
                }
                Err(e) => {
                    // carry_onto_moved_base restored the worktree, so the old
                    // base is still what the diff is against.
                    let note = format!(
                        "Rebase: could not carry the work onto {base_ref} ({e}); verifying on the old base"
                    );
                    if !task.decisions.contains(&note) {
                        task.decisions.push(note);
                    }
                }
            }
        }

        let mut stale_evidence: Option<(u32, Vec<String>)> = None;
        {
            let since = task.attempts[idx].started_at;
            let scope = evidence_scope(&task);
            let saved = save_evidence(&worktree, since, &run_dir.join("evidence"), &scope);
            let n = task.attempts[idx].n;
            if !saved.is_empty() {
                task.attempts[idx].evidence = saved;
                let wt = worktree.clone();
                task.attempts[idx].evidence_tree =
                    tokio::task::spawn_blocking(move || git::worktree_tree(&wt).ok())
                        .await
                        .unwrap_or(None);
                task.attempts[idx].evidence_base = Some(task.base_sha.clone());
                app.save_task_logged(&task);
                app.broadcast_task(&task);
            } else if !task.visual_criteria_texts().is_empty() {
                let source = task
                    .attempts
                    .iter()
                    .filter(|a| {
                        a.stage == Stage::Implement
                            && a.n < n
                            && a.evidence_from.is_none()
                            && !a.evidence.is_empty()
                    })
                    .filter_map(|a| {
                        Some((
                            a.n,
                            a.evidence_tree.clone()?,
                            a.evidence_base.clone(),
                            a.evidence.clone(),
                        ))
                    })
                    .next_back();
                if let Some((m, old_tree, old_base, old_evidence)) = source {
                    let old_evidence: Vec<String> = old_evidence
                        .into_iter()
                        .filter(|e| scope.allows_saved(e))
                        .collect();
                    let wt = worktree.clone();
                    let now_base = task.base_sha.clone();
                    let diff = tokio::task::spawn_blocking(move || {
                        crate::engine::healing::own_ui_changes_since(
                            &wt,
                            &old_tree,
                            old_base.as_deref(),
                            &now_base,
                        )
                    })
                    .await
                    .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
                    if let Ok(other) = diff {
                        if other.is_empty() && !old_evidence.is_empty() {
                            task.attempts[idx].evidence = old_evidence;
                            task.attempts[idx].evidence_from = Some(m);
                            task.decisions.push(format!(
                                "Orchestrator: attempt {n} changed no UI file of its own since attempt {m} (artifacts, tests and carried-in base commits do not count); reused attempt {m}'s evidence"
                            ));
                            app.save_task_logged(&task);
                            app.broadcast_task(&task);
                        } else {
                            stale_evidence = Some((m, other.into_iter().take(5).collect()));
                        }
                    }
                }
            }
        }
        apply_pending_amendment(&app, &mut task, &pending_amend, &cancel).await;
        // A merged task's checklist runs before the full verify: a failing
        // item fails the attempt by name, without the slow commands.
        if let Some(detail) = plan_gate::run_checklist(
            &app,
            &mut task,
            &worktree,
            &run_dir.join("checklist"),
            &cancel,
        )
        .await
        {
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                FailureKind::Verify,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue => {
                    drop(permit);
                    continue 'attempts;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }
        if cancel.is_cancelled() {
            interrupt_attempt(&app, &mut task, idx);
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }
        let (verify_commands, skipped) =
            scope_task_checks(&app, &task, &worktree, &base_sha, &task.verify).await;
        record_check_decisions(&mut task, skipped);
        let (mut verify_results, cache_line) = run_verify_cached(
            &app,
            &task_id,
            &task.repo,
            &worktree,
            &run_dir,
            &base_sha,
            &verify_commands,
            &cancel,
        )
        .await;
        record_check_decisions(&mut task, cache_line);
        if cancel.is_cancelled() {
            // Killed by the shutdown, not failed: the next daemon re-runs it.
            interrupt_attempt(&app, &mut task, idx);
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }
        record_check_outcomes(&app, &mut task, &verify_results);
        task.attempts[idx].verify = verify_results.clone();
        let mut preflight_checked = false;
        while let Some(failed) = verify_results.iter().find(|v| v.code != Some(0)).cloned() {
            // The base preflight may rule on this very command (it cannot
            // run here, or fails for an unrelated reason): its ruling comes
            // before the failure counts. A changed command runs again, so a
            // rewrite spends no counted attempt.
            if !preflight_checked && !timed_out_under_load(&failed) {
                preflight_checked = true;
                let before = task.verify.clone();
                if !settle_base_preflight(&app, &mut task, &cancel).await {
                    interrupt_attempt(&app, &mut task, idx);
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                if task.verify != before {
                    let (verify_commands, skipped) =
                        scope_task_checks(&app, &task, &worktree, &base_sha, &task.verify).await;
                    record_check_decisions(&mut task, skipped);
                    let (results, cache_line) = run_verify_cached(
                        &app,
                        &task_id,
                        &task.repo,
                        &worktree,
                        &run_dir,
                        &base_sha,
                        &verify_commands,
                        &cancel,
                    )
                    .await;
                    verify_results = results;
                    record_check_decisions(&mut task, cache_line);
                    if cancel.is_cancelled() {
                        interrupt_attempt(&app, &mut task, idx);
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                    record_check_outcomes(&app, &mut task, &verify_results);
                    task.attempts[idx].verify = verify_results.clone();
                    continue;
                }
            }
            // The work was carried onto a base that moved during this
            // attempt: a command that passed on the old base and fails on
            // the new one alone is the base's failure, not the attempt's.
            if attempt_start_base != task.base_sha
                && !base_check_answered(&task, &failed.command, &task.base_sha)
            {
                let base_dir = run_dir.join("verify-base");
                if let Some(base_run) = broken_by_the_new_base(
                    &app,
                    &task,
                    &failed,
                    &attempt_start_base,
                    &base_dir,
                    &cancel,
                )
                .await
                {
                    match wait_on_broken_base(
                        &app,
                        &task_id,
                        &mut task,
                        idx,
                        &failed,
                        &base_run,
                        &mut attempt_budget,
                        &pending_answer,
                        &cancel,
                        &mut permit,
                    )
                    .await
                    {
                        BaseBreak::Stopped => {
                            drop(permit);
                            app.finish_task_loop(&task_id);
                            return;
                        }
                        BaseBreak::Retry => {
                            drop(permit);
                            continue 'attempts;
                        }
                        BaseBreak::Dropped => {
                            verify_results.retain(|v| v.command != failed.command);
                            record_check_outcomes(&app, &mut task, &verify_results);
                            task.attempts[idx].verify = verify_results.clone();
                            continue;
                        }
                    }
                }
            }
            let detail = format!(
                "{} exited {}.\n{}",
                failed.command,
                failed
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                failed.tail
            );
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                FailureKind::Verify,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue => {
                    drop(permit);
                    continue 'attempts;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        // Never through the verify cache: it holds the verify commands'
        // results only.
        let mut failure: Option<(FailureKind, String)> = None;
        let gated = gated_checks(&task);
        if !gated.is_empty() {
            let commands: Vec<String> = gated.iter().map(|c| c.run.clone()).collect();
            let sandbox = app.task_sandbox(&task.repo, &worktree, &run_dir);
            let results = run_verify_commands(
                CheckContext::new(&app, Some(&task.id)),
                &worktree,
                &run_dir.join("checks"),
                &commands,
                sandbox.as_ref(),
                &task.base_sha,
                &cancel,
            )
            .await;
            record_check_outcomes(&app, &mut task, &results);
            if cancel.is_cancelled() {
                interrupt_attempt(&app, &mut task, idx);
                drop(permit);
                app.finish_task_loop(&task_id);
                return;
            }
            task.attempts[idx].verify.extend(results.iter().cloned());
            verify_results.extend(results.iter().cloned());
            if let Some(failed) = results.iter().find(|v| v.code != Some(0)) {
                failure = Some((
                    FailureKind::Verify,
                    format!(
                        "{} exited {}.\n{}",
                        failed.command,
                        failed
                            .code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "null".to_string()),
                        failed.tail
                    ),
                ));
            }
        }
        if failure.is_none() {
            if let Some(held) = gated_held_out(&task) {
                let (result, detail) = run_held_out(
                    CheckContext::new(&app, Some(&task.id)),
                    &task,
                    &held,
                    &worktree,
                    &run_dir.join("checks"),
                    app.task_sandbox(&task.repo, &worktree, &run_dir).as_ref(),
                    &cancel,
                )
                .await;
                record_check_outcomes(&app, &mut task, std::slice::from_ref(&result));
                if cancel.is_cancelled() {
                    interrupt_attempt(&app, &mut task, idx);
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                task.attempts[idx].verify.push(result.clone());
                verify_results.push(result);
                failure = detail.map(|d| (FailureKind::Heldout, d));
            }
        }
        refresh_criteria_results(&mut task, idx);
        if let Some((kind, detail)) = failure {
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                kind,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue => {
                    drop(permit);
                    continue;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        // The planner named the command that captures the images: orchd
        // runs it itself, so an attempt is never failed for a missing image.
        let sandbox = app.task_sandbox(&task.repo, &worktree, &run_dir);
        capture_screenshots(
            &app,
            &mut task,
            idx,
            &worktree,
            &run_dir,
            sandbox.as_ref(),
            &cancel,
        )
        .await;
        if cancel.is_cancelled() {
            interrupt_attempt(&app, &mut task, idx);
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }
        let visual: Vec<&str> = task.visual_criteria_texts();
        let scope = evidence_scope(&task);
        let missing_named = scope.missing(&task.attempts[idx].evidence);
        let evidence_missing = match scope {
            EvidenceScope::Named(_) => !missing_named.is_empty(),
            _ => task.attempts[idx].evidence.is_empty(),
        };
        if !visual.is_empty() && evidence_missing && task.brief_check.screenshot.is_none() {
            // Nothing in the repo can take the picture: failing the attempt
            // would only burn it on a gate it cannot pass. Say so once, hand
            // the criteria to the report as follow-ups, and let review rule
            // on what it can.
            let note = format!(
                "Orchestrator: no screenshot command is set and no image was saved, so the visual criteria are judged without images: {}",
                visual.join("; ")
            );
            let follow: Vec<String> = visual
                .iter()
                .map(|c| format!("Check by eye, no image evidence was captured: {c}"))
                .collect();
            if !task.decisions.contains(&note) {
                task.decisions.push(note);
                add_follow_ups(&mut task, follow);
                app.save_task_logged(&task);
                app.broadcast_task(&task);
            }
        } else if !visual.is_empty() && evidence_missing {
            let criteria = visual
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n");
            let command = task.brief_check.screenshot.clone().unwrap_or_default();
            let mut detail = if missing_named.is_empty() {
                format!(
                    "The screenshot command `{command}` ran, but these criteria are visual and no image was saved under artifacts/ during this attempt:\n{criteria}\nSave one screenshot per criterion under artifacts/ (for example artifacts/<name>.png) and look at it before finishing."
                )
            } else {
                format!(
                    "The screenshot command `{command}` ran, but these criteria are visual and the images they name were not saved during this attempt: {}\n{criteria}\nSave each named file and look at it before finishing; other images under artifacts/ are not evidence.",
                    missing_named.join(", ")
                )
            };
            if let Some((m, paths)) = &stale_evidence {
                detail.push_str(&format!(
                    "\nAttempt {m}'s images were not reused because these files changed since: {}",
                    paths.join(", ")
                ));
            }
            match fail_or_accept(
                &app,
                &task_id,
                &mut task,
                idx,
                FailureKind::Evidence,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                FailSignal::Continue => {
                    drop(permit);
                    continue;
                }
                FailSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                FailSignal::Accept => {
                    match finish_after_review(
                        &app,
                        &task_id,
                        &mut task,
                        idx,
                        attempt_n,
                        &worktree,
                        &run_dir,
                        &settings,
                        None,
                        &mut attempt_budget,
                        &pending_answer,
                        &cancel,
                        &mut permit,
                    )
                    .await
                    {
                        Tail::Continue => {
                            continue;
                        }
                        Tail::Return => return,
                    }
                }
            }
        }

        if let Some(path) = changed
            .iter()
            .find(|f| matches_any_protected(f, &settings.protected_paths))
        {
            task.attempts[idx].status = AttemptStatus::Blocked;
            task.attempts[idx].ended_at = Some(now_ms());
            task.question = Some(Question::new(
                format!("Change touches protected path {path}: approve or reject?"),
                vec!["approve".into(), "reject".into()],
                QuestionKind::ProtectedPath,
                AskedBy::Implement,
            ));
            task.status = TaskStatus::Waiting;
            task.updated_at = now_ms();
            match wait_for_answer(
                &app,
                &task_id,
                &mut task,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                None => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                Some(answer) => {
                    // Proceeds only on an exact "approve"; anything else --
                    // "reject", a typo, free text -- rejects the change.
                    if answer != "approve" {
                        match fail_and_continue(
                            &app,
                            &task_id,
                            &mut task,
                            idx,
                            FailureKind::Protected,
                            format!("Owner rejected change to protected path {path}"),
                            &mut attempt_budget,
                            &pending_answer,
                            &cancel,
                            &mut permit,
                        )
                        .await
                        {
                            LoopSignal::Continue => {
                                drop(permit);
                                continue;
                            }
                            LoopSignal::Stop => {
                                drop(permit);
                                app.finish_task_loop(&task_id);
                                return;
                            }
                        }
                    }
                    // approved: fall through to review/commit
                }
            }
        }

        apply_pending_amendment(&app, &mut task, &pending_amend, &cancel).await;
        if heal_answers(&app, &mut task, attempt_n, &cancel)
            .await
            .is_err()
        {
            interrupt_attempt(&app, &mut task, idx);
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }
        let mut review_result: Option<ReviewResult> = None;
        let mut final_task = None;
        let review_id = task.variant().review_route_id(&settings).to_string();
        if !review_id.is_empty() {
            let selected = select_review_route_with(&settings, &review_id, &route, task.tier)
                .and_then(|(r, why)| {
                    app.usable_route(&settings, r.clone())
                        .map(|(r, swap)| (r, why, swap))
                });
            if let Some((review_route, why, swap)) = selected {
                let review_route = &review_route;
                let note = format!("Orchestrator: review: {} ({why})", review_route.id);
                for line in swap.into_iter().chain([note]) {
                    if !task.decisions.contains(&line) {
                        task.decisions.push(line);
                    }
                }
                if !wait_while_over_budget(
                    &app,
                    &task_id,
                    &mut task,
                    "review",
                    TaskStatus::Running,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                {
                    // Stopped at the gate: the attempt ends here too, or a
                    // restart would find it `running` and requeue the task.
                    if let Ok(Some(mut t)) = app.store.load_task(&task_id) {
                        if app.leaves_run_going(&t, idx) {
                            drop(permit);
                            app.finish_task_loop(&task_id);
                            return;
                        }
                        if let Some(a) = t.attempts.get_mut(idx) {
                            a.status = AttemptStatus::Interrupted;
                            a.ended_at = Some(now_ms());
                        }
                        t.updated_at = now_ms();
                        app.save_task_logged(&t);
                        app.broadcast_task(&t);
                    }
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                // The preflight can drop or rewrite a final command even
                // when fast verification passed. Settle it before cloning
                // the commands or a stale failure could discard the review.
                if !settle_base_preflight(&app, &mut task, &cancel).await {
                    interrupt_attempt(&app, &mut task, idx);
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                let mut review_cost = 0.0;
                let finals =
                    BackgroundFinal::start(&app, &task, &worktree, &run_dir, &settings, &cancel);
                let review_cancel = finals.review_cancel.clone();
                final_task = Some(finals);
                let mut review_fingerprint = None;
                // Every review run of this attempt: the first, the one
                // automatic re-run after a reply with no verdict, and any the
                // owner or the policy asks for with `retry`. Same diff, no new
                // implement attempt.
                let mut round = 1u32;
                let mut after_no_verdict = false;
                let mut auto_rerun = true;
                // Findings a judge dropped in this attempt.
                let mut dropped: Vec<(String, String)> = Vec::new();
                // After a narrow PASS that left files no open finding names,
                // every later round of this attempt is a full review; the
                // escalation happens at most once.
                let mut force_full = false;
                loop {
                    if review_cancel.is_cancelled() && !cancel.is_cancelled() {
                        break;
                    }
                    let reviewed = run_review(
                        &app,
                        &task_id,
                        attempt_n,
                        &mut task,
                        &worktree,
                        &base_sha,
                        &verify_results,
                        &implementer_note,
                        &agent_decisions,
                        review_route,
                        &deny_read,
                        &review_cancel,
                        &mut review_cost,
                        &mut review_fingerprint,
                        after_no_verdict,
                        &dropped,
                        // Reserve the last review run for the whole change.
                        force_full || round == MAX_REVIEW_RUNS,
                    )
                    .await;
                    // The task's total, and separately the attempt's
                    // review_cost_usd for display: an attempt's own cost_usd is
                    // what later resumes of its session subtract
                    // (session cost accounting), so the review's cost must never join it.
                    task.cost_usd += review_cost;
                    if review_fingerprint.is_some() {
                        task.attempts[idx].review_fingerprint = review_fingerprint.take();
                    }
                    task.attempts[idx].review_cost_usd =
                        Some(task.attempts[idx].review_cost_usd.unwrap_or(0.0) + review_cost);
                    review_cost = 0.0;
                    if review_cancel.is_cancelled() && !cancel.is_cancelled() {
                        break;
                    }
                    let (why, no_verdict) = match reviewed {
                        Ok((mut r, recorded_as_pass, saw)) => {
                            task.attempts[idx].review_tree = saw.tree.clone();
                            task.attempts[idx].review_base = Some(base_sha.clone());
                            task.attempts[idx].review_scope = Some(saw.scope);
                            if r.verdict == Verdict::Pass
                                && saw.scope == ReviewScope::Narrow
                                && !force_full
                            {
                                let unnamed = unnamed_changes_since_full_review(
                                    &task,
                                    attempt_n,
                                    saw.tree.as_deref(),
                                    &worktree,
                                    &base_sha,
                                )
                                .await;
                                if !unnamed.is_empty() {
                                    task.decisions.push(format!(
                                        "Orchestrator: attempt {attempt_n} narrow review passed, but {} changed since the last full review and no open finding names {}; reviewing the whole change once",
                                        unnamed.join(", "),
                                        if unnamed.len() == 1 { "it" } else { "them" }
                                    ));
                                    force_full = true;
                                    round += 1;
                                    after_no_verdict = false;
                                    continue;
                                }
                            }
                            move_minor_findings(&mut task, attempt_n, &mut r);
                            register_findings(&mut task, attempt_n, round, &mut r);
                            if recorded_as_pass {
                                task.decisions.push(
                                    "Orchestrator: review FAIL recorded as PASS (only P2/P3 findings)"
                                        .to_string(),
                                );
                            }
                            if r.verdict == Verdict::Fail {
                                let task_tier = task.tier;
                                let mut newly = handle_repeated_findings(
                                    &app,
                                    &mut task,
                                    idx,
                                    attempt_n,
                                    &mut r,
                                    &implementer_note,
                                    &settings,
                                    review_route,
                                    &route,
                                    &worktree,
                                    &cancel,
                                )
                                .await;
                                if cancel.is_cancelled() {
                                    interrupt_attempt(&app, &mut task, idx);
                                    drop(permit);
                                    app.finish_task_loop(&task_id);
                                    return;
                                }
                                if newly.is_empty()
                                    && !r.repeated.is_empty()
                                    && !task.attempts[idx].disputes.is_empty()
                                {
                                    let judge_route = task.variant().judge_route;
                                    newly = judge_disputed_findings(
                                        &app,
                                        &mut task,
                                        idx,
                                        attempt_n,
                                        &r.repeated,
                                        None,
                                        &settings,
                                        judge_route.as_deref(),
                                        review_route,
                                        &route,
                                        task_tier,
                                        &cancel,
                                    )
                                    .await;
                                }
                                if !newly.is_empty() {
                                    dropped.extend(newly);
                                    if round < MAX_REVIEW_RUNS {
                                        round += 1;
                                        after_no_verdict = false;
                                        continue;
                                    }
                                    task.decisions.push(review_stopped_note(attempt_n));
                                    review_result = Some(r);
                                    break;
                                }
                                if review_cancel.is_cancelled() && !cancel.is_cancelled() {
                                    break;
                                }
                                match heal_repeated_unmet(&app, &mut task, idx, &r, &cancel).await {
                                    Ok(true) => {
                                        task.updated_at = now_ms();
                                        app.save_task_logged(&task);
                                        app.broadcast_task(&task);
                                        if round < MAX_REVIEW_RUNS {
                                            round += 1;
                                            continue;
                                        }
                                        task.decisions.push(review_stopped_note(attempt_n));
                                    }
                                    Ok(false) => {}
                                    Err(_) => {
                                        interrupt_attempt(&app, &mut task, idx);
                                        drop(permit);
                                        app.finish_task_loop(&task_id);
                                        return;
                                    }
                                }
                            }
                            review_result = Some(r);
                            break;
                        }
                        Err(ReviewFailure::Cancelled) => {
                            if !cancel.is_cancelled() {
                                break;
                            }
                            interrupt_attempt(&app, &mut task, idx);
                            if let Some(finals) = final_task.take() {
                                let _ = finals.finish().await;
                            }
                            drop(permit);
                            app.finish_task_loop(&task_id);
                            return;
                        }
                        Err(ReviewFailure::NoVerdict(why)) => (why, true),
                        Err(ReviewFailure::Harness(why)) => (why, false),
                    };
                    if no_verdict && auto_rerun && round < MAX_REVIEW_RUNS {
                        auto_rerun = false;
                        round += 1;
                        after_no_verdict = true;
                        task.decisions.push(format!(
                            "Orchestrator: attempt {attempt_n} review reply had no verdict; reviewing the same attempt again"
                        ));
                        continue;
                    }
                    if review_cancel.is_cancelled() && !cancel.is_cancelled() {
                        break;
                    }
                    // No verdict is never a PASS: the owner decides whether
                    // to commit this attempt unreviewed.
                    // The last run the attempt may have: no `retry` to offer.
                    let may_retry = round < MAX_REVIEW_RUNS;
                    if !may_retry {
                        task.decisions.push(review_stopped_note(attempt_n));
                    }
                    task.attempts[idx].status = AttemptStatus::Blocked;
                    task.attempts[idx].ended_at = Some(now_ms());
                    task.question = Some(Question::new(
                        format!(
                            "The review gave no verdict ({why}). Commit this attempt unreviewed?"
                        ),
                        if may_retry {
                            vec!["approve".into(), "retry".into()]
                        } else {
                            vec!["approve".into()]
                        },
                        QuestionKind::ReviewNoVerdict,
                        AskedBy::Review,
                    ));
                    task.status = TaskStatus::Waiting;
                    task.updated_at = now_ms();
                    match wait_for_answer(
                        &app,
                        &task_id,
                        &mut task,
                        &pending_answer,
                        &cancel,
                        &mut permit,
                    )
                    .await
                    {
                        None => {
                            drop(permit);
                            app.finish_task_loop(&task_id);
                            return;
                        }
                        Some(answer) if answer == "approve" => {
                            task.decisions.push(format!(
                                "Orchestrator: attempt {attempt_n} committed without a review verdict"
                            ));
                            break;
                        }
                        Some(answer) if answer == "retry" && may_retry => {
                            // Review the same attempt again, not a new one.
                            task.attempts[idx].status = AttemptStatus::Running;
                            task.attempts[idx].ended_at = None;
                            round += 1;
                            after_no_verdict = no_verdict;
                            continue;
                        }
                        Some(answer) => {
                            review_result = Some(ReviewResult {
                                verdict: Verdict::Fail,
                                findings: vec![format!("Another attempt was asked for: {answer}")],
                                repeated: Vec::new(),
                                severities: Vec::new(),
                                details: Vec::new(),
                                criteria: Vec::new(),
                            });
                            break;
                        }
                    }
                }
            } else {
                let note = format!(
                    "Orchestrator: review skipped, no route matches review \"{}\"",
                    review_id
                );
                if !task.decisions.contains(&note) {
                    task.decisions.push(note);
                }
            }
        }
        let mut final_failed = final_task
            .as_ref()
            .is_some_and(|f| f.review_cancel.is_cancelled())
            && !cancel.is_cancelled();
        if final_failed {
            review_result = None;
        }
        let mut review_follow_ups = Vec::new();
        if let Some(r) = review_result.as_mut() {
            // After the first attempt only P1+ findings about the task's own
            // work cost an attempt; the rest go to the report.
            if r.verdict == Verdict::Fail && implement_attempt_count(&task) >= 2 {
                let was = r.findings.len();
                let follow_ups = scope_review(&task, idx, &changed, r);
                if !follow_ups.is_empty() {
                    task.decisions.push(if r.verdict == Verdict::Pass {
                        format!(
                            "Orchestrator: review FAIL of attempt {attempt_n} recorded as PASS: its {was} finding(s) are P2/P3 or unrelated to this task, kept as follow-ups"
                        )
                    } else {
                        format!(
                            "Orchestrator: attempt {attempt_n}'s review: {} finding(s) are P2/P3 or unrelated to this task and are follow-ups, not blockers",
                            follow_ups.len()
                        )
                    });
                    review_follow_ups.extend(follow_ups);
                }
            } else if r.verdict == Verdict::Pass {
                review_follow_ups.extend(r.findings.clone());
            }
        }
        let finished = match final_task.take() {
            Some(finals)
                if !final_failed
                    && !finals.review_cancel.is_cancelled()
                    && review_result
                        .as_ref()
                        .is_some_and(|r| r.verdict == Verdict::Fail) =>
            {
                // Only the scoped, judged and healed verdict can stop the checks.
                finals.cancel.cancel();
                let _ = finals.finish().await;
                task.decisions.push(format!(
                    "Orchestrator: attempt {attempt_n} review failed; cancelled the final checks"
                ));
                None
            }
            Some(finals) => finals.finish().await,
            None => None,
        };
        if cancel.is_cancelled() {
            interrupt_attempt(&app, &mut task, idx);
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }
        final_failed |= finished.as_ref().is_some_and(|f| {
            f.results
                .iter()
                .any(|r| r.code.is_some_and(|code| code != 0))
        });
        if final_failed {
            task.decisions.push(format!(
                "Orchestrator: attempt {attempt_n} review discarded because a final check failed"
            ));
            review_result = None;
            task.attempts[idx].review_tree = None;
            task.attempts[idx].review_base = None;
            task.attempts[idx].review_scope = None;
        } else {
            add_follow_ups(&mut task, review_follow_ups);
        }
        task.attempts[idx].review = review_result.clone();
        refresh_criteria_results(&mut task, idx);

        if let Some(r) = &review_result {
            if r.verdict == Verdict::Fail {
                match fail_or_accept(
                    &app,
                    &task_id,
                    &mut task,
                    idx,
                    FailureKind::Review,
                    r.findings.join("; "),
                    &mut attempt_budget,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                {
                    FailSignal::Continue => {
                        drop(permit);
                        continue;
                    }
                    FailSignal::Stop => {
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                    FailSignal::Accept => {
                        match finish_after_review(
                            &app,
                            &task_id,
                            &mut task,
                            idx,
                            attempt_n,
                            &worktree,
                            &run_dir,
                            &settings,
                            None,
                            &mut attempt_budget,
                            &pending_answer,
                            &cancel,
                            &mut permit,
                        )
                        .await
                        {
                            Tail::Continue => {
                                continue;
                            }
                            Tail::Return => return,
                        }
                    }
                }
            }
        }

        match finish_after_review(
            &app,
            &task_id,
            &mut task,
            idx,
            attempt_n,
            &worktree,
            &run_dir,
            &settings,
            finished,
            &mut attempt_budget,
            &pending_answer,
            &cancel,
            &mut permit,
        )
        .await
        {
            Tail::Continue => {
                continue;
            }
            Tail::Return => return,
        }
    }
}

pub(super) fn resume_attempt<'a>(
    task: &'a Task,
    route: &Route,
    conflict_only: bool,
    best_of: bool,
) -> Option<&'a Attempt> {
    if conflict_only || best_of || last_review_repeated(task) {
        return None;
    }
    task.attempts
        .iter()
        .rfind(|a| a.stage == Stage::Implement)
        .filter(|a| {
            a.failure
                .as_ref()
                .is_some_and(|f| f.kind == FailureKind::Verify)
                && a.session_id.as_ref().is_some_and(|id| !id.is_empty())
                && a.route_id == route.id
                && a.harness == route.harness
                && a.model == route.model.as_deref().unwrap_or_default()
        })
}

/// The files the task changed since the last full review that no open finding
/// of the review the narrow one built on names, by `path:line`. Empty when
/// nothing is unnamed or it cannot be told (no full review with a tree, a git
/// error).
async fn unnamed_changes_since_full_review(
    task: &Task,
    attempt_n: u32,
    now_tree: Option<&str>,
    worktree: &Path,
    base_sha: &str,
) -> Vec<String> {
    let Some(now_tree) = now_tree else {
        return Vec::new();
    };
    let Some(open) = narrow_basis(task, attempt_n).and_then(|a| a.review.as_ref()) else {
        return Vec::new();
    };
    let Some((full_tree, full_base)) = task
        .attempts
        .iter()
        .rev()
        .filter(|a| a.stage == Stage::Implement && a.n < attempt_n)
        .find(|a| a.review_scope == Some(ReviewScope::Full) && a.review_tree.is_some())
        .and_then(|a| Some((a.review_tree.clone()?, a.review_base.clone())))
    else {
        return Vec::new();
    };
    let named = brief::finding_paths(&open.findings);
    let (wt, now, base) = (
        worktree.to_path_buf(),
        now_tree.to_string(),
        base_sha.to_string(),
    );
    tokio::task::spawn_blocking(move || {
        let files = match &full_base {
            Some(old_base) => git::own_changes_between(&wt, &full_tree, old_base, &now, &base),
            None => git::diff_trees_name_only(&wt, &full_tree, &now),
        }
        .unwrap_or_default();
        files
            .into_iter()
            .filter(|f| !named.contains(f))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default()
}

/// Records the worktree's diff against the task's work base on the attempt and the task.
async fn record_diff_stat(task: &mut Task, idx: usize, worktree: &Path) {
    let (wt, base) = (worktree.to_path_buf(), task.work_base().to_string());
    let stat = tokio::task::spawn_blocking(move || git::worktree_diff_stat(&wt, &base).ok())
        .await
        .unwrap_or(None);
    if stat.is_some() {
        task.attempts[idx].diff_stat = stat;
        task.diff_stat = stat;
    }
}

/// Decision line for a carry that stopped on conflicts.
fn rebase_conflict_note(
    base_ref: &str,
    new_sha: &str,
    files: &[String],
    rewritten: bool,
) -> String {
    if rewritten {
        format!(
            "Rebase: {base_ref} was rewritten; carried the work's own changes onto it at {} with conflicts in {}",
            short_sha(new_sha),
            files.join(", ")
        )
    } else {
        format!(
            "Rebase: carried the work onto {base_ref} at {} with conflicts in {}",
            short_sha(new_sha),
            files.join(", ")
        )
    }
}

pub(super) enum BaseCarry {
    Ready,
    Conflict(String),
    Failed(String),
}

/// Carries committed and uncommitted task work (and its trusted planner test
/// commit) onto the current base head.
pub(super) async fn carry_onto_newest_base(task: &mut Task, worktree: &Path) -> BaseCarry {
    let Some(base_ref) = task.base_ref.clone() else {
        let reason = "Rebase: cannot carry the work without a base branch".to_string();
        task.decisions.push(reason.clone());
        return BaseCarry::Failed(reason);
    };
    let (wt, mut snapshot, r) = (worktree.to_path_buf(), task.clone(), base_ref.clone());
    let carried = tokio::task::spawn_blocking(move || {
        let from = snapshot.base_sha.clone();
        let new_sha = git::resolve_commit(&wt, &r)?;
        if new_sha == from {
            return Ok((git::Rebase::Unchanged, false, snapshot.planner_tests));
        }
        // Preserve a merge the task already made; resetting it would carry
        // the base's changes a second time along with the task's own work.
        if git::is_ancestor(&wt, &new_sha, "HEAD") == Some(true) {
            return Ok((
                git::Rebase::Moved {
                    new_sha,
                    rewritten: false,
                },
                true,
                snapshot.planner_tests,
            ));
        }
        // Work committed by an attempt goes back to uncommitted, down to the
        // trusted planner test commit when there is one.
        let keep = snapshot
            .planner_tests
            .commit
            .clone()
            .filter(|c| *c != from && git::is_ancestor(&wt, c, "HEAD") == Some(true));
        let target = keep.unwrap_or_else(|| from.clone());
        if git::head_sha(&wt)? != target {
            git::uncommit_to(&wt, &target)?;
        }
        let carried = carry_with_planner_tests(&mut snapshot, &wt, &r)?;
        Ok((carried, false, snapshot.planner_tests))
    })
    .await
    .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
    match carried {
        Ok((git::Rebase::Unchanged, _, _)) => BaseCarry::Ready,
        Ok((git::Rebase::Moved { new_sha, rewritten }, already_contained, tests)) => {
            task.planner_tests = tests;
            task.decisions.push(if already_contained {
                format!(
                    "Rebase: the task branch already contains {base_ref} at {}; moved base_sha to that head",
                    short_sha(&new_sha)
                )
            } else if rewritten {
                format!(
                    "Rebase: {base_ref} was rewritten; carried the work's own changes onto it at {}",
                    short_sha(&new_sha)
                )
            } else {
                format!(
                    "Rebase: carried the work onto {base_ref} at {}",
                    short_sha(&new_sha)
                )
            });
            task.base_sha = new_sha;
            BaseCarry::Ready
        }
        Ok((
            git::Rebase::Conflicts {
                new_sha,
                files,
                rewritten,
            },
            _,
            tests,
        )) => {
            task.planner_tests = tests;
            task.decisions
                .push(rebase_conflict_note(&base_ref, &new_sha, &files, rewritten));
            let detail = conflict_detail(&base_ref, &new_sha, &files, &task.id);
            task.queue.carry_conflicts = files;
            task.base_sha = new_sha;
            BaseCarry::Conflict(detail)
        }
        Ok((git::Rebase::Skipped { reason }, _, _)) => {
            let detail = format!("Rebase: not carried onto {base_ref}: {reason}");
            task.decisions.push(detail.clone());
            BaseCarry::Failed(detail)
        }
        Err(e) => {
            let detail = format!("Rebase: carrying onto {base_ref} failed: {e}");
            task.decisions.push(detail.clone());
            BaseCarry::Failed(detail)
        }
    }
}

/// How often a task that waits for its base branch to move looks at it.
const BASE_MOVE_POLL: Duration = Duration::from_secs(1);

pub(super) enum BaseRetry {
    Ready,
    Conflict(String),
    Stopped,
}

/// Waits for the base to move and carries onto it. A failed carry stays on
/// an owner question until it can be retried, rather than implementing on
/// the old base. Conflicts are left for a conflict-only implement attempt.
pub(super) async fn carry_after_base_moves(
    app: &Arc<App>,
    task: &mut Task,
    worktree: &Path,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> BaseRetry {
    loop {
        if let Some(base_ref) = task.base_ref.clone() {
            let mut noted = false;
            loop {
                let (wt, r) = (worktree.to_path_buf(), base_ref.clone());
                let now = tokio::task::spawn_blocking(move || git::resolve_commit(&wt, &r))
                    .await
                    .ok()
                    .and_then(|r| r.ok());
                if now.as_deref().is_none_or(|sha| sha != task.base_sha) {
                    break;
                }
                if !noted {
                    noted = true;
                    permit.take();
                    task.decisions.push(format!(
                        "Orchestrator: waiting for {base_ref} to move past {} before the retry",
                        short_sha(&task.base_sha)
                    ));
                    task.updated_at = now_ms();
                    app.save_task_logged(task);
                    app.broadcast_task(task);
                }
                tokio::select! {
                    _ = cancel.cancelled() => return BaseRetry::Stopped,
                    _ = tokio::time::sleep(BASE_MOVE_POLL) => {}
                }
            }
        }
        if cancel.is_cancelled() {
            return BaseRetry::Stopped;
        }
        match carry_onto_newest_base(task, worktree).await {
            BaseCarry::Ready => return BaseRetry::Ready,
            BaseCarry::Conflict(detail) => return BaseRetry::Conflict(detail),
            BaseCarry::Failed(reason) => {
                task.question = Some(Question::new(
                    format!("{reason}. Fix the carry problem, then retry, or stop?"),
                    vec!["retry".to_string(), "stop".to_string()],
                    QuestionKind::AgentQuestion,
                    AskedBy::Verify,
                ));
                task.status = TaskStatus::Waiting;
                task.updated_at = now_ms();
                let task_id = task.id.clone();
                let Some(answer) =
                    wait_for_answer(app, &task_id, task, pending_answer, cancel, permit).await
                else {
                    return BaseRetry::Stopped;
                };
                if is_option(&answer, "stop") {
                    task.status = TaskStatus::Stopped;
                    task.updated_at = now_ms();
                    app.save_task_logged(task);
                    app.broadcast_task(task);
                    return BaseRetry::Stopped;
                }
            }
        }
    }
}

/// The attempt's loop was cancelled after its run: interrupted, never a
/// failure. A daemon shutdown during review or verify leaves a finished (or
/// still running) implement attempt `running` instead, and the next daemon
/// resumes into review and verify.
fn interrupt_attempt(app: &Arc<App>, task: &mut Task, idx: usize) {
    if app.leaves_run_going(task, idx) {
        return;
    }
    task.attempts[idx].status = AttemptStatus::Interrupted;
    task.attempts[idx].ended_at = Some(now_ms());
    app.apply_cancellation(task);
    task.updated_at = now_ms();
    app.save_task_logged(task);
    app.broadcast_task(task);
}

pub(super) async fn mark_stopped_if_not_already(app: &Arc<App>, task_id: &str) {
    if let Ok(Some(mut task)) = app.store.load_task(task_id) {
        if !matches!(task.status, TaskStatus::Done | TaskStatus::Failed) {
            app.apply_cancellation(&mut task);
            task.updated_at = now_ms();
            app.save_task_logged(&task);
            app.broadcast_task(&task);
        }
        app.advance_queue(&task.repo);
    }
}

impl App {
    pub(super) fn start_task_loop(&self, task_id: String) {
        self.spawn_task_loop(task_id, true);
    }

    /// `auto_start_after_plan` only matters for a task still `drafting`: once
    /// planning finishes, `true` lets the loop fall straight through into
    /// the implement stage, `false` leaves it `stopped` (fields filled in)
    /// for the owner to review before calling `task.start` themselves. Every
    /// caller except the `{repo, request}` form of `task.create` wants the
    /// former, since for them planning has either already happened or never
    /// applies.
    pub(super) fn spawn_task_loop(&self, task_id: String, auto_start_after_plan: bool) {
        let mut controls = self.controls.lock().unwrap();
        if controls.contains_key(&task_id) {
            return;
        }
        let cancel = CancelToken::new();
        let pending_answer = Arc::new(StdMutex::new(None));
        let pending_amend = Arc::new(StdMutex::new(None));
        let app = self.arc();
        let cancel_for_loop = cancel.clone();
        let pending_for_loop = pending_answer.clone();
        let amend_for_loop = pending_amend.clone();
        let tid = task_id.clone();
        let handle = tokio::spawn(async move {
            run_task_loop(
                app,
                tid,
                pending_for_loop,
                amend_for_loop,
                cancel_for_loop,
                auto_start_after_plan,
            )
            .await;
        });
        controls.insert(
            task_id,
            TaskControl {
                cancel,
                pending_answer,
                pending_amend,
                handle,
            },
        );
    }

    fn finish_task_loop(&self, task_id: &str) {
        let restart = {
            let mut cancellations = self.cancellations.lock().unwrap();
            self.controls.lock().unwrap().remove(task_id);
            let end = cancellations.remove(task_id);
            if let Some(end) = &end {
                if let Ok(Some(mut task)) = self.store.load_task(task_id) {
                    end.apply(&mut task);
                    for attempt in &mut task.attempts {
                        if attempt.status == AttemptStatus::Running {
                            attempt.status = AttemptStatus::Interrupted;
                            attempt.ended_at = Some(now_ms());
                        }
                    }
                    task.updated_at = now_ms();
                    let _ = self.store.save_task(&task);
                    self.broadcast_task(&task);
                }
            }
            end.is_some_and(|c| c.restart)
        };
        self.record_task_metrics(task_id);
        self.final_base_jobs.lock().unwrap().remove(task_id);
        if let Ok(Some(mut task)) = self.store.load_task(task_id) {
            let before = task.decisions.len();
            record_check_outcomes(self, &mut task, &[]);
            if task.decisions.len() != before {
                self.save_task_logged(&task);
                self.broadcast_task(&task);
            }
        }
        if restart {
            self.spawn_task_loop(task_id.to_string(), true);
        }
        if let Ok(Some(task)) = self.store.load_task(task_id) {
            self.drop_settled_output(&task);
            self.advance_queue(&task.repo);
            self.advance_graph(&task.repo);
        }
        self.record_evolution_signals(task_id);
    }
}

/// What a failing final check tells the next attempt.
fn final_failure_detail(failed: &VerifyOutcome) -> String {
    format!(
        "Final check {} exited {}.\n{}",
        failed.command,
        failed
            .code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "null".to_string()),
        failed.tail
    )
}

pub(super) enum Tail {
    Continue,
    Return,
}

pub(super) struct FinalChecks {
    results: Vec<VerifyOutcome>,
    skipped: Vec<String>,
}

struct BackgroundFinal {
    cancel: CancelToken,
    review_cancel: CancelToken,
    handle: Option<tokio::task::JoinHandle<FinalChecks>>,
}

impl BackgroundFinal {
    fn start(
        app: &Arc<App>,
        task: &Task,
        worktree: &Path,
        run_dir: &Path,
        settings: &Settings,
        parent: &CancelToken,
    ) -> Self {
        let cancel = parent.child();
        let review_cancel = parent.child();
        let (commands, deferred) = split_deferred(app, settings, task);
        let sandbox = app.task_sandbox(&task.repo, worktree, run_dir);
        let app = app.clone();
        let task = task.clone();
        let base = task.base_sha.clone();
        let worktree = worktree.to_path_buf();
        let run_dir = run_dir.to_path_buf();
        let final_cancel = cancel.clone();
        let failed = review_cancel.clone();
        let handle = tokio::spawn(async move {
            let mut finished = run_final_checks(
                &app,
                &task,
                &worktree,
                &run_dir,
                &base,
                &commands,
                sandbox.as_ref(),
                &final_cancel,
                Some(&failed),
            )
            .await;
            finished
                .skipped
                .extend(deferred.iter().map(|c| deferred_line(c)));
            finished
        });
        Self {
            cancel,
            review_cancel,
            handle: Some(handle),
        }
    }

    async fn finish(mut self) -> Option<FinalChecks> {
        self.handle.take().unwrap().await.ok()
    }
}

impl Drop for BackgroundFinal {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_final_checks(
    app: &Arc<App>,
    task: &Task,
    worktree: &Path,
    run_dir: &Path,
    base: &str,
    commands: &[String],
    sandbox: Option<&TaskSandbox>,
    cancel: &CancelToken,
    cancel_review: Option<&CancelToken>,
) -> FinalChecks {
    let (commands, skipped) = scope_task_checks(app, task, worktree, base, commands).await;
    let mut results = Vec::new();
    for command in &commands {
        if cancel.is_cancelled() {
            break;
        }
        let checked = run_verify_commands(
            CheckContext::new(app, Some(&task.id)).with_gate("final checks"),
            worktree,
            &run_dir.join("final"),
            std::slice::from_ref(command),
            sandbox,
            base,
            cancel,
        )
        .await;
        if checked.iter().any(|r| r.code.is_some_and(|code| code != 0)) {
            if let Some(review) = cancel_review {
                review.cancel();
            }
        }
        results.extend(checked);
    }
    FinalChecks { results, skipped }
}

/// Settles retained final results, or runs the checks for an accepted attempt.
/// A failing final is a Verify failure or a pre-existing-check question.
#[allow(clippy::too_many_arguments)]
pub(super) async fn finish_after_review(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    attempt_n: u32,
    worktree: &Path,
    run_dir: &Path,
    settings: &Settings,
    finished: Option<FinalChecks>,
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> Tail {
    // Settle the base preflight before interpreting final results or landing.
    if !settle_base_preflight(app, task, cancel).await {
        interrupt_attempt(app, task, idx);
        *permit = None;
        app.finish_task_loop(task_id);
        return Tail::Return;
    }
    // The commands the parent runs anyway are left out of this run; the
    // stored `finalVerify` is not changed.
    let (to_run, deferred) = split_deferred(app, settings, task);
    record_check_decisions(task, deferred.iter().map(|c| deferred_line(c)).collect());
    if !to_run.is_empty() {
        // Not through the verify cache: it keys on the diff alone and
        // would hand back the fast checks' results.
        let FinalChecks {
            results: final_results,
            skipped,
        } = match finished {
            Some(finished) => finished,
            None => {
                let sandbox = app.task_sandbox(&task.repo, worktree, run_dir);
                run_final_checks(
                    app,
                    task,
                    worktree,
                    run_dir,
                    &task.base_sha,
                    &to_run,
                    sandbox.as_ref(),
                    cancel,
                    None,
                )
                .await
            }
        };
        record_check_decisions(task, skipped);
        record_check_outcomes(app, task, &final_results);
        if cancel.is_cancelled() {
            interrupt_attempt(app, task, idx);
            *permit = None;
            app.finish_task_loop(task_id);
            return Tail::Return;
        }
        task.attempts[idx]
            .verify
            .extend(final_results.iter().cloned());
        let failed_finals: Vec<VerifyOutcome> = final_results
            .iter()
            .filter(|v| v.code != Some(0))
            .cloned()
            .collect();
        // Every failing final goes through the base lookup (cache, else the
        // in-flight job, else a job started now), never a second run.
        let base_dir = run_dir.join("final-base");
        let mut on_base: Vec<(VerifyOutcome, VerifyOutcome)> = Vec::new();
        let mut ordinary: Vec<VerifyOutcome> = Vec::new();
        for failed in failed_finals {
            if timed_out_under_load(&failed) {
                ordinary.push(failed);
                continue;
            }
            match failing_on_base(app, task, &failed, &base_dir, cancel).await {
                Some(base_run) => on_base.push((failed, base_run)),
                None => ordinary.push(failed),
            }
        }
        if cancel.is_cancelled() {
            interrupt_attempt(app, task, idx);
            *permit = None;
            app.finish_task_loop(task_id);
            return Tail::Return;
        }
        note_flaky_base_checks(app, task, &to_run);
        // A base failure the plan explains (the files it names are created
        // by the plan) or the owner kept is an ordinary failure.
        let names: Vec<String> = on_base.iter().map(|(f, _)| f.command.clone()).collect();
        let kept = keep_planned_checks(app, task, &names);
        let mut pre_existing: Vec<VerifyOutcome> = Vec::new();
        for (failed, base_run) in on_base {
            if kept.contains(&failed.command) || is_kept(task, &failed.command) {
                ordinary.push(failed);
            } else {
                pre_existing.push(base_run);
            }
        }
        if !kept.is_empty() {
            task.updated_at = now_ms();
            app.save_task_logged(task);
            app.broadcast_task(task);
        }
        let ordinary_detail = ordinary
            .iter()
            .map(final_failure_detail)
            .collect::<Vec<_>>()
            .join("\n\n");
        if pre_existing.is_empty() {
            if !ordinary.is_empty() {
                return match fail_and_continue(
                    app,
                    task_id,
                    task,
                    idx,
                    FailureKind::Verify,
                    ordinary_detail,
                    attempt_budget,
                    pending_answer,
                    cancel,
                    permit,
                )
                .await
                {
                    LoopSignal::Continue => {
                        *permit = None;
                        Tail::Continue
                    }
                    LoopSignal::Stop => {
                        *permit = None;
                        app.finish_task_loop(task_id);
                        Tail::Return
                    }
                };
            }
        } else {
            // The failure is older than this task: no retry can fix it. The
            // owner decides, once for all of them, and no attempt is spent.
            let mut detail: Vec<String> = pre_existing
                .iter()
                .map(|b| {
                    let failed = final_results.iter().find(|v| v.command == b.command);
                    failed.map(final_failure_detail).unwrap_or_default()
                })
                .collect();
            if !ordinary_detail.is_empty() {
                detail.push(ordinary_detail.clone());
            }
            record_failure(task, idx, FailureKind::Verify, detail.join("\n\n"));
            let question = pre_existing_question(&pre_existing, &task.base_sha);
            let assumed = task.assumptions.len();
            let already_retried = base_retry_requested(task, &question.commands, &task.base_sha);
            let answer = if already_retried {
                PRE_EXISTING_RETRY.to_string()
            } else {
                task.question = Some(question.clone());
                task.status = TaskStatus::Waiting;
                task.updated_at = now_ms();
                let Some(answer) =
                    wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await
                else {
                    *permit = None;
                    app.finish_task_loop(task_id);
                    return Tail::Return;
                };
                answer
            };
            let policy_answered = task.assumptions.len() > assumed;
            if !already_retried {
                let base_sha = task.base_sha.clone();
                record_owner_base_check(
                    task,
                    &question.commands,
                    &base_sha,
                    &answer,
                    policy_answered,
                );
            }
            let settings = app.settings.read().unwrap().clone();
            let origin = origin_of_last_answer(task);
            // Every removal goes through `remove_check_command`; a refused
            // command stays and the answer counts as a retry for it.
            if apply_base_check_answer(&settings, task, &question, &answer, origin) {
                app.verify_cache.lock().unwrap().remove(task_id);
            }
            let kept_all =
                !question.commands.is_empty() && question.commands.iter().all(|c| is_kept(task, c));
            if kept_all {
                let mut details: Vec<String> = final_results
                    .iter()
                    .filter(|result| question.commands.contains(&result.command))
                    .map(final_failure_detail)
                    .collect();
                if !ordinary_detail.is_empty() {
                    details.push(ordinary_detail.clone());
                }
                return match fail_and_continue(
                    app,
                    task_id,
                    task,
                    idx,
                    FailureKind::Verify,
                    details.join("\n\n"),
                    attempt_budget,
                    pending_answer,
                    cancel,
                    permit,
                )
                .await
                {
                    LoopSignal::Continue => {
                        *permit = None;
                        Tail::Continue
                    }
                    LoopSignal::Stop => {
                        *permit = None;
                        app.finish_task_loop(task_id);
                        Tail::Return
                    }
                };
            }
            let stays = question
                .commands
                .iter()
                .any(|c| task.final_verify.contains(c));
            if stays {
                // Retry (any other answer): one more attempt on the newest
                // base, even when the attempt budget is spent.
                match carry_after_base_moves(app, task, worktree, pending_answer, cancel, permit)
                    .await
                {
                    BaseRetry::Ready => {}
                    BaseRetry::Conflict(detail) => {
                        record_failure(task, idx, FailureKind::Conflict, detail);
                    }
                    BaseRetry::Stopped => {
                        interrupt_attempt(app, task, idx);
                        *permit = None;
                        app.finish_task_loop(task_id);
                        return Tail::Return;
                    }
                }
                *attempt_budget = (*attempt_budget).max(counted_attempts(task) + 1);
                task.updated_at = now_ms();
                app.save_task_logged(task);
                app.broadcast_task(task);
                *permit = None;
                return Tail::Continue;
            }
            // Every listed check is gone: the attempt is not failed by them.
            let a = &mut task.attempts[idx];
            a.status = AttemptStatus::Running;
            a.ended_at = None;
            a.failure = None;
            task.status = TaskStatus::Running;
            task.updated_at = now_ms();
            app.save_task_logged(task);
            app.broadcast_task(task);
            if !ordinary.is_empty() {
                return match fail_and_continue(
                    app,
                    task_id,
                    task,
                    idx,
                    FailureKind::Verify,
                    ordinary_detail,
                    attempt_budget,
                    pending_answer,
                    cancel,
                    permit,
                )
                .await
                {
                    LoopSignal::Continue => {
                        *permit = None;
                        Tail::Continue
                    }
                    LoopSignal::Stop => {
                        *permit = None;
                        app.finish_task_loop(task_id);
                        Tail::Return
                    }
                };
            }
        }
    }
    finish_attempt(
        app,
        task_id,
        task,
        idx,
        attempt_n,
        worktree,
        run_dir,
        attempt_budget,
        pending_answer,
        cancel,
        permit,
    )
    .await
}

/// What follows a passed attempt: the task lands on its parent or its base
/// branch when it has one to land on, else it commits on its own branch.
/// Ends the loop itself (`Tail::Return`) or hands back to it.
#[allow(clippy::too_many_arguments)]
pub(super) async fn finish_attempt(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    attempt_n: u32,
    worktree: &Path,
    run_dir: &Path,
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> Tail {
    let landing = if let Some(parent_id) = task.parent.clone() {
        Some(
            land_on_parent(
                app, task, idx, attempt_n, worktree, run_dir, &parent_id, cancel,
            )
            .await,
        )
    } else if task.variant().land {
        Some(land_on_base(app, task, Some(idx), attempt_n, worktree, run_dir, cancel).await)
    } else {
        None
    };
    if let Some(landing) = landing {
        match landing {
            Landing::Waiting => {
                task.attempts[idx].status = AttemptStatus::Passed;
                task.attempts[idx].ended_at = Some(now_ms());
                task.status = TaskStatus::Landing;
                task.updated_at = now_ms();
                app.save_task_logged(task);
                app.broadcast_task(task);
                *permit = None;
                app.finish_task_loop(task_id);
                return Tail::Return;
            }
            Landing::Landed => {
                git::delete_wip_ref(worktree, task_id);
                task.attempts[idx].status = AttemptStatus::Passed;
                task.attempts[idx].ended_at = Some(now_ms());
                task.status = TaskStatus::Done;
                task.updated_at = now_ms();
                app.write_report(task).await;
                app.release_worktree(task, "landed").await;
                task.updated_at = now_ms();
                app.save_task_logged(task);
                app.broadcast_task(task);
                *permit = None;
                app.finish_task_loop(task_id);
                return Tail::Return;
            }
            Landing::Failed { kind, detail } => {
                match fail_and_continue(
                    app,
                    task_id,
                    task,
                    idx,
                    kind,
                    detail,
                    attempt_budget,
                    pending_answer,
                    cancel,
                    permit,
                )
                .await
                {
                    LoopSignal::Continue => {
                        *permit = None;
                        return Tail::Continue;
                    }
                    LoopSignal::Stop => {
                        *permit = None;
                        app.finish_task_loop(task_id);
                        return Tail::Return;
                    }
                }
            }
            Landing::Cancelled => {
                task.attempts[idx].status = AttemptStatus::Interrupted;
                task.attempts[idx].ended_at = Some(now_ms());
                app.apply_cancellation(task);
                task.updated_at = now_ms();
                app.save_task_logged(task);
                app.broadcast_task(task);
                *permit = None;
                app.finish_task_loop(task_id);
                return Tail::Return;
            }
            Landing::NoParent => {}
        }
    }

    let wt4 = worktree.to_path_buf();
    let title = task.title.clone();
    let tid = task_id.to_string();
    let commit_res =
        tokio::task::spawn_blocking(move || git::commit(&wt4, &title, &tid, attempt_n)).await;
    match commit_res {
        Ok(Ok(())) => {
            git::delete_wip_ref(worktree, task_id);
            refresh_diff_stat(task).await;
            task.attempts[idx].status = AttemptStatus::Passed;
            task.attempts[idx].ended_at = Some(now_ms());
            task.status = TaskStatus::Done;
            if let Some(cmd) = task.eval_check_cmd.clone() {
                let sha = git::head_sha(worktree).unwrap_or_default();
                let check = run_eval_check(
                    CheckContext::new(app, Some(task_id)),
                    Path::new(&task.repo),
                    run_dir,
                    &sha,
                    &cmd,
                    app.task_sandbox(&task.repo, &run_dir.join("eval-check"), run_dir)
                        .as_ref(),
                    cancel,
                )
                .await;
                record_check_outcomes(app, task, std::slice::from_ref(&check));
                task.eval_check = Some(EvalCheck {
                    code: check.code,
                    tail: check.tail,
                });
            }
        }
        Ok(Err(e)) => {
            record_failure(task, idx, FailureKind::Error, e.to_string());
            task.status = TaskStatus::Failed;
        }
        Err(e) => {
            record_failure(
                task,
                idx,
                FailureKind::Error,
                format!("commit task panicked: {e}"),
            );
            task.status = TaskStatus::Failed;
        }
    }
    if task.status == TaskStatus::Done {
        task.updated_at = now_ms();
        app.write_report(task).await;
        app.release_worktree(task, "committed on its branch").await;
    }
    task.updated_at = now_ms();
    app.save_task_logged(task);
    app.broadcast_task(task);
    wake_orchestrator_for_done(app, task);
    *permit = None;
    app.finish_task_loop(task_id);
    Tail::Return
}

#[cfg(test)]
mod final_check_tests {
    use super::*;

    fn fixture(
        commands: Vec<String>,
    ) -> (
        Arc<App>,
        tempfile::TempDir,
        tempfile::TempDir,
        Task,
        Settings,
    ) {
        let (app, home) = test_app();
        let repo = tempfile::tempdir().unwrap();
        init_git_repo(repo.path());
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.path().to_string_lossy().into_owned();
        task.worktree = task.repo.clone();
        task.base_sha = git_in(repo.path(), &["rev-parse", "HEAD"]);
        task.final_verify = commands;
        let mut settings = app.settings.read().unwrap().clone();
        settings.sandbox = SandboxMode::Host;
        (app, home, repo, task, settings)
    }

    #[tokio::test]
    async fn background_final_failure_cancels_review_without_stopping_the_task() {
        let (app, _home, repo, task, settings) = fixture(vec!["echo failed; exit 7".into()]);
        let parent = CancelToken::new();
        let finals =
            BackgroundFinal::start(&app, &task, repo.path(), repo.path(), &settings, &parent);
        tokio::time::timeout(Duration::from_secs(5), finals.review_cancel.cancelled())
            .await
            .unwrap();
        assert!(!parent.is_cancelled());
        assert!(!finals.cancel.is_cancelled());
        let result = finals.finish().await.unwrap();
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].code, Some(7));
        assert!(result.results[0].tail.contains("failed"));
    }

    #[tokio::test]
    async fn background_final_cancellation_stops_the_command_before_its_done_marker() {
        let (app, _home, repo, task, settings) =
            fixture(vec!["touch STARTED; sleep 30; touch DONE".into()]);
        let parent = CancelToken::new();
        let finals =
            BackgroundFinal::start(&app, &task, repo.path(), repo.path(), &settings, &parent);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !repo.path().join("STARTED").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        finals.cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(10), finals.finish())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.results[0].code, None);
        assert!(!repo.path().join("DONE").exists());
        assert!(!parent.is_cancelled());
    }
}
