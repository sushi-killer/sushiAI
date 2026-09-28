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
    cancel: CancelToken,
    auto_start_after_plan: bool,
) {
    let mut attempt_budget = app.settings.read().unwrap().max_attempts;
    // Never resume immediately after a waiting/answer cycle -- a fresh
    // brief carries the owner's answer instead (spec item 11). Reset each
    // iteration; set back to `true` only when this iteration itself ends
    // via an answered wait.
    let mut just_answered = false;

    loop {
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

        // The task graph: a parent never implements, and a task whose
        // dependencies are not all done waits `queued` with no loop until
        // `advance_graph` starts it again. Drafting runs meanwhile.
        if !needs_planning(&task) {
            let all = app.repo_tasks(&task.repo);
            if is_parent(&task, &all) {
                run_parent(&app, &task_id, &cancel).await;
                app.finish_task_loop(&task_id);
                return;
            }
            let parent_blocked = task
                .parent
                .as_deref()
                .and_then(|pid| all.iter().find(|t| t.id == pid))
                .is_some_and(|p| {
                    !matches!(own_waits_state(p, &all), Waits::Ready | Waits::Nothing)
                });
            if parent_blocked
                || (!task.depends_on.is_empty()
                    && !matches!(waits_state(&task, &all), Waits::Ready | Waits::Nothing))
            {
                if task.status != TaskStatus::Queued {
                    task.status = TaskStatus::Queued;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                }
                app.finish_task_loop(&task_id);
                return;
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
        if implement_attempt_count(&task) == 0 {
            let all = app.repo_tasks(&task.repo);
            if is_parent(&task, &all) {
                drop(permit);
                run_parent(&app, &task_id, &cancel).await;
                app.finish_task_loop(&task_id);
                return;
            }
        }

        let resume_eligible = !just_answered;
        just_answered = false;

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
        if implement_attempt_count(&task) == 0
            && (task.parent.is_some() || !task.depends_on.is_empty())
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
                if let Ok(Ok(git::Rebase::Moved { new_sha })) = synced {
                    task.decisions.push(format!(
                        "Rebase: started from {base_ref} at {}",
                        short_sha(&new_sha)
                    ));
                    task.base_sha = new_sha;
                }
            }
        }

        let mut jev_tier_choice = None;
        let mut planner_tier_used = false;
        let mut tier_fallback_reason = None;
        if implement_attempt_count(&task) == 0 {
            match task.planned_tier.filter(|_| task.variant().planner_tier) {
                Some(tier) => {
                    task.tier = tier;
                    task.tier_fallback = None;
                    planner_tier_used = true;
                }
                None => match classify_tier(&app, &task).await {
                    TierPick::Classified { tier, choice, p } => {
                        task.tier = tier;
                        task.tier_fallback = None;
                        jev_tier_choice = Some((choice, p));
                    }
                    TierPick::Fallback { reason } => {
                        task.tier = Tier::Standard;
                        task.tier_fallback = Some(reason.clone());
                        tier_fallback_reason = Some(reason);
                    }
                },
            }
        }

        let settings = app.settings.read().unwrap().clone();
        let attempt_n = implement_attempt_count(&task) + 1;
        let (route_id, overridden) = task.variant().implement_route_id(&settings, task.tier);
        if overridden {
            let line = variant_route_line(&format!("tier {}", task.tier.as_str()), &route_id);
            if !task.decisions.contains(&line) {
                task.decisions.push(line);
            }
        }
        if let Some((choice, p)) = jev_tier_choice {
            task.decisions.push(jev_tier_line(&choice, p, &route_id));
        }
        if planner_tier_used {
            task.decisions.push(format!(
                "Planner: tier {} -> route {route_id}",
                task.tier.as_str()
            ));
        }
        if let Some(reason) = tier_fallback_reason {
            task.decisions
                .push(jev_tier_fallback_line(&reason, &route_id));
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
            });

        // The plan attempt (if any) is never a resume candidate -- it's a
        // different route/harness shape entirely, and has nothing to do
        // with the implement route's own session history.
        let prev = task
            .attempts
            .iter()
            .rev()
            .find(|a| a.stage == Stage::Implement)
            .cloned();
        let resume_session = if resume_eligible && task.variant().retry_mode == RetryMode::Resume {
            prev.as_ref().and_then(|p| {
                let route_matches = p.route_id == route.id;
                let has_session = p.session_id.is_some();
                let verify_failure =
                    p.failure.as_ref().map(|f| f.kind) == Some(FailureKind::Verify);
                let was_interrupted = p.status == AttemptStatus::Interrupted;
                if route_matches && has_session && (verify_failure || was_interrupted) {
                    p.session_id.clone()
                } else {
                    None
                }
            })
        } else {
            None
        };

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

        if task.variant().advisor {
            if let Some(cost) = run_advisor_before_retry(
                &app, &mut task, &settings, &route, &worktree, &base_sha, &cancel,
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

        let brief_text = match (
            &resume_session,
            prev.as_ref().and_then(|p| p.failure.as_ref()),
        ) {
            (Some(_), Some(failure)) => brief::build_resume_delta(failure),
            _ => brief::build_brief(&task, &status_short, &diff_stat),
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
            Some(advice) if task.variant().advisor => {
                brief::with_block_before_report(&brief_text, &brief::advisor_block(advice))
            }
            _ => brief_text,
        };

        let reason = format!("tier {} -> route {}", task.tier.as_str(), route.id);
        let attempt = Attempt {
            n: attempt_n,
            stage: Stage::Implement,
            route_id: route.id.clone(),
            harness: route.harness,
            model: route.model.clone().unwrap_or_default(),
            reason,
            session_id: None,
            pgid: None,
            resumed: resume_session.is_some(),
            started_at: now_ms(),
            ended_at: None,
            status: AttemptStatus::Running,
            summary: None,
            handoff: None,
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
            advice: None,
            advisor_cost_usd: None,
        };
        task.attempts.push(attempt);
        let idx = task.attempts.len() - 1;
        task.status = TaskStatus::Running;
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);

        let run_dir = app.store.run_dir(&task_id, attempt_n);
        let _ = std::fs::create_dir_all(&run_dir);
        let mcp_path = run_dir.join("mcp.json");
        let task_mcp_path = app.store.task_dir(&task_id).join("mcp.json");
        // The project's own servers, plus orchd's messaging bridge scoped to
        // this task.
        let messages_server = messages::task_server(&app, &task_id);
        let mut mcp_config = std::fs::read_to_string(&task_mcp_path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .filter(|v| v.get("mcpServers").is_some_and(|s| s.is_object()))
            .unwrap_or_else(|| json!({"mcpServers": {}}));
        mcp_config["mcpServers"][messages::SERVER] = messages_server.clone();
        let _ = store::write_json_atomic(&mcp_path, &mcp_config);

        let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
        let token = uuid::Uuid::new_v4().to_string();
        let settings_path = run_dir.join("settings.json");
        let key_path = run_dir.join("key");
        // Keep our own clone of the hook context alongside the one handed
        // to `hook.stop` lookups, so we can read back how many times it
        // blocked into `attempt.gateBlocks` once the run ends.
        let mut registered: Option<(String, Arc<HookContext>)> = None;
        if matches!(route.harness, Harness::Claude) {
            let profile = route
                .profile_id
                .as_ref()
                .and_then(|pid| app.secrets.read().unwrap().profiles.get(pid).cloned());
            let mut profile_obj = serde_json::Map::new();
            if let Some(p) = &profile {
                if !p.env.is_empty() {
                    profile_obj.insert(
                        "env".to_string(),
                        serde_json::to_value(&p.env).unwrap_or(serde_json::Value::Null),
                    );
                }
                if let Some(key) = &p.key {
                    if store::write_secret_file(&key_path, key).is_ok() {
                        profile_obj.insert(
                            "apiKeyHelper".to_string(),
                            serde_json::Value::String(format!(
                                "cat {}",
                                harness::shell_quote(&key_path.to_string_lossy())
                            )),
                        );
                    }
                }
            }
            let profile_value = if profile_obj.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(profile_obj))
            };

            let socket_path_str = app.socket_path.to_string_lossy().to_string();
            let stop_hook = harness::StopHook {
                orchd_path: &app.orchd_path,
                socket_path: &socket_path_str,
                token: &token,
                lean_output: task.variant().lean_output,
            };
            let mut claude_settings = harness::build_claude_settings(
                profile_value.as_ref(),
                settings.sandbox,
                &settings.allowed_domains,
                &deny_read,
                Some(stop_hook),
            );
            // A headless run answers no prompts, so the messaging tools must
            // be allowed up front to be usable at all.
            if let Some(allow) = claude_settings["permissions"]["allow"].as_array_mut() {
                for tool in crate::mcp::TASK_TOOLS {
                    allow.push(json!(format!("mcp__{}__{tool}", messages::SERVER)));
                }
            }
            let _ = store::write_json_atomic(&settings_path, &claude_settings);
            let ctx = Arc::new(HookContext {
                task_id: task_id.clone(),
                attempt_n,
                worktree: worktree.clone(),
                base_sha: base_sha.clone(),
                verify: task.verify.clone(),
                blocks: AtomicU32::new(0),
                cancel: CancelToken::new(),
                hook_running: Arc::new(AtomicBool::new(false)),
            });
            app.hook_tokens
                .write()
                .unwrap()
                .insert(token.clone(), ctx.clone());
            registered = Some((token.clone(), ctx));
        }

        let network_allowed = settings.codex_network;
        let req = harness::RunRequest {
            harness: route.harness,
            worktree: &worktree,
            model: route.model.as_deref(),
            effort: route.effort.as_deref(),
            resume: resume_session.as_deref(),
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
        let run_result = run_harness(
            &app,
            &task_id,
            attempt_n,
            true,
            &worktree,
            &req,
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
        )
        .await;

        let gate_blocks = if let Some((tok, ctx)) = registered.take() {
            app.hook_tokens.write().unwrap().remove(&tok);
            ctx.cancel.cancel();
            ctx.blocks.load(Ordering::SeqCst)
        } else {
            0
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

        let outcome = match run_result {
            Ok(o) => o,
            Err(RunError::Cancelled) => {
                task.attempts[idx].status = AttemptStatus::Interrupted;
                task.attempts[idx].ended_at = Some(now_ms());
                settle_unfinished_cost(&mut task, idx, &run_dir, &settings.prices);
                task.status = TaskStatus::Stopped;
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
                    LoopSignal::Continue { answered } => {
                        just_answered = answered;
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

        task.attempts[idx].session_id = outcome.session_id.clone();
        task.attempts[idx].usage = Some(Usage {
            input: outcome.usage_input,
            output: outcome.usage_output,
            cached: outcome.usage_cached,
        });
        // A resumed first turn also carries the whole earlier conversation.
        if route.harness == Harness::Claude && resume_session.is_none() {
            task.attempts[idx].prefix_tokens = outcome.first_turn_tokens;
        }
        let cost = outcome.cost_usd.map(|total| {
            attempt_cost(
                &task.attempts[..idx],
                &task.attempts[idx],
                total,
                outcome.cost_estimated,
            )
        });
        task.attempts[idx].cost_usd = cost;
        task.attempts[idx].cost_estimated = outcome.cost_estimated;
        if let Some(cost) = cost {
            task.cost_usd += cost;
        }

        if outcome.stalled {
            let detail = outcome.error.clone().unwrap_or_default();
            let (wt, base) = (worktree.clone(), base_sha.clone());
            task.attempts[idx].changed_files = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &base).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                FailureKind::Stall,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue { answered } => {
                    just_answered = answered;
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
                LoopSignal::Continue { answered } => {
                    just_answered = answered;
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
        let implementer_note = match report.as_ref() {
            Some(r) if !r.summary.trim().is_empty() || !r.decisions.is_empty() => {
                let account = std::iter::once(r.summary.trim().to_string())
                    .chain(r.decisions.iter().map(|d| format!("- {d}")))
                    .collect::<Vec<_>>()
                    .join("\n");
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
        task.attempts[idx].handoff = report
            .as_ref()
            .map(|r| r.handoff.trim().to_string())
            .filter(|h| !h.is_empty());
        // Agent-reported decisions are trusted less than the owner's: strip
        // any leading "Owner:" the agent might have echoed back, prefix
        // with "Agent:", and never duplicate an identical entry.
        if let Some(r) = &report {
            for d in &r.decisions {
                let cleaned = ["Owner:", "Agent:"]
                    .iter()
                    .find_map(|p| d.strip_prefix(p))
                    .map(|s| s.trim_start())
                    .unwrap_or(d.as_str());
                let entry = format!("Agent: {cleaned}");
                if !task.decisions.contains(&entry) {
                    task.decisions.push(entry);
                }
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
            // question can't loop forever even when the classifier keeps
            // saying it's answerable.
            record_failure(&mut task, idx, FailureKind::Blocked, question_text.clone());
            let should_continue = advance_after_failure(&mut task, attempt_budget);
            if !should_continue {
                // Same "attempts keep failing" escalation as
                // `fail_and_continue`'s, inlined because a
                // should_continue==true blocked report falls through to
                // `classify_answerable` below instead of looping.
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
                        just_answered = true;
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

            let answerable_p = classify_answerable(&app, &task, &question_text).await;
            let blocked_decision = decide_blocked_question(answerable_p);
            if let Some(p) = answerable_p {
                task.decisions.push(jev_answerable_line(
                    p,
                    blocked_decision == BlockedDecision::AnswerSelf,
                ));
            }
            match blocked_decision {
                BlockedDecision::AnswerSelf => {
                    let decision =
                        format!("Answer it yourself from the repository: {question_text}");
                    if !task.decisions.contains(&decision) {
                        task.decisions.push(decision);
                    }
                    task.status = TaskStatus::Queued;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                    drop(permit);
                    continue;
                }
                BlockedDecision::Waiting => {
                    task.question = Some(Question {
                        text: question_text,
                        options: vec![],
                    });
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
                            just_answered = true;
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
                LoopSignal::Continue { answered } => {
                    just_answered = answered;
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
            task.status = TaskStatus::Stopped;
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
                Ok(git::Rebase::Moved { new_sha }) => {
                    task.decisions.push(format!(
                        "Rebase: carried the work onto {base_ref} at {}",
                        short_sha(&new_sha)
                    ));
                    task.base_sha = new_sha.clone();
                    base_sha = new_sha;
                    let (wt, b) = (worktree.clone(), base_sha.clone());
                    changed = tokio::task::spawn_blocking(move || {
                        git::changed_files(&wt, &b).unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default();
                    task.attempts[idx].changed_files = changed.clone();
                    if changed.is_empty() {
                        // The base already holds this work: nothing left to commit.
                        task.decisions.push(format!(
                            "Rebase: {base_ref} already contains this work; nothing to commit"
                        ));
                        task.attempts[idx].status = AttemptStatus::Passed;
                        task.attempts[idx].ended_at = Some(now_ms());
                        task.status = TaskStatus::Done;
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
                Ok(git::Rebase::Conflicts { new_sha, files }) => {
                    task.base_sha = new_sha.clone();
                    let detail = conflict_detail(&base_ref, &new_sha, &files, &task_id);
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
                        LoopSignal::Continue { answered } => {
                            just_answered = answered;
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

        let verify_results = run_verify_cached(
            &app,
            &task_id,
            &worktree,
            &run_dir,
            &base_sha,
            &task.verify,
            &cancel,
        )
        .await;
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
                LoopSignal::Continue { answered } => {
                    just_answered = answered;
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
            task.attempts[idx].status = AttemptStatus::Blocked;
            task.attempts[idx].ended_at = Some(now_ms());
            task.question = Some(Question {
                text: format!("Change touches protected path {path}: approve or reject?"),
                options: vec!["approve".into(), "reject".into()],
            });
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
                    just_answered = true;
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
                            LoopSignal::Continue { answered } => {
                                just_answered = just_answered || answered;
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

        let mut review_result: Option<ReviewResult> = None;
        if !settings.review.is_empty() {
            if let Some(review_route) =
                select_review_route(&settings, &route, task.variant().review_other_family)
            {
                if task.variant().review_other_family && review_route.harness == route.harness {
                    // The A/B arm says other family; record when it wasn't.
                    let note = format!(
                        "Orchestrator: no review route on another harness; reviewed by {}",
                        review_route.id
                    );
                    if !task.decisions.contains(&note) {
                        task.decisions.push(note);
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
                let reviewed = run_review(
                    &app,
                    &task_id,
                    attempt_n,
                    &task,
                    &worktree,
                    &base_sha,
                    &verify_results,
                    &implementer_note,
                    review_route,
                    &deny_read,
                    &cancel,
                    &mut review_cost,
                )
                .await;
                // The task's total, and separately the attempt's
                // review_cost_usd for display: an attempt's own cost_usd is
                // what later resumes of its session subtract
                // (`attempt_cost`), so the review's cost must never join it.
                task.cost_usd += review_cost;
                task.attempts[idx].review_cost_usd =
                    Some(task.attempts[idx].review_cost_usd.unwrap_or(0.0) + review_cost);
                match reviewed {
                    Ok(r) => review_result = Some(r),
                    Err(RunError::Cancelled) => {
                        // A cancelled review is never a PASS: the attempt
                        // (and the task) is simply stopped.
                        task.attempts[idx].status = AttemptStatus::Interrupted;
                        task.attempts[idx].ended_at = Some(now_ms());
                        task.status = TaskStatus::Stopped;
                        task.updated_at = now_ms();
                        let _ = app.store.save_task(&task);
                        app.broadcast_task(&task);
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                    Err(RunError::Io(why)) => {
                        // No verdict is never a PASS: the owner decides
                        // whether to commit this attempt unreviewed.
                        task.attempts[idx].status = AttemptStatus::Blocked;
                        task.attempts[idx].ended_at = Some(now_ms());
                        task.question = Some(Question {
                            text: format!("The review gave no verdict ({why}). Commit this attempt unreviewed?"),
                            options: vec!["approve".into(), "retry".into()],
                        });
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
                                if answer == "approve" {
                                    task.decisions.push(format!(
                                        "Orchestrator: attempt {attempt_n} committed without a review verdict"
                                    ));
                                } else {
                                    review_result = Some(ReviewResult {
                                        verdict: Verdict::Fail,
                                        findings: vec![format!(
                                            "Owner asked for another attempt: {answer}"
                                        )],
                                    });
                                }
                            }
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
        task.attempts[idx].review = review_result.clone();

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
                    LoopSignal::Continue { answered } => {
                        just_answered = answered;
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

        if !task.final_verify.is_empty() {
            // Not through the verify cache: it keys on the diff alone and
            // would hand back the fast checks' results.
            let final_results = run_verify_commands(
                &worktree,
                &run_dir.join("final"),
                &task.final_verify,
                settings.sandbox,
                &cancel,
            )
            .await;
            task.attempts[idx]
                .verify
                .extend(final_results.iter().cloned());
            if let Some(failed) = final_results.iter().find(|v| v.code != Some(0)) {
                let detail = format!(
                    "Final check {} exited {}.\n{}",
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
                    LoopSignal::Continue { answered } => {
                        just_answered = answered;
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

        if let Some(parent_id) = task.parent.clone() {
            let landing = land_on_parent(
                &app, &mut task, idx, attempt_n, &worktree, &run_dir, &parent_id, &cancel,
            )
            .await;
            match landing {
                Landing::Landed => {
                    git::delete_wip_ref(&worktree, &task_id);
                    task.attempts[idx].status = AttemptStatus::Passed;
                    task.attempts[idx].ended_at = Some(now_ms());
                    task.status = TaskStatus::Done;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                Landing::Failed { kind, detail } => {
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
                        LoopSignal::Continue { answered } => {
                            just_answered = answered;
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
                Landing::Cancelled => {
                    task.attempts[idx].status = AttemptStatus::Interrupted;
                    task.attempts[idx].ended_at = Some(now_ms());
                    task.status = TaskStatus::Stopped;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                Landing::NoParent => {}
            }
        }

        let wt4 = worktree.clone();
        let title = task.title.clone();
        let tid = task_id.clone();
        let commit_res =
            tokio::task::spawn_blocking(move || git::commit(&wt4, &title, &tid, attempt_n)).await;
        match commit_res {
            Ok(Ok(())) => {
                git::delete_wip_ref(&worktree, &task_id);
                task.attempts[idx].status = AttemptStatus::Passed;
                task.attempts[idx].ended_at = Some(now_ms());
                task.status = TaskStatus::Done;
            }
            Ok(Err(e)) => {
                record_failure(&mut task, idx, FailureKind::Error, e.to_string());
                task.status = TaskStatus::Failed;
            }
            Err(e) => {
                record_failure(
                    &mut task,
                    idx,
                    FailureKind::Error,
                    format!("commit task panicked: {e}"),
                );
                task.status = TaskStatus::Failed;
            }
        }
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);
        drop(permit);
        app.finish_task_loop(&task_id);
        return;
    }
}

pub(super) async fn mark_stopped_if_not_already(app: &Arc<App>, task_id: &str) {
    if let Ok(Some(mut task)) = app.store.load_task(task_id) {
        if !matches!(
            task.status,
            TaskStatus::Stopped | TaskStatus::Done | TaskStatus::Failed
        ) {
            task.status = TaskStatus::Stopped;
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
        let app = self.arc();
        let cancel_for_loop = cancel.clone();
        let pending_for_loop = pending_answer.clone();
        let tid = task_id.clone();
        let handle = tokio::spawn(async move {
            run_task_loop(
                app,
                tid,
                pending_for_loop,
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
                handle,
            },
        );
    }

    fn finish_task_loop(&self, task_id: &str) {
        self.controls.lock().unwrap().remove(task_id);
        if let Ok(Some(task)) = self.store.load_task(task_id) {
            self.advance_graph(&task.repo);
        }
    }
}
