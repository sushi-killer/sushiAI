use super::*;

/// The per-task attempt loop (spec "Engine", steps 1-10). Runs as its own
/// tokio task from `task.start` until the task reaches `done`/`failed`, is
/// stopped, or the daemon shuts down; `waiting` parks it on `pending_answer`
/// rather than exiting, so the in-memory attempt-budget extension (step 9)
/// survives across a wait/answer cycle.
pub(super) async fn run_task_loop(
    app: Arc<App>,
    task_id: String,
    pending_answer: Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    pending_amend: Arc<StdMutex<Option<Amendment>>>,
    cancel: CancelToken,
    auto_start_after_plan: bool,
) {
    let mut attempt_budget = app.settings.read().unwrap().max_attempts;
    // Set once a parent's finish failed after every subtask landed: the
    // failure its own implement attempt has to fix. While it is set (and once
    // the parent has an attempt) the parent never goes back to `run_parent`.
    let mut parent_failure: Option<String> = None;

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

        // A worktree removed while the task was archived comes back before
        // anything touches it.
        match app.ensure_worktree(&mut task).await {
            Ok(true) => {
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
            }
            Ok(false) => {}
            Err(e) => {
                task.decisions.push(format!("Worktree: {e}"));
                task.status = TaskStatus::Failed;
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
                app.finish_task_loop(&task_id);
                return;
            }
        }

        // The task graph: a parent never implements, and a task whose
        // dependencies are not all done waits `queued` with no loop until
        // `advance_graph` starts it again. Drafting runs meanwhile.
        // A subtask is planned only once everything it depends on has landed,
        // so no planning is spent on a part that cannot start yet.
        if !needs_planning(&task) || task.parent.is_some() {
            let all = app.repo_tasks(&task.repo);
            if is_parent(&task, &all)
                && parent_failure.is_none()
                && (implement_attempt_count(&task) == 0
                    || !matches!(waits_state(&task, &all), Waits::Ready | Waits::Nothing))
            {
                match run_parent(&app, &task_id, &pending_answer, &pending_amend, &cancel).await {
                    ParentEnd::Finished => {
                        app.finish_task_loop(&task_id);
                        return;
                    }
                    ParentEnd::Attempt { failure, budget } => {
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
                        // A parent parked on a question (its base check) starts
                        // no subtask until it is answered.
                        || (p.status == TaskStatus::Waiting
                            && implement_attempt_count(&task) == 0)
                });
            if parent_blocked
                || (!task.depends_on.is_empty()
                    && !matches!(waits_state(&task, &all), Waits::Ready | Waits::Nothing))
            {
                if task.status != TaskStatus::Queued || task.queue.queue_reason.is_some() {
                    task.status = TaskStatus::Queued;
                    task.queue.queue_reason = None;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                }
                app.finish_task_loop(&task_id);
                return;
            }
            // Paths another live task on this base holds: wait `queued`, no
            // slot taken, until that task lands, stops or fails.
            if !needs_planning(&task) {
                let force = task.status == TaskStatus::Landing
                    || implement_attempt_count(&task) > 0
                    || parent_failure.is_some();
                if !pass_lease_gate(&app, &mut task, force) {
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
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
            let settings = app.settings.read().unwrap().clone();
            let worktree = PathBuf::from(&task.worktree);
            match finish_attempt(
                &app,
                &task_id,
                &mut task,
                idx,
                attempt_n,
                &worktree,
                &run_dir,
                &settings,
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
            let _ = app.store.save_task(&task);
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
            match run_plan_stage(
                &app,
                &task_id,
                &pending_answer,
                &cancel,
                &mut permit,
                auto_start_after_plan,
            )
            .await
            {
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
        if implement_attempt_count(&task) == 0 && parent_failure.is_none() {
            let all = app.repo_tasks(&task.repo);
            if is_parent(&task, &all) {
                drop(permit);
                match run_parent(&app, &task_id, &pending_answer, &pending_amend, &cancel).await {
                    ParentEnd::Finished => {
                        app.finish_task_loop(&task_id);
                        return;
                    }
                    ParentEnd::Attempt { failure, budget } => {
                        parent_failure = Some(failure);
                        attempt_budget = budget.unwrap_or(attempt_budget);
                        continue 'attempts;
                    }
                }
            }
        }

        // Explicit criteria are checked once, before the first attempt; a
        // planned brief was already checked in the plan stage.
        if implement_attempt_count(&task) == 0 && !task.brief_check.done && parent_failure.is_none()
        {
            let attempt_n = task.attempts.len() as u32 + 1;
            if let BriefAction::Cancelled =
                check_brief(&app, &task_id, attempt_n, false, &cancel).await
            {
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
        if implement_attempt_count(&task) == 0
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
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
                drop(permit);
                continue 'attempts;
            }
        }

        // Every check command runs on the base once: one the repository
        // cannot run is rewritten, one broken for an unrelated reason stops
        // gating, before an attempt is spent on either.
        if implement_attempt_count(&task) == 0
            && !task.brief_check.feasibility_done
            && parent_failure.is_none()
        {
            let attempt_n = task.attempts.len() as u32 + 1;
            if !heal_feasibility(&app, &task_id, attempt_n, &cancel).await {
                drop(permit);
                mark_stopped_if_not_already(&app, &task_id).await;
                app.finish_task_loop(&task_id);
                return;
            }
            if let Ok(Some(reloaded)) = app.store.load_task(&task_id) {
                task = reloaded;
            }
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
            if implement_attempt_count(&task) == 0 && task.queue.relay_base.is_none() {
                start_relay_link(&app, &mut task, &prev_id).await;
            }
        }
        if implement_attempt_count(&task) == 0
            && parent_failure.is_none()
            && (task.parent.is_some() || !task.depends_on.is_empty() || task.queue.waited_on_lease)
            && task.queue.relay_of.is_none()
        {
            if let Some(base_ref) = task.base_ref.clone() {
                let (wt, from, tid, r) = (
                    PathBuf::from(&task.worktree),
                    task.base_sha.clone(),
                    task_id.clone(),
                    base_ref.clone(),
                );
                let synced = tokio::task::spawn_blocking(move || {
                    git::carry_onto_moved_base(&wt, &r, &from, &tid)
                })
                .await;
                if let Ok(Ok(git::Rebase::Moved { new_sha, .. })) = synced {
                    task.decisions.push(format!(
                        "Rebase: started from {base_ref} at {}",
                        short_sha(&new_sha)
                    ));
                    task.base_sha = new_sha;
                }
            }
        }
        task.queue.waited_on_lease = false;

        if task.variant().grounded_checks
            && implement_attempt_count(&task) == 0
            && needs_baseline(&task)
        {
            let wt = PathBuf::from(&task.worktree);
            if !baseline_checks(&app, &task_id, &mut task, &wt, &cancel).await {
                mark_stopped_if_not_already(&app, &task_id).await;
                drop(permit);
                app.finish_task_loop(&task_id);
                return;
            }
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
        }
        apply_pending_amendment(&app, &mut task, &pending_amend, &cancel).await;

        // A check that already fails on the base can never be met by the
        // work: ask about it now, before an attempt is spent on it.
        if implement_attempt_count(&task) == 0 && parent_failure.is_none() {
            let unanswered: Vec<String> = task
                .final_verify
                .iter()
                .filter(|c| !owner_answered_base_check(&task, c, &task.base_sha))
                .cloned()
                .collect();
            if let Some(base_run) = failing_twice_on_base(&app, &task, &unanswered, &cancel).await {
                let question =
                    pre_existing_question(&base_run.command, &task.base_sha, &base_run.tail);
                task.question = Some(question.clone());
                task.status = TaskStatus::Waiting;
                task.updated_at = now_ms();
                let assumed = task.assumptions.len();
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
                let policy_answered = task.assumptions.len() > assumed;
                let base_sha = task.base_sha.clone();
                record_owner_base_check(
                    &mut task,
                    &base_run.command,
                    &base_sha,
                    &answer,
                    policy_answered,
                );
                if is_option(&answer, PRE_EXISTING_DROP) {
                    apply_base_check_answer(&mut task, &question, &answer);
                    app.verify_cache.lock().unwrap().remove(&task_id);
                } else {
                    // Retry: on the newest base. On an unmoved one the
                    // answered pair keeps the preflight from asking again.
                    let wt = PathBuf::from(&task.worktree);
                    carry_onto_newest_base(&mut task, &wt).await;
                }
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
                drop(permit);
                continue 'attempts;
            }
        }

        let mut planner_tier_used = false;
        let mut no_planner_tier = false;
        if implement_attempt_count(&task) == 0 {
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

        let settings = app.settings.read().unwrap().clone();
        let attempt_n = implement_attempt_count(&task) + 1;
        // A landing that conflicted only needs its markers resolved: that
        // runs on the cheap route, not as a full re-implementation.
        let conflict_only = last_failure_is_conflict(&task);
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
                "Orchestrator: the landing conflicted -> conflict-only attempt on the cheap route {route_id}"
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
                strength: None,
            });

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
                let _ = app.store.save_task(&task);
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
            brief::with_block_before_report(&brief_text, &brief::conflict_only_block())
        } else {
            brief_text
        };
        let brief_text = match &task.brief_check.conflict {
            Some(conflict) if implement_attempt_count(&task) == 0 => {
                brief::with_block_before_report(
                    &brief_text,
                    &brief::conflict_block(conflict, task.variant().grounded_checks),
                )
            }
            _ => brief_text,
        };
        let brief_text = brief::with_block_before_report(
            &brief_text,
            &brief::landed_dependencies_block(&task, &app.repo_tasks(&task.repo), None),
        );
        let brief_text = if implement_attempt_count(&task) == 0 {
            brief::with_block_before_report(
                &brief_text,
                &brief::relay_block(&task, &app.repo_tasks(&task.repo)),
            )
        } else {
            brief_text
        };
        // A parent's first attempt gets what failed when it tried to finish.
        let brief_text = match &parent_failure {
            Some(failure) if implement_attempt_count(&task) == 0 => {
                brief::with_block_before_report(&brief_text, &brief::parent_failure_block(failure))
            }
            _ => brief_text,
        };
        // The messages sent to this task since its last attempt are
        // delivered here, with the attempt about to start.
        let brief_text = brief::with_block_before_report(
            &brief_text,
            &messages::brief_block_for_task(&app, &task_id),
        );
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
        let brief_text =
            brief::with_block_before_report(&brief_text, &permissions::tools_block(&tools, false));

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
        task.attempts.push(attempt);
        let idx = task.attempts.len() - 1;
        refresh_criteria_results(&mut task, idx);
        task.status = TaskStatus::Running;
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);

        let run_dir = app.store.run_dir(&task_id, attempt_n);
        let _ = std::fs::create_dir_all(&run_dir);
        let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
        let PreparedRun {
            mcp_path,
            settings_path,
            key_path,
            messages_server,
            mut registered,
        } = prepare_run(
            &app, &task, &task_id, &route, &settings, &worktree, &base_sha, attempt_n, &run_dir,
            &deny_read, &tools,
        );

        let network_allowed = settings.codex_network;
        let req = harness::RunRequest {
            harness: route.harness,
            worktree: &worktree,
            model: route.model.as_deref(),
            effort: route.effort.as_deref(),
            max_budget_usd: (task.variant().max_attempt_cost_usd > 0.0)
                .then(|| task.variant().max_attempt_cost_usd),
            review: false,
            mcp_config: Some(&mcp_path),
            settings_path: Some(&settings_path),
            network_allowed,
            codex_mcp: Some((messages::SERVER, &messages_server)),
            images: &[],
            repo_settings: true,
        };

        let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
        let events_path = run_dir.join("events.jsonl");
        let a_run = run_harness(
            &app,
            &task_id,
            attempt_n,
            true,
            &worktree,
            &req,
            CostTag::task("implement", &route.id),
            &brief_text,
            &events_path,
            &cancel,
            (task.variant().stall_timeout_secs > 0).then(|| Stall {
                limit: Duration::from_secs(task.variant().stall_timeout_secs),
                paused: registered
                    .as_ref()
                    .map(|(_, ctx)| ctx.hook_running.clone())
                    .unwrap_or_default(),
            }),
            task.variant().loop_detect.then(LoopDetector::new),
        );
        // Recorded after the reload below, which would drop a line pushed now.
        let mut no_second = BestOfExtra::default();
        let second = match second_route(&task, &settings, &route, attempt_n) {
            Ok(second) => second,
            Err(line) => {
                no_second.decisions.push(line);
                None
            }
        };
        let (run_result, best_of) = match second {
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

        let (gate_blocks, handled, staged_lines) = if let Some((tok, ctx)) = registered.take() {
            app.hook_tokens.write().unwrap().remove(&tok);
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
        task.decisions.extend(best_of.decisions);
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
                settle_unfinished_cost(&mut task, idx, &run_dir, &settings.prices);
                task.status = app.cancelled_status(&task.status);
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
                drop(permit);
                app.finish_task_loop(&task_id);
                return;
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

        if outcome.stalled || outcome.over_budget {
            let kind = if outcome.over_budget {
                FailureKind::Budget
            } else {
                FailureKind::Stall
            };
            let detail = outcome.error.clone().unwrap_or_default();
            let (wt, base) = (worktree.clone(), base_sha.clone());
            task.attempts[idx].changed_files = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &base).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            record_diff_stat(&mut task, idx, &worktree, &base_sha).await;
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
            let (wt, base) = (worktree.clone(), base_sha.clone());
            task.attempts[idx].changed_files = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &base).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            record_diff_stat(&mut task, idx, &worktree, &base_sha).await;
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
            let (wt, base) = (worktree.clone(), base_sha.clone());
            task.attempts[idx].changed_files = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &base).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            record_diff_stat(&mut task, idx, &worktree, &base_sha).await;
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
        let base3 = base_sha.clone();
        let changed = tokio::task::spawn_blocking(move || {
            git::changed_files(&wt3, &base3).unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        task.attempts[idx].changed_files = changed.clone();
        record_diff_stat(&mut task, idx, &worktree, &base_sha).await;

        if task.variant().grounded_checks {
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
                            drop_criterion(&mut task, n);
                            task.updated_at = now_ms();
                            let _ = app.store.save_task(&task);
                            app.broadcast_task(&task);
                        }
                        drop(permit);
                        continue;
                    }
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
                let _ = app.store.save_task(&task);
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
            task.attempts[idx].status = AttemptStatus::Interrupted;
            task.attempts[idx].ended_at = Some(now_ms());
            task.status = app.cancelled_status(&task.status);
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }

        // The base branch may have moved while the agent worked (another
        // task merged, the owner committed): carry the work onto it so verify
        // and review judge the tree that would actually land. Conflicts go
        // back to the agent as a failure; it resolves them, not this code.
        let mut base_sha = base_sha;
        let mut changed = changed;
        if let Some(base_ref) = task.base_ref.clone() {
            let (wt, from, tid, r) = (
                worktree.clone(),
                base_sha.clone(),
                task_id.clone(),
                base_ref.clone(),
            );
            let carried = tokio::task::spawn_blocking(move || {
                git::carry_onto_moved_base(&wt, &r, &from, &tid)
            })
            .await
            .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
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
                    let (wt, b) = (worktree.clone(), base_sha.clone());
                    changed = tokio::task::spawn_blocking(move || {
                        git::changed_files(&wt, &b).unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default();
                    task.attempts[idx].changed_files = changed.clone();
                    record_diff_stat(&mut task, idx, &worktree, &base_sha).await;
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
                        let _ = app.store.save_task(&task);
                        app.broadcast_task(&task);
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
                let _ = app.store.save_task(&task);
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
                            let _ = app.store.save_task(&task);
                            app.broadcast_task(&task);
                        } else {
                            stale_evidence = Some((m, other.into_iter().take(5).collect()));
                        }
                    }
                }
            }
        }
        apply_pending_amendment(&app, &mut task, &pending_amend, &cancel).await;
        let (verify_commands, skipped) =
            scope_to_diff(&app, &task.repo, &worktree, &base_sha, &task.verify).await;
        record_skips(&mut task, skipped);
        let mut verify_results = run_verify_cached(
            &app,
            &task_id,
            &worktree,
            &run_dir,
            &base_sha,
            &verify_commands,
            &cancel,
        )
        .await;
        if cancel.is_cancelled() {
            // Killed by the shutdown, not failed: the next daemon re-runs it.
            interrupt_attempt(&app, &mut task, idx);
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }
        task.attempts[idx].verify = verify_results.clone();
        if let Some(failed) = verify_results.iter().find(|v| v.code != Some(0)) {
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
                    continue;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        if task.variant().grounded_checks {
            // Never through the verify cache: it holds the verify commands'
            // results only.
            let mut failure: Option<(FailureKind, String)> = None;
            let gated = gated_checks(&task);
            if !gated.is_empty() {
                let commands: Vec<String> = gated.iter().map(|c| c.run.clone()).collect();
                let results = run_verify_commands(
                    &worktree,
                    &run_dir.join("checks"),
                    &commands,
                    settings.sandbox,
                    &task.base_sha,
                    &cancel,
                )
                .await;
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
                        &task,
                        &held,
                        &worktree,
                        &run_dir.join("checks"),
                        settings.sandbox,
                        &cancel,
                    )
                    .await;
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
        }

        // The planner named the command that captures the images: orchd
        // runs it itself, so an attempt is never failed for a missing image.
        capture_screenshots(
            &mut task,
            idx,
            &worktree,
            &run_dir,
            settings.sandbox,
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
            let criteria = visual
                .iter()
                .map(|c| format!("- {c}"))
                .collect::<Vec<_>>()
                .join("\n");
            let mut detail = if missing_named.is_empty() {
                format!(
                    "These criteria are visual and no image was saved under artifacts/ during this attempt:\n{criteria}\nSave one screenshot per criterion under artifacts/ (for example artifacts/<name>.png) and look at it before finishing."
                )
            } else {
                format!(
                    "These criteria are visual and the images they name were not saved during this attempt: {}\n{criteria}\nSave each named file and look at it before finishing; other images under artifacts/ are not evidence.",
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
        if !settings.review.is_empty() {
            if let Some((review_route, why)) = select_review_route(&settings, &route, task.tier) {
                let note = format!("Orchestrator: review: {} ({why})", review_route.id);
                if !task.decisions.contains(&note) {
                    task.decisions.push(note);
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
                        if let Some(a) = t.attempts.get_mut(idx) {
                            a.status = AttemptStatus::Interrupted;
                            a.ended_at = Some(now_ms());
                        }
                        t.updated_at = now_ms();
                        let _ = app.store.save_task(&t);
                        app.broadcast_task(&t);
                    }
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                let mut review_cost = 0.0;
                let mut review_fingerprint = None;
                // Every review run of this attempt: the first, the one
                // automatic re-run after a reply with no verdict, and any the
                // owner or the policy asks for with `retry`. Same diff, no new
                // implement attempt.
                let mut round = 1u32;
                let mut after_no_verdict = false;
                let mut auto_rerun = true;
                // Findings a judge dropped in this attempt, and its judge runs.
                let mut dropped: Vec<(String, String)> = Vec::new();
                let mut judge_runs = 0u32;
                loop {
                    let reviewed = run_review(
                        &app,
                        &task_id,
                        attempt_n,
                        &task,
                        &worktree,
                        &base_sha,
                        &verify_results,
                        &implementer_note,
                        &agent_decisions,
                        review_route,
                        &deny_read,
                        &cancel,
                        &mut review_cost,
                        &mut review_fingerprint,
                        round,
                        after_no_verdict,
                        &dropped,
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
                    let (why, no_verdict) = match reviewed {
                        Ok((r, recorded_as_pass)) => {
                            if recorded_as_pass {
                                task.decisions.push(
                                    "Orchestrator: review FAIL recorded as PASS (only P2/P3 findings)"
                                        .to_string(),
                                );
                            }
                            if r.verdict == Verdict::Fail {
                                let task_tier = task.tier;
                                if !r.repeated.is_empty() && !task.attempts[idx].disputes.is_empty()
                                {
                                    let newly = judge_disputed_findings(
                                        &app,
                                        &mut task,
                                        idx,
                                        attempt_n,
                                        &r.repeated,
                                        &settings,
                                        review_route,
                                        &route,
                                        task_tier,
                                        &mut judge_runs,
                                        &cancel,
                                    )
                                    .await;
                                    if !newly.is_empty() {
                                        dropped.extend(newly);
                                        round += 1;
                                        after_no_verdict = false;
                                        continue;
                                    }
                                }
                                match heal_repeated_unmet(&app, &mut task, idx, &r, &cancel).await {
                                    Ok(true) => {
                                        task.updated_at = now_ms();
                                        let _ = app.store.save_task(&task);
                                        app.broadcast_task(&task);
                                        round += 1;
                                        continue;
                                    }
                                    Ok(false) => {}
                                    Err(_) => {
                                        task.attempts[idx].status = AttemptStatus::Interrupted;
                                        task.attempts[idx].ended_at = Some(now_ms());
                                        task.status = app.cancelled_status(&task.status);
                                        task.updated_at = now_ms();
                                        let _ = app.store.save_task(&task);
                                        app.broadcast_task(&task);
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
                            // A cancelled review is never a PASS: the attempt
                            // (and the task) is simply stopped.
                            task.attempts[idx].status = AttemptStatus::Interrupted;
                            task.attempts[idx].ended_at = Some(now_ms());
                            task.status = app.cancelled_status(&task.status);
                            task.updated_at = now_ms();
                            let _ = app.store.save_task(&task);
                            app.broadcast_task(&task);
                            drop(permit);
                            app.finish_task_loop(&task_id);
                            return;
                        }
                        Err(ReviewFailure::NoVerdict(why)) => (why, true),
                        Err(ReviewFailure::Harness(why)) => (why, false),
                    };
                    if no_verdict && auto_rerun {
                        auto_rerun = false;
                        round += 1;
                        after_no_verdict = true;
                        task.decisions.push(format!(
                            "Orchestrator: attempt {attempt_n} review reply had no verdict; reviewing the same attempt again"
                        ));
                        continue;
                    }
                    // No verdict is never a PASS: the owner decides whether
                    // to commit this attempt unreviewed.
                    task.attempts[idx].status = AttemptStatus::Blocked;
                    task.attempts[idx].ended_at = Some(now_ms());
                    task.question = Some(Question::new(
                        format!(
                            "The review gave no verdict ({why}). Commit this attempt unreviewed?"
                        ),
                        vec!["approve".into(), "retry".into()],
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
                        Some(answer) if answer == "retry" => {
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
                                criteria: Vec::new(),
                            });
                            break;
                        }
                    }
                }
            } else {
                let note = format!(
                    "Orchestrator: review skipped, no route matches review \"{}\"",
                    settings.review
                );
                if !task.decisions.contains(&note) {
                    task.decisions.push(note);
                }
            }
        }
        if let Some(r) = review_result.as_mut() {
            // After the first attempt only P1+ findings about the task's own
            // work cost an attempt; the rest go to the report.
            if r.verdict == Verdict::Fail && attempt_n >= 2 {
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
                    add_follow_ups(&mut task, follow_ups);
                }
            } else if r.verdict == Verdict::Pass {
                add_follow_ups(&mut task, r.findings.clone());
            }
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

/// Records the worktree's diff against `base` on the attempt and the task.
async fn record_diff_stat(task: &mut Task, idx: usize, worktree: &Path, base: &str) {
    let (wt, base) = (worktree.to_path_buf(), base.to_string());
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

/// Carries the task's work onto its base branch as it is now, so a retry
/// after "the base is fixed" runs on the fix. Nothing happens when the task
/// has no base branch or it did not move.
pub(super) async fn carry_onto_newest_base(task: &mut Task, worktree: &Path) {
    let Some(base_ref) = task.base_ref.clone() else {
        return;
    };
    let (wt, from, tid, r) = (
        worktree.to_path_buf(),
        task.base_sha.clone(),
        task.id.clone(),
        base_ref.clone(),
    );
    let carried =
        tokio::task::spawn_blocking(move || git::carry_onto_moved_base(&wt, &r, &from, &tid)).await;
    match carried {
        Ok(Ok(git::Rebase::Moved { new_sha, rewritten })) => {
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
            task.base_sha = new_sha;
        }
        Ok(Ok(git::Rebase::Conflicts {
            new_sha,
            files,
            rewritten,
        })) => {
            task.decisions
                .push(rebase_conflict_note(&base_ref, &new_sha, &files, rewritten));
            task.base_sha = new_sha;
        }
        _ => {}
    }
}

/// A daemon shutdown killed the attempt's run: it is interrupted, never a
/// failure, and the next daemon resumes the task.
fn interrupt_attempt(app: &Arc<App>, task: &mut Task, idx: usize) {
    task.attempts[idx].status = AttemptStatus::Interrupted;
    task.attempts[idx].ended_at = Some(now_ms());
    task.status = app.cancelled_status(&task.status);
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
}

pub(super) async fn mark_stopped_if_not_already(app: &Arc<App>, task_id: &str) {
    if let Ok(Some(mut task)) = app.store.load_task(task_id) {
        if !matches!(
            task.status,
            TaskStatus::Stopped | TaskStatus::Done | TaskStatus::Failed
        ) {
            task.status = app.cancelled_status(&task.status);
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
        }
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
        self.controls.lock().unwrap().remove(task_id);
        if let Ok(Some(task)) = self.store.load_task(task_id) {
            self.advance_graph(&task.repo);
        }
        self.record_evolution_signals(task_id);
    }
}

pub(super) enum Tail {
    Continue,
    Return,
}

/// A passed review (or an accepted attempt): the task's final checks run on
/// the worktree, then `finish_attempt` commits or lands it. A failing final
/// check is a `Verify` failure -- or, when it already failed on the base, a
/// question to the owner.
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
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Tail {
    if !task.final_verify.is_empty() {
        // Not through the verify cache: it keys on the diff alone and
        // would hand back the fast checks' results.
        let (final_commands, skipped) = scope_to_diff(
            app,
            &task.repo,
            worktree,
            &task.base_sha,
            &task.final_verify,
        )
        .await;
        record_skips(task, skipped);
        let final_results = run_verify_commands(
            worktree,
            &run_dir.join("final"),
            &final_commands,
            settings.sandbox,
            &task.base_sha,
            cancel,
        )
        .await;
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
        for failed in &failed_finals {
            let detail = format!(
                "Final check {} exited {}.\n{}",
                failed.command,
                failed
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                failed.tail
            );
            let base_dir = run_dir.join("final-base");
            let on_base = failing_on_base(app, task, failed, &base_dir, cancel).await;
            let Some(base_run) = on_base else {
                match fail_and_continue(
                    app,
                    task_id,
                    task,
                    idx,
                    FailureKind::Verify,
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
            };
            // The failure is older than this task: no retry can fix it.
            // The owner decides what the check is worth.
            record_failure(task, idx, FailureKind::Verify, detail);
            task.question = Some(pre_existing_question(
                &failed.command,
                &task.base_sha,
                &base_run.tail,
            ));
            task.status = TaskStatus::Waiting;
            task.updated_at = now_ms();
            let assumed = task.assumptions.len();
            let Some(answer) =
                wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await
            else {
                *permit = None;
                app.finish_task_loop(task_id);
                return Tail::Return;
            };
            let policy_answered = task.assumptions.len() > assumed;
            let base_sha = task.base_sha.clone();
            record_owner_base_check(task, &failed.command, &base_sha, &answer, policy_answered);
            if is_option(&answer, PRE_EXISTING_DROP) {
                task.final_verify.retain(|c| c != &failed.command);
                task.decisions.push(format!(
                    "Orchestrator: dropped final check {}",
                    failed.command
                ));
                // The check no longer counts: the attempt is not failed.
                let a = &mut task.attempts[idx];
                a.status = AttemptStatus::Running;
                a.ended_at = None;
                a.failure = None;
                task.status = TaskStatus::Running;
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
                app.broadcast_task(task);
                continue;
            }
            // Retry (any other answer): one more attempt on the newest
            // base, even when the attempt budget is spent.
            carry_onto_newest_base(task, worktree).await;
            *attempt_budget = (*attempt_budget).max(implement_attempt_count(task) + 1);
            task.updated_at = now_ms();
            let _ = app.store.save_task(task);
            app.broadcast_task(task);
            *permit = None;
            return Tail::Continue;
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
        settings,
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
    settings: &Settings,
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
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
                let _ = app.store.save_task(task);
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
                let _ = app.store.save_task(task);
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
                task.status = app.cancelled_status(&task.status);
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
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
                task.eval_check = Some(
                    run_eval_check(
                        Path::new(&task.repo),
                        run_dir,
                        &sha,
                        &cmd,
                        settings.sandbox,
                        cancel,
                    )
                    .await,
                );
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
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    *permit = None;
    app.finish_task_loop(task_id);
    Tail::Return
}
