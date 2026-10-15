use crate::clock::now_ms;
use crate::engine::answer_policy::record_policy_answer;
use crate::engine::brief_check::check_brief;
use crate::engine::brief_check::refresh_after_dependencies;
use crate::engine::brief_check::BriefAction;
use crate::engine::cost::settle_unfinished_cost;
use crate::engine::cost::wait_while_over_budget;
use crate::engine::decision::completed_implement_attempts;
use crate::engine::decision::fail_and_continue;
use crate::engine::decision::implement_attempt_count;
use crate::engine::decision::record_failure;
use crate::engine::decision::LoopSignal;
use crate::engine::fresh_base;
use crate::engine::graph::parent::run_parent;
use crate::engine::graph::parent::ParentEnd;
use crate::engine::graph::relay::conflict_detail;
use crate::engine::graph::relay::start_relay_link;
use crate::engine::ladder;
use crate::engine::leases::watch_lease;
use crate::engine::lifecycle::base_wait::carry_after_base_moves;
use crate::engine::lifecycle::base_wait::BaseRetry;
use crate::engine::lifecycle::carry::carry_onto_newest_base;
use crate::engine::lifecycle::carry::rebase_conflict_note;
use crate::engine::lifecycle::carry::BaseCarry;
use crate::engine::lifecycle::changes::capture_run_interruption;
use crate::engine::lifecycle::changes::record_diff_stat;
use crate::engine::lifecycle::changes::unnamed_changes_since_full_review;
use crate::engine::lifecycle::completion::admission::carry_admission;
use crate::engine::lifecycle::completion::admission::delivery_admission;
use crate::engine::lifecycle::completion::delivery::finish_attempt;
use crate::engine::lifecycle::completion::finish::finish_after_review;
use crate::engine::lifecycle::completion::Tail;
use crate::engine::lifecycle::final_checks::BackgroundFinal;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::plan_gate;
use crate::engine::planning::records::needs_planning;
use crate::engine::planning::run::run_plan_stage;
use crate::engine::planning::run::PlanOutcome;
use crate::engine::protected::adopt_carried_tests;
use crate::engine::protected::guard_protected;
use crate::engine::protected::needs_prepare;
use crate::engine::protected::prepare_tests;
use crate::engine::protected::repair_protection;
use crate::engine::questions::parking::wait_for_answer;
use crate::engine::questions::wake::wake_orchestrator_for_done;
use crate::engine::rate_limit::counted_attempts;
use crate::engine::rate_limit::pause_for_rate_limit;
use crate::engine::rate_limit::rate_limit_wait_secs;
use crate::engine::recovery::resumable_attempt;
use crate::engine::review::evidence_scope;
use crate::engine::review::run_review;
use crate::engine::review::save_evidence;
use crate::engine::review::select_review_route_with;
use crate::engine::review::EvidenceScope;
use crate::engine::review::ReviewFailure;
use crate::engine::review_findings::handle_repeated_findings;
use crate::engine::review_findings::review_stopped_note;
use crate::engine::review_findings::MAX_REVIEW_RUNS;
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::take_slot;
use crate::engine::slots::Slot;
use crate::engine::state::App;
use crate::engine::test_claim;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::engine::verification::base::answers::base_check_answered;
use crate::engine::verification::base::answers::base_retry_requested;
use crate::engine::verification::base::checks::broken_by_the_new_base;
use crate::engine::verification::base::checks::start_final_base_jobs;
use crate::engine::verification::base::wait::wait_on_broken_base;
use crate::engine::verification::base::wait::BaseBreak;
use crate::engine::verification::baseline::gated_checks;
use crate::engine::verification::baseline::gated_held_out;
use crate::engine::verification::cache::run_verify_cached;
use crate::engine::verification::context::record_check_decisions;
use crate::engine::verification::context::record_check_outcomes;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::healing::answers::{heal_answers, AnswerHealing};
use crate::engine::verification::healing::evidence::capture_screenshots;
use crate::engine::verification::healing::evidence::findings_old_on_base;
use crate::engine::verification::healing::preflight::settle_base_preflight;
use crate::engine::verification::healing::preflight::start_base_checks;
use crate::engine::verification::healing::repeated::heal_repeated_unmet;
use crate::engine::verification::held_out::run_held_out;
use crate::engine::verification::runner::run_verify_commands;
use crate::engine::verification::scoping::scope_task_checks;
use crate::git::paths::short_sha;
use crate::harness::native::error::RunError;
use crate::policy::paths::matches_any_protected;
use crate::policy::routing::Tier;
use crate::task::attempts::{
    self as observations, BestOfSelection, EvidenceObservation, ReviewState, ReviewView,
    RunCompletion, VerifyObservation,
};
use crate::task::checks::results::refresh_criteria_results;
use crate::task::findings::{apply_declines, move_minor_findings, register_findings};
use crate::task::graph::is_parent;
use crate::task::AskedBy;
use crate::task::FailureKind;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::ReviewResult;
use crate::task::ReviewScope;
use crate::task::Stage;
use crate::task::TaskStatus;
use crate::task::Usage;
use crate::task::Verdict;
use crate::task::Wait;
use crate::verify::healing::scope::scope_review;
use crate::verify::load::timed_out_under_load;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// The per-task attempt loop (spec "Engine", steps 1-10). Runs as its own
/// tokio task from `task.start` until the task reaches `done`/`failed`, is
/// stopped, or the daemon shuts down; `waiting` parks it on `pending_answer`
/// rather than exiting, so the in-memory attempt-budget extension (step 9)
/// survives across a wait/answer cycle.
pub(in crate::engine) async fn run_task_loop(
    app: Arc<App>,
    task_id: String,
    signals: TaskSignals,
    auto_start_after_plan: bool,
) {
    let pending_answer = signals.pending_answer.clone();
    let pending_amend = signals.pending_amend.clone();
    let cancel = signals.cancel.clone();
    let mut attempt_budget = app.settings.read().unwrap().max_attempts;
    let mut budget_loaded = false;
    // Set once a parent's finish failed after every subtask landed: the
    // failure its own implement attempt has to fix. While it is set (and once
    // the parent has an attempt) the parent never goes back to `run_parent`.
    let mut parent_failure: Option<String> = None;
    let mut parent_conflict_only = false;
    let mut base_refreshed = false;

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
            task.record_metrics(None);
            task.touch(now_ms());
            app.save_task_logged(&task);
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
            let previous = task.updated_at;
            crate::task::transitions::end_limit_pause(
                &mut task,
                crate::task::transitions::PauseEnd::Elapsed,
                previous,
            );
            task.touch(now_ms());
            app.save_task_logged(&task);
            app.broadcast_task(&task);
        }

        let Some(recreated) =
            crate::engine::lifecycle::workspace::restore(&app, &mut task, &cancel).await
        else {
            return;
        };
        if !crate::engine::lifecycle::workspace::setup(&app, &mut task, recreated, &cancel).await {
            return;
        }

        apply_pending_amendment(&app, &mut task, &pending_amend, &cancel).await;
        if completed_implement_attempts(&task) > 0
            && crate::engine::verification::baseline::needs_baseline(&task)
        {
            crate::engine::verification::base::amendments::baseline_current_plan(
                &app, &mut task, &cancel,
            )
            .await;
        }

        match crate::engine::lifecycle::admission::graph(
            &app,
            &mut task,
            &signals,
            parent_failure.as_deref(),
        )
        .await
        {
            crate::engine::lifecycle::admission::Admission::Ready { carry_conflict } => {
                if let Some(failure) = carry_conflict {
                    parent_conflict_only = true;
                    parent_failure = Some(failure);
                }
            }
            crate::engine::lifecycle::admission::Admission::Waiting
            | crate::engine::lifecycle::admission::Admission::Parent(ParentEnd::Finished) => {
                app.finish_task_loop(&task_id);
                return;
            }
            crate::engine::lifecycle::admission::Admission::Parent(ParentEnd::Attempt {
                failure,
                budget,
                conflict_only,
            }) => {
                parent_conflict_only = conflict_only;
                parent_failure = Some(failure);
                attempt_budget = budget.unwrap_or(attempt_budget);
                continue 'attempts;
            }
        }

        // The gates are passed: a wait on dependencies or a subtask slot is over.
        if matches!(
            task.queue.wait,
            Some(Wait::Dependency { .. } | Wait::SubtaskSlot { .. })
        ) {
            task.set_wait(None);
            task.touch(now_ms());
            app.save_task_logged(&task);
            app.broadcast_task(&task);
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
            task.touch(now_ms());
            app.save_task_logged(&task);
            app.broadcast_task(&task);
        }

        // Concurrency limit: stays `queued` while waiting for a slot, and
        // `task.stop` (via `cancel`) works here too.
        let mut permit = match take_slot(&app, &mut task, &cancel).await {
            Slot::Got(p) => Some(p),
            Slot::Cancelled => {
                mark_stopped_if_not_already(&app, &task_id).await;
                app.finish_task_loop(&task_id);
                return;
            }
            Slot::Closed => {
                app.finish_task_loop(&task_id);
                return;
            }
        };

        // A named base is fetched again as the task starts, so it plans or
        // implements on the branch as its remote has it now.
        if !base_refreshed {
            base_refreshed = true;
            fresh_base::refresh_at_start(&app, &mut task).await;
        }

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

        // A retired "no verdict" question: only the final checks and the
        // commit are left, and the work is not landed.
        if let Some(idx) = ladder::pending_unreviewed(&task) {
            let attempt_n = task.attempts[idx].n;
            let run_dir = app.store.run_dir(&task_id, attempt_n);
            let _ = std::fs::create_dir_all(&run_dir);
            let settings = app.settings.read().unwrap().clone();
            let worktree = PathBuf::from(&task.worktree);
            crate::task::transitions::reopen(&mut task, idx, now_ms());
            task.clear_unreviewed_attempt();
            task.resume_phase(TaskStatus::Running, task.updated_at);
            task.touch(now_ms());
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
                fresh_base::plan_on_newest_base(&mut task).await;
            }
            if task.status == TaskStatus::Queued {
                task.resume_phase(TaskStatus::Drafting, task.updated_at);
            }
            task.touch(now_ms());
            app.save_task_logged(&task);
            app.broadcast_task(&task);
            let planned =
                run_plan_stage(&app, &task_id, &signals, &mut permit, auto_start_after_plan).await;
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
                match run_parent(&app, &task_id, &signals).await {
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
        // planned brief was already checked in the plan stage. The base
        // checks start beside the brief check, not after it.
        if completed_implement_attempts(&task) == 0
            && !task.brief_check.done
            && parent_failure.is_none()
        {
            start_base_checks(&app, &mut task, &cancel);
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
                // Only enabling a connected tool is the owner's to allow;
                // without one the task goes on without the capability.
                if missing.tool_id.is_none() {
                    task.record_decision(format!(
                        "Orchestrator: no tool exists for {}; the task goes on without it",
                        missing.capability
                    ));
                } else {
                    task.ask(
                        crate::engine::permissions::capabilities::missing_tool_question(
                            &task, &missing,
                        ),
                        task.updated_at,
                    );
                    task.resume_phase(TaskStatus::Waiting, task.updated_at);
                    task.touch(now_ms());
                    let Some(answer) =
                        wait_for_answer(&app, &mut task, &pending_answer, &cancel, &mut permit)
                            .await
                    else {
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    };
                    crate::engine::permissions::capabilities::apply_missing_answer(
                        &app, &mut task, &missing, &answer,
                    );
                }
                task.mark_missing_tool_asked();
                task.touch(now_ms());
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
            start_base_checks(&app, &mut task, &cancel);
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
                let before_relay = task.clone();
                start_relay_link(&app, &mut task, &prev_id).await;
                if let Some(tail) =
                    carry_admission(&app, &mut task, &before_relay, &cancel, &mut permit)
                {
                    match tail {
                        Tail::Return => return,
                        Tail::Continue => continue 'attempts,
                    }
                }
            }
        }
        let wt = PathBuf::from(&task.worktree);
        repair_protection(&mut task, &wt).await;
        if task.parent.is_some() {
            let _ = app.sync_graph_base(&task.id, false).await;
            app.carry_part_onto_parent(&mut task, &cancel).await;
        } else if completed_implement_attempts(&task) == 0
            && parent_failure.is_none()
            && !task.depends_on.is_empty()
            && task.queue.relay_of.is_none()
        {
            let wt = PathBuf::from(&task.worktree);
            match carry_onto_newest_base(&app, &mut task, &wt, &cancel).await {
                BaseCarry::Stale => {
                    drop(permit);
                    continue 'attempts;
                }
                BaseCarry::Stopped => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                _ => {}
            }
        }
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

        if completed_implement_attempts(&task) == 0 {
            match task.planned_tier {
                Some(tier) => {
                    task.select_tier(tier, None);
                }
                None => {
                    task.select_tier(Tier::Standard, Some("no planner tier".to_string()));
                }
            }
        }

        // A run that outlived the daemon is followed, not started again: its
        // outcome goes through review and verify like any other pass.
        let resumed = resumable_attempt(&app, &task);
        // The lease follows the worktree diff while the attempt runs.
        let _lease_watch = watch_lease(&app, &task);
        let mode = match resumed {
            Some(idx) => crate::engine::lifecycle::execution::ExecutionMode::Adopt(idx),
            None => crate::engine::lifecycle::execution::ExecutionMode::Start(
                parent_failure.as_deref().map(|detail| {
                    crate::engine::lifecycle::execution::ParentFailure {
                        detail,
                        conflict_only: parent_conflict_only,
                    }
                }),
            ),
        };
        let crate::engine::lifecycle::execution::ExecutedAttempt {
            run: mut run_receipt,
            key_path,
            mut registered,
            run_result,
            best_of,
        } = match crate::engine::lifecycle::execution::execute(
            &app,
            &mut task,
            &signals,
            &mut permit,
            mode,
        )
        .await
        {
            crate::engine::lifecycle::execution::ExecutionStep::Executed(run) => *run,
            crate::engine::lifecycle::execution::ExecutionStep::Retry => {
                drop(permit);
                continue 'attempts;
            }
            crate::engine::lifecycle::execution::ExecutionStep::Stop => return,
        };
        let settings = run_receipt.settings.clone();
        let attempt_n = run_receipt.attempt_n;
        let idx = run_receipt.idx;
        let route = run_receipt.route.clone();
        let worktree = run_receipt.worktree.clone();
        let base_sha = run_receipt.base_sha.clone();
        let run_dir = run_receipt.run_dir.clone();

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
            let applied = crate::engine::permissions::staging::finish_staging(&ctx);
            (
                ctx.blocks.load(Ordering::SeqCst),
                ctx.handled.lock().unwrap().clone(),
                crate::engine::permissions::staging::staging_lines(&applied),
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
        observations::record_gate_blocks(&mut task, idx, gate_blocks);
        task.record_decisions(staged_lines);
        // Both candidates' spend counts, the loser's and the pick run's on
        // top of the winner's (added below with the attempt's own).
        record_check_decisions(&mut task, best_of.decisions);
        let best_of_discarded = best_of.discarded;
        let b_won = best_of.winner.is_some();
        let route = best_of.winner.clone().unwrap_or(route);
        run_receipt.route = route.clone();
        observations::record_best_of(
            &mut task,
            idx,
            BestOfSelection {
                winner: best_of.winner,
                candidates: best_of.candidates,
                extra_cost: best_of.cost,
            },
        );

        let outcome = match run_result {
            Ok(o) => o,
            Err(RunError::Cancelled) => {
                crate::task::attempts::end(
                    &mut task,
                    idx,
                    crate::task::attempts::AttemptEnd::Interrupted,
                    now_ms(),
                );
                settle_unfinished_cost(&mut task, idx, &app.store, &settings.prices);
                app.apply_cancellation(&mut task);
                if task.status == TaskStatus::Stopped {
                    app.close_stopped_task(&mut task);
                }
                task.touch(now_ms());
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
                crate::task::attempts::discard(&mut task, idx, now_ms());
                task.record_decision(format!("Orchestrator: {msg}; no attempt was used"));
                task.touch(now_ms());
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
        observations::record_run_completion(
            &mut task,
            idx,
            RunCompletion {
                session: outcome.session_id.clone().filter(|_| !b_won),
                fingerprint: outcome.fingerprint.clone(),
                usage: Usage {
                    input: outcome.usage_input,
                    output: outcome.usage_output,
                    cached: outcome.usage_cached,
                },
                first_turn_tokens: outcome.first_turn_tokens,
                cost: outcome.cost_usd,
                estimated: outcome.cost_estimated,
            },
        );

        if let Some((kind, detail)) = capture_run_interruption(
            &mut task,
            idx,
            &outcome,
            &worktree,
            &base_sha,
            settings.implement_budget_mins,
        )
        .await
        {
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

        // A call the run was refused and orchd never decided: one owner
        // question, not another attempt that would be refused the same way.
        let protected_now: Vec<String> = task
            .planner_tests
            .protected
            .iter()
            .flat_map(|m| m.keys().cloned())
            .collect();
        for line in crate::engine::permissions::denials::scratch_lines(&worktree, &outcome) {
            if !task.decisions.contains(&line) {
                task.record_decision(line);
            }
        }
        let refused = crate::engine::permissions::denials::unhandled_denials(
            &handled,
            &worktree,
            &outcome,
            &protected_now,
        );
        if !refused.is_empty() {
            let (wt, own) = (worktree.clone(), task.own_diff());
            let changed =
                tokio::task::spawn_blocking(move || own.changed_files(&wt).unwrap_or_default())
                    .await
                    .unwrap_or_default();
            observations::record_changed_files(&mut task, idx, changed);
            record_diff_stat(&mut task, idx, &worktree).await;
            if !crate::engine::permissions::denials::prepare_denials(&app, &mut task, idx, &refused)
            {
                drop(permit);
                app.finish_task_loop(&task_id);
                return;
            }
            match wait_for_answer(&app, &mut task, &pending_answer, &cancel, &mut permit).await {
                Some(answer) => {
                    crate::engine::permissions::denials::finish_denials(
                        &app, &mut task, &refused, &answer,
                    );
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

        let implementer_report = crate::engine::lifecycle::report::record(&mut task, idx, &outcome);

        let (wt3, own3) = (worktree.clone(), task.own_diff());
        let mut changed =
            tokio::task::spawn_blocking(move || own3.changed_files(&wt3).unwrap_or_default())
                .await
                .unwrap_or_default();
        observations::record_changed_files(&mut task, idx, changed.clone());
        record_diff_stat(&mut task, idx, &worktree).await;

        // The planner's tests and the files the checks name: a change is put
        // back, silently. No failure, no question; the next brief says so and
        // the checks run on the originals.
        let restored = guard_protected(&task, &worktree, &changed)
            .await
            .unwrap_or_default();
        if !restored.is_empty() {
            task.record_decision(format!(
                "Orchestrator: discarded the attempt's edits to protected files: {}; checks run on the originals",
                restored.join(", ")
            ));
            changed.retain(|f| !restored.contains(f));
            observations::record_changed_files(&mut task, idx, changed.clone());
        }
        let mut restored = restored;
        restored.extend(best_of_discarded);
        restored.sort();
        restored.dedup();
        task.record_discarded_tests(restored);
        if let Some(r) = implementer_report
            .report
            .as_ref()
            .filter(|r| !r.test_claims.is_empty())
        {
            let claims = r.test_claims.clone();
            test_claim::rule_on_claims(
                &app,
                &mut task,
                idx,
                &claims,
                &worktree,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await;
        }

        match crate::engine::lifecycle::reported_question::resolve(
            &app,
            &mut task,
            idx,
            &implementer_report,
            &signals,
            &mut permit,
            attempt_budget,
        )
        .await
        {
            crate::engine::lifecycle::reported_question::Action::Proceed => {}
            crate::engine::lifecycle::reported_question::Action::Retry(extra) => {
                attempt_budget += extra;
                drop(permit);
                continue;
            }
            crate::engine::lifecycle::reported_question::Action::Stop => {
                drop(permit);
                app.finish_task_loop(&task_id);
                return;
            }
        }
        let crate::engine::lifecycle::report::ImplementerReport {
            note: implementer_note,
            decisions: agent_decisions,
            ..
        } = implementer_report;

        // A usage or rate limit is not a failed attempt: the task pauses and
        // resumes by itself.
        if changed.is_empty() {
            let failure = outcome.provider_failure(now_ms() / 1000);
            if let Some((error, wait)) =
                outcome.error.as_deref().zip(failure).and_then(|(e, f)| {
                    rate_limit_wait_secs(f, task.queue.rate_pauses).map(|w| (e, w))
                })
            {
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
            task.reset_rate_pauses();
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
        if let Some(base_ref) = task.base_ref.clone() {
            let before_carry = task.clone();
            let (wt, mut snapshot, r) = (worktree.clone(), task.clone(), base_ref.clone());
            let carried = tokio::task::spawn_blocking(move || {
                let carried = fresh_base::carry_work(&mut snapshot, &wt, &r)?;
                Ok((carried, snapshot.planner_tests.clone()))
            })
            .await
            .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())));
            let carried = carried.map(|(carried, tests)| {
                adopt_carried_tests(&mut task, tests, &carried);
                carried
            });
            if let Ok(
                crate::delivery::git::carry::Rebase::Moved { new_sha, .. }
                | crate::delivery::git::carry::Rebase::Conflicts { new_sha, .. },
            ) = &carried
            {
                crate::task::transitions::move_base(&mut task, new_sha.clone());
                if let Some(tail) =
                    carry_admission(&app, &mut task, &before_carry, &cancel, &mut permit)
                {
                    match tail {
                        Tail::Return => return,
                        Tail::Continue => continue 'attempts,
                    }
                }
            }
            match carried {
                Ok(crate::delivery::git::carry::Rebase::Unchanged) => {}
                Ok(crate::delivery::git::carry::Rebase::Moved { new_sha, rewritten }) => {
                    task.record_decision(if rewritten {
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
                    base_sha = new_sha;
                    let (wt, own) = (worktree.clone(), task.own_diff());
                    changed = tokio::task::spawn_blocking(move || {
                        own.changed_files(&wt).unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default();
                    observations::record_changed_files(&mut task, idx, changed.clone());
                    record_diff_stat(&mut task, idx, &worktree).await;
                    if changed.is_empty() {
                        if let Some(tail) =
                            delivery_admission(&app, &mut task, idx, &cancel, &mut permit)
                        {
                            match tail {
                                Tail::Return => return,
                                Tail::Continue => continue 'attempts,
                            }
                        }
                        // The base already holds this work: nothing left to commit.
                        task.record_decision(format!(
                            "Rebase: {base_ref} already contains this work; nothing to commit"
                        ));
                        crate::task::transitions::complete(
                            &mut task,
                            Some(idx),
                            crate::task::transitions::Completion::Done,
                            now_ms(),
                        );
                        task.touch(now_ms());
                        app.write_report(&mut task).await;
                        app.release_worktree(&mut task, "task done").await;
                        task.touch(now_ms());
                        app.save_task_logged(&task);
                        app.broadcast_task(&task);
                        wake_orchestrator_for_done(&app, &mut task);
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                }
                Ok(crate::delivery::git::carry::Rebase::Skipped { reason }) => {
                    let note = format!("Rebase: not carried onto {base_ref} ({reason})");
                    if !task.decisions.contains(&note) {
                        task.record_decision(note);
                    }
                }
                Ok(crate::delivery::git::carry::Rebase::Conflicts {
                    new_sha,
                    files,
                    rewritten,
                }) => {
                    if rewritten {
                        task.record_decision(rebase_conflict_note(
                            &base_ref, &new_sha, &files, rewritten,
                        ));
                    }
                    task.record_carry_conflicts(files.clone());
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
                        task.record_decision(note);
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
                let wt = worktree.clone();
                let tree = tokio::task::spawn_blocking(move || {
                    crate::git::changes::worktree_tree(&wt).ok()
                })
                .await
                .unwrap_or(None);
                let base = task.base_sha.clone();
                observations::record_evidence(
                    &mut task,
                    idx,
                    EvidenceObservation::Captured {
                        files: saved,
                        tree,
                        base,
                    },
                );
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
                        crate::verify::healing::scope::own_changes_since(
                            &wt,
                            &old_tree,
                            old_base.as_deref(),
                            &now_base,
                        )
                    })
                    .await
                    .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())));
                    if let Ok(other) = diff {
                        if other.is_empty() && !old_evidence.is_empty() {
                            observations::record_evidence(
                                &mut task,
                                idx,
                                EvidenceObservation::Reused {
                                    files: old_evidence,
                                    from: m,
                                },
                            );
                            task.record_decision(format!(
                                "Orchestrator: attempt {n} changed no file of its own since attempt {m} (artifacts, tests and carried-in base commits do not count); reused attempt {m}'s evidence"
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
        observations::record_verify(
            &mut task,
            idx,
            VerifyObservation::Replaced(verify_results.clone()),
        );
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
                    observations::record_verify(
                        &mut task,
                        idx,
                        VerifyObservation::Replaced(verify_results.clone()),
                    );
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
                    crate::engine::verification::base::jobs::BaseCheckJob::new(
                        Some(&task.id),
                        &task.repo,
                        &task.base_sha,
                        &base_dir,
                        false,
                    ),
                    &failed,
                    &attempt_start_base,
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
                            observations::record_verify(
                                &mut task,
                                idx,
                                VerifyObservation::Replaced(verify_results.clone()),
                            );
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
                crate::engine::verification::runner::CheckRunner::from_context(
                    CheckContext::new(&app, Some(&task.id)),
                    &worktree,
                    sandbox.as_ref(),
                    &task.base_sha,
                    &cancel,
                ),
                &commands,
            )
            .await;
            record_check_outcomes(&app, &mut task, &results);
            if cancel.is_cancelled() {
                interrupt_attempt(&app, &mut task, idx);
                drop(permit);
                app.finish_task_loop(&task_id);
                return;
            }
            observations::record_verify(
                &mut task,
                idx,
                VerifyObservation::Appended(results.clone()),
            );
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
                    crate::engine::verification::runner::CheckRunner::from_context(
                        CheckContext::new(&app, Some(&task.id)),
                        &worktree,
                        app.task_sandbox(&task.repo, &worktree, &run_dir).as_ref(),
                        &task.base_sha,
                        &cancel,
                    ),
                    &task,
                    &held,
                )
                .await;
                record_check_outcomes(&app, &mut task, std::slice::from_ref(&result));
                if cancel.is_cancelled() {
                    interrupt_attempt(&app, &mut task, idx);
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                observations::record_verify(
                    &mut task,
                    idx,
                    VerifyObservation::Appended(vec![result.clone()]),
                );
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
                task.record_decision(note);
                task.add_review_follow_ups(follow);
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
            match fail_and_continue(
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

        if let Some(path) = changed
            .iter()
            .find(|f| matches_any_protected(f, &settings.protected_paths))
        {
            crate::task::attempts::end(
                &mut task,
                idx,
                crate::task::attempts::AttemptEnd::Blocked,
                now_ms(),
            );
            task.ask(
                Question::new(
                    format!("Change touches protected path {path}: approve or reject?"),
                    vec!["approve".into(), "reject".into()],
                    QuestionKind::ProtectedPath,
                    AskedBy::Implement,
                    crate::clock::now_ms(),
                ),
                task.updated_at,
            );
            task.resume_phase(TaskStatus::Waiting, task.updated_at);
            task.touch(now_ms());
            match wait_for_answer(&app, &mut task, &pending_answer, &cancel, &mut permit).await {
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

        loop {
            apply_pending_amendment(&app, &mut task, &pending_amend, &cancel).await;
            match heal_answers(&app, &mut task, attempt_n, &cancel).await {
                Ok(AnswerHealing::Ready) => break,
                Ok(AnswerHealing::Retry) => continue,
                Ok(AnswerHealing::Stopped) | Err(_) => {
                    interrupt_attempt(&app, &mut task, idx);
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }
        let mut review_result: Option<ReviewResult> = None;
        task.record_unreviewed(false);
        ladder::inherit_unreviewed(&app, &mut task);
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
                        task.record_decision(line);
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
                        if idx < t.attempts.len() {
                            observations::end(
                                &mut t,
                                idx,
                                observations::AttemptEnd::Interrupted,
                                now_ms(),
                            );
                        }
                        t.touch(now_ms());
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
                // The route a review that gave no verdict twice moves to, once.
                let mut review_route_now = review_route.clone();
                let mut fallback_tried = false;
                // A review run that errored is tried again after a pause,
                // twice, before it counts as no verdict.
                let mut harness_retries = 0u32;
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
                        &review_route_now,
                        &deny_read,
                        &review_cancel,
                        &mut review_cost,
                        &mut review_fingerprint,
                        after_no_verdict,
                        // Reserve the last review run for the whole change.
                        force_full || round == MAX_REVIEW_RUNS,
                    )
                    .await;
                    // The task's total, and separately the attempt's
                    // review_cost_usd for display: an attempt's own cost_usd is
                    // what later resumes of its session subtract
                    // (session cost accounting), so the review's cost must never join it.
                    observations::record_review_cost(
                        &mut task,
                        idx,
                        review_cost,
                        review_fingerprint.take(),
                    );
                    review_cost = 0.0;
                    if review_cancel.is_cancelled() && !cancel.is_cancelled() {
                        break;
                    }
                    let (why, no_verdict) = match reviewed {
                        Ok((mut r, recorded_as_pass, saw)) => {
                            observations::record_review_view(
                                &mut task,
                                idx,
                                Some(ReviewView {
                                    tree: saw.tree.clone(),
                                    base: base_sha.clone(),
                                    scope: saw.scope,
                                }),
                            );
                            // Rulings first: a narrow PASS is judged on the findings
                            // that are still open after them.
                            let declines = task.attempts[idx].declines.clone();
                            apply_declines(&mut task, attempt_n, &mut r, &declines, &saw.rulings);
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
                                    task.record_decision(format!(
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
                                task.record_decision(
                                    "Orchestrator: review FAIL recorded as PASS (only P2/P3 findings)"
                                        .to_string(),
                                );
                            }
                            if r.verdict == Verdict::Fail {
                                handle_repeated_findings(
                                    &app, &mut task, attempt_n, &mut r, &worktree, &cancel,
                                )
                                .await;
                                if cancel.is_cancelled() {
                                    interrupt_attempt(&app, &mut task, idx);
                                    drop(permit);
                                    app.finish_task_loop(&task_id);
                                    return;
                                }
                                if review_cancel.is_cancelled() && !cancel.is_cancelled() {
                                    break;
                                }
                                match heal_repeated_unmet(&app, &mut task, idx, &r, &cancel).await {
                                    Ok(true) => {
                                        task.touch(now_ms());
                                        app.save_task_logged(&task);
                                        app.broadcast_task(&task);
                                        if round < MAX_REVIEW_RUNS {
                                            round += 1;
                                            continue;
                                        }
                                        task.record_decision(review_stopped_note(attempt_n));
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
                        Err(ReviewFailure::Harness(why)) => {
                            if harness_retries < ladder::SETUP_RETRIES && !cancel.is_cancelled() {
                                harness_retries += 1;
                                task.record_decision(format!(
                                    "Orchestrator: attempt {attempt_n} review run failed ({why}); trying again ({harness_retries}/{})",
                                    ladder::SETUP_RETRIES
                                ));
                                if !ladder::sleep_unless_cancelled(
                                    ladder::setup_retry_delay(harness_retries),
                                    &cancel,
                                )
                                .await
                                {
                                    interrupt_attempt(&app, &mut task, idx);
                                    if let Some(finals) = final_task.take() {
                                        let _ = finals.finish().await;
                                    }
                                    drop(permit);
                                    app.finish_task_loop(&task_id);
                                    return;
                                }
                                continue;
                            }
                            (why, false)
                        }
                    };
                    if no_verdict && auto_rerun && round < MAX_REVIEW_RUNS {
                        auto_rerun = false;
                        round += 1;
                        after_no_verdict = true;
                        task.record_decision(format!(
                            "Orchestrator: attempt {attempt_n} review reply had no verdict; reviewing the same attempt again"
                        ));
                        continue;
                    }
                    if review_cancel.is_cancelled() && !cancel.is_cancelled() {
                        break;
                    }
                    // No verdict is never a PASS: another route reads the
                    // attempt once; if none does, it is committed marked
                    // unreviewed. The owner is not asked.
                    if !fallback_tried {
                        fallback_tried = true;
                        if let Some((alt, swap)) =
                            ladder::fallback_review_route(&app, &settings, &review_route_now)
                        {
                            for line in swap.into_iter().chain([format!(
                                "Orchestrator: attempt {attempt_n} review gave no verdict ({why}); reviewing on {}",
                                alt.id
                            )]) {
                                task.record_decision(line);
                            }
                            review_route_now = alt;
                            // One run beyond the usual two, on the whole change.
                            force_full = true;
                            round += 1;
                            after_no_verdict = no_verdict;
                            continue;
                        }
                    }
                    let question = Question::new(
                        format!(
                            "The review gave no verdict ({why}). Commit this attempt unreviewed?"
                        ),
                        vec!["approve".into()],
                        QuestionKind::ReviewNoVerdict,
                        AskedBy::Review,
                        crate::clock::now_ms(),
                    );
                    record_policy_answer(
                        &mut task,
                        &question,
                        "approve",
                        "policy",
                        "no review route gave a verdict; the attempt is committed unreviewed",
                        attempt_n,
                    );
                    task.record_unreviewed(true);
                    let after = if task.parent.is_some() {
                        "it lands on the parent, which then becomes unreviewed"
                    } else {
                        "it is not landed, the owner gets a pull request"
                    };
                    task.record_decision(format!(
                        "Orchestrator: attempt {attempt_n} committed unreviewed: no review route gave a verdict; {after}"
                    ));
                    break;
                }
            } else {
                let note = format!(
                    "Orchestrator: review skipped, no route matches review \"{}\"",
                    review_id
                );
                if !task.decisions.contains(&note) {
                    task.record_decision(note);
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
                let old_on_base = findings_old_on_base(&app, &task, r, &run_dir, &cancel).await;
                let follow_ups = scope_review(r, &old_on_base);
                if !follow_ups.is_empty() {
                    task.record_decision(if r.verdict == Verdict::Pass {
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
                task.record_decision(format!(
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
            task.record_decision(format!(
                "Orchestrator: attempt {attempt_n} review discarded because a final check failed"
            ));
            review_result = None;
            observations::record_review_view(&mut task, idx, None);
        } else {
            task.add_review_follow_ups(review_follow_ups);
        }
        observations::record_review(
            &mut task,
            idx,
            review_result
                .clone()
                .map(ReviewState::Recorded)
                .unwrap_or(ReviewState::Discarded),
        );
        refresh_criteria_results(&mut task, idx);

        if let Some(r) = &review_result {
            if r.verdict == Verdict::Fail {
                match fail_and_continue(
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
