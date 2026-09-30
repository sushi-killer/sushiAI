use super::*;

/// How many times the orchestrator may extend a task's budget on its own
/// before "attempts keep failing" goes to the owner: enough to push past a
/// fixable failure or a wrong review finding, not enough to burn the
/// budget in a loop.
const MAX_ORCHESTRATOR_CONTINUES: usize = 2;

/// The "attempts keep failing" question: the orchestrator triages it first
/// (it sees the last failure, e.g. the review findings, and can say what to
/// fix or that a finding is wrong), up to [`MAX_ORCHESTRATOR_CONTINUES`]
/// times per task; after that, or with auto-answer off, only the owner. The
/// answer policy goes first: it continues once on its own.
pub(super) async fn wait_for_exhausted_answer(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<Answered> {
    let asked = task.question.clone();
    if let Some(text) = try_policy_answer(app, task, cancel).await {
        return Some(Answered {
            text,
            by_owner: false,
            shown: asked,
        });
    }
    let continues = task
        .decisions
        .iter()
        .filter(|d| d.starts_with(EXHAUSTED_ANSWER_PREFIX))
        .count();
    if continues < MAX_ORCHESTRATOR_CONTINUES {
        wait_for_triaged_answer(app, task_id, task, idx, pending_answer, cancel, permit).await
    } else {
        let shown = task.question.clone();
        park_for_answer(app, task_id, task, pending_answer, cancel, permit)
            .await
            .map(|text| Answered {
                text,
                by_owner: true,
                shown,
            })
    }
}

/// An answer to a parked question and where it came from: only the owner's
/// may accept an attempt. `shown` is the question the owner actually saw
/// (triage may have replaced it on escalation), captured before parking
/// clears `task.question`.
pub(super) struct Answered {
    pub text: String,
    pub by_owner: bool,
    pub shown: Option<Question>,
}

/// Whether the owner's free text tells orchd to accept the last attempt: one
/// read-only session on the orchestrator route. No route, a harness error,
/// a cancellation or a reply without a verdict all mean no.
pub(super) async fn owner_accepts(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    question: &Question,
    answer: &str,
    cancel: &CancelToken,
) -> bool {
    let settings = app.settings.read().unwrap().clone();
    let Some(route) = app.chat_route(&settings) else {
        return false;
    };
    let attempt_n = task.attempts[idx].n;
    let worktree = PathBuf::from(&task.worktree);
    let run_dir = app.store.run_dir(task_id, attempt_n).join("accept-check");
    let _ = std::fs::create_dir_all(&run_dir);
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            &route,
            app,
            &key_path,
            &settings,
            &deny_read,
            &settings_path,
        );
    }
    let req = harness::RunRequest {
        harness: route.harness,
        worktree: &worktree,
        model: route.model.as_deref(),
        effort: route.effort.as_deref(),
        max_budget_usd: None,
        review: true,
        mcp_config: Some(&mcp_path),
        settings_path: Some(&settings_path),
        network_allowed: false,
        codex_mcp: None,
        images: &[],
        repo_settings: true,
    };
    let brief_text = brief::build_accept_brief(task, &question.text, &question.options, answer);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let run_result = run_harness(
        app,
        task_id,
        attempt_n,
        false,
        &worktree,
        &req,
        CostTag::task("triage", &route.id),
        &brief_text,
        &events_path,
        cancel,
        None,
        None,
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    match run_result {
        Ok(o) => {
            if let Some(cost) = o.cost_usd {
                task.cost_usd += cost;
            }
            o.error.is_none()
                && o.final_text
                    .as_deref()
                    .and_then(brief::parse_accept)
                    .unwrap_or(false)
        }
        Err(_) => false,
    }
}

/// [`park_for_answer`] after one shot at the answer policy: a rule or the
/// judge may answer the question before it reaches the owner.
pub(super) async fn wait_for_answer(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<String> {
    if let Some(answer) = try_policy_answer(app, task, cancel).await {
        return Some(answer);
    }
    park_for_answer(app, task_id, task, pending_answer, cancel, permit).await
}

/// Parks on a fresh one-shot channel until either `task.answer` delivers an
/// answer or `cancel` fires. `task` must already have its `question`/
/// `status: Waiting` set by the caller -- it's persisted here, *after* the
/// one-shot is installed, so a `task.answer` that arrives the instant the
/// broadcast reaches a client can never beat the one-shot into existence
/// and get wrongly rejected as "no live parked loop". Persists the
/// decision/status transition either way afterwards, so a caller never has
/// to duplicate that bookkeeping.
async fn park_for_answer(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<String> {
    // A task waiting for its owner holds no slot: parallel limits running
    // agents, not open questions.
    permit.take();
    let (tx, rx) = oneshot::channel();
    *pending_answer.lock().unwrap() = Some(tx);
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    let result = tokio::select! {
        _ = cancel.cancelled() => None,
        answer = rx => answer.ok(),
    };
    *pending_answer.lock().unwrap() = None;
    match &result {
        // The caller keeps using (and later saves) its own copy, so the
        // transition is applied to it rather than to a fresh load -- a
        // stale copy used to overwrite it with the question still set.
        Some(answer) => {
            task.decisions.push(format!("Owner: {answer}"));
            if let Some(question) = task.question.take() {
                record_answered_question(task, &question, answer, AnsweredBy::Owner);
            }
            task.status = TaskStatus::Queued;
            task.updated_at = now_ms();
            let _ = app.store.save_task(task);
            app.broadcast_task(task);
        }
        None => {
            if let Ok(Some(mut task)) = app.store.load_task(task_id) {
                let next = app.cancelled_status(&task.status);
                if task.status != next {
                    task.status = next;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                }
            }
        }
    }
    result
}

/// A completed triage session: the decision, plus whatever it cost (the
/// caller adds this to `task.cost_usd` itself, same as an implement/plan
/// attempt would).
struct TriageRun {
    decision: brief::TriageDecision,
    cost_usd: Option<f64>,
}

/// One fresh, read-only session on the `orchestrator` route, asked to
/// answer or escalate a question that would otherwise go straight to the
/// owner. `None` means "don't triage this one" -- the route is off or names
/// no configured route (same semantics as `planner`'s unknown-id case,
/// except a triage failure never fails the task, it just falls through to
/// asking the owner as before). A harness error or an unparseable reply
/// still produces `Some`, escalating with the original question untouched
/// (`sanitize_triage`) -- fail-open to the owner, never loops.
#[allow(clippy::too_many_arguments)]
async fn run_triage(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    task: &Task,
    worktree: &Path,
    question: &str,
    options: &[String],
    force: bool,
    cancel: &CancelToken,
) -> Option<TriageRun> {
    // P1-2: at most one consecutive triage *answer* per task -- if the
    // previous decision recorded is already an orchestrator answer, this
    // question goes straight to the owner. Without this, a recurring
    // blocked question (or a planner that keeps re-asking) could let
    // triage answer every single time with no bound at all.
    // "Attempts keep failing" has its own bound (MAX_ORCHESTRATOR_CONTINUES).
    if !question.starts_with(EXHAUSTED_QUESTION)
        && task
            .decisions
            .last()
            .map(|d| is_orchestrator_answer_decision(d))
            .unwrap_or(false)
    {
        return None;
    }
    let settings = app.settings.read().unwrap().clone();
    // `force`: the answer policy asks triage to confirm a cautious option even
    // with auto-answer off.
    if !settings.auto_answer && !force {
        return None;
    }
    let route = app.chat_route(&settings)?;

    let run_dir = app.store.run_dir(task_id, attempt_n).join("triage");
    let _ = std::fs::create_dir_all(&run_dir);
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            &route,
            app,
            &key_path,
            &settings,
            &deny_read,
            &settings_path,
        );
    }
    let req = harness::RunRequest {
        harness: route.harness,
        worktree,
        model: route.model.as_deref(),
        effort: route.effort.as_deref(),
        max_budget_usd: None,
        review: true,
        mcp_config: Some(&mcp_path),
        settings_path: Some(&settings_path),
        network_allowed: false,
        codex_mcp: None,
        images: &[],
        repo_settings: true,
    };
    let brief_text = brief::build_triage_brief(task, question, options);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let run_result = run_harness(
        app,
        task_id,
        attempt_n,
        false,
        worktree,
        &req,
        CostTag::task("triage", &route.id),
        &brief_text,
        &events_path,
        cancel,
        None,
        None,
    )
    .await;
    let _ = std::fs::remove_file(&key_path);

    match run_result {
        // The task was stopped mid-triage: don't manufacture an escalation
        // for it, just skip triage and let the normal wait pick up the
        // cancellation itself (its own `cancel.cancelled()` branch).
        Err(RunError::Cancelled) => None,
        Ok(o) if o.error.is_none() => {
            let cost_usd = o.cost_usd;
            let text = o.final_text.unwrap_or_default();
            Some(TriageRun {
                decision: brief::sanitize_triage(brief::parse_triage(&text), question, options),
                cost_usd,
            })
        }
        Ok(o) => Some(TriageRun {
            decision: brief::sanitize_triage(None, question, options),
            cost_usd: o.cost_usd,
        }),
        Err(RunError::Io(_) | RunError::NotFound(_)) => Some(TriageRun {
            decision: brief::sanitize_triage(None, question, options),
            cost_usd: None,
        }),
    }
}

/// Wraps [`wait_for_answer`] with one shot at orchestrator triage first --
/// for the agent's own blocked question, and for "attempts keep failing"
/// through [`wait_for_exhausted_answer`], which bounds how often the
/// orchestrator may extend the budget. `run_triage` caps any other question
/// to one *answer* in a row. `task.question` must already be set by the
/// caller, exactly as for a plain `wait_for_answer` -- an escalation only
/// ever sharpens it, it never invents a question from nothing. Never use
/// this for the protected-path approval question: only the owner may
/// approve that.
///
/// P1-1: on `Answer`, mutates and saves the caller's own `task` in place
/// (exactly like `wait_for_answer`'s own post-answer step) instead of
/// reloading from disk -- `task` is this iteration's accumulated attempt
/// state (failure, cost, usage, summary, changed files, decisions), most of
/// which is never persisted anywhere until this call's `save_task`; an
/// independent reload would silently discard all of it and leave the
/// attempt stuck `running`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn wait_for_answer_with_triage(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<String> {
    if let Some(answer) = try_policy_answer(app, task, cancel).await {
        return Some(answer);
    }
    wait_for_triaged_answer(app, task_id, task, idx, pending_answer, cancel, permit)
        .await
        .map(|a| a.text)
}

/// [`wait_for_answer_with_triage`] once the answer policy has had its turn.
#[allow(clippy::too_many_arguments)]
async fn wait_for_triaged_answer(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<Answered> {
    let question = task.question.clone().unwrap_or_else(|| {
        Question::new("", vec![], QuestionKind::AgentQuestion, AskedBy::Implement)
    });
    let attempt_n = task.attempts[idx].n;
    let worktree = PathBuf::from(&task.worktree);
    let cautious = cautious_choice(app, task, &question);
    let auto_answer = app.settings.read().unwrap().auto_answer;
    if let Some(TriageRun { decision, cost_usd }) = run_triage(
        app,
        task_id,
        attempt_n,
        task,
        &worktree,
        &question.text,
        &question.options,
        cautious.is_some(),
        cancel,
    )
    .await
    {
        if let Some(cost) = cost_usd {
            task.cost_usd += cost;
        }
        let agrees = agrees_with(&decision, cautious.as_deref());
        match decision.action {
            brief::TriageAction::Answer if !agrees && !auto_answer => {}
            brief::TriageAction::Answer => {
                if agrees {
                    let evidence = format!(
                        "the cautious option, and triage agrees: {}",
                        decision.reason
                    );
                    record_policy_answer(
                        task,
                        &question,
                        &decision.answer,
                        "policy",
                        &evidence,
                        attempt_n,
                    );
                    app.broadcast_log(
                        task_id,
                        attempt_n,
                        task.decisions.last().cloned().unwrap_or_default(),
                    );
                } else {
                    let line = orchestrator_answer_line(
                        &question.text,
                        &decision.answer,
                        &decision.reason,
                    );
                    app.broadcast_log(task_id, attempt_n, line.clone());
                    task.decisions.push(line);
                    record_answered_question(
                        task,
                        &question,
                        &decision.answer,
                        AnsweredBy::Orchestrator,
                    );
                }
                task.question = None;
                task.status = TaskStatus::Queued;
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
                app.broadcast_task(task);
                return Some(Answered {
                    text: decision.answer,
                    by_owner: false,
                    shown: Some(question),
                });
            }
            brief::TriageAction::Escalate if !auto_answer => {}
            brief::TriageAction::Escalate => {
                task.question = Some(Question {
                    text: decision.question,
                    options: decision.options,
                    kind: question.kind,
                    asked_by: question.asked_by,
                    asked_at: question.asked_at,
                });
                task.decisions
                    .push(orchestrator_escalate_line(&decision.reason));
            }
        }
    }
    let shown = task.question.clone();
    park_for_answer(app, task_id, task, pending_answer, cancel, permit)
        .await
        .map(|text| Answered {
            text,
            by_owner: true,
            shown,
        })
}

/// Wraps [`ask_plan_question`] with the same orchestrator-triage shot, for
/// the planner's own draft questions specifically -- not the "no
/// verification command" synthesized question or the retry-clarification
/// question, which still go straight to the owner unchanged. Same P1-1 fix
/// as [`wait_for_answer_with_triage`]: mutates and saves the caller's own
/// `task` in place rather than an independent reload. With `keep_text` an
/// escalation asks the owner `question_text` and `options` as given instead
/// of triage's sharpened version (a batched question lists several
/// questions, which a single sharpened one would drop).
#[allow(clippy::too_many_arguments)]
pub(super) async fn ask_plan_question_with_triage(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    attempt_n: u32,
    worktree: &Path,
    question_text: &str,
    options: Vec<String>,
    keep_text: bool,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<String> {
    let asked = Question::new(
        question_text,
        options.clone(),
        QuestionKind::PlanQuestion,
        AskedBy::Plan,
    );
    let cautious = cautious_choice(app, task, &asked);
    let auto_answer = app.settings.read().unwrap().auto_answer;
    if let Some(TriageRun { decision, cost_usd }) = run_triage(
        app,
        task_id,
        attempt_n,
        task,
        worktree,
        question_text,
        &options,
        cautious.is_some(),
        cancel,
    )
    .await
    {
        if let Some(cost) = cost_usd {
            task.cost_usd += cost;
        }
        let agrees = agrees_with(&decision, cautious.as_deref());
        match decision.action {
            brief::TriageAction::Answer if !agrees && !auto_answer => {}
            brief::TriageAction::Escalate if !auto_answer => {}
            brief::TriageAction::Answer => {
                if agrees {
                    let evidence = format!(
                        "the cautious option, and triage agrees: {}",
                        decision.reason
                    );
                    record_policy_answer(
                        task,
                        &asked,
                        &decision.answer,
                        "policy",
                        &evidence,
                        attempt_n,
                    );
                    app.broadcast_log(
                        task_id,
                        attempt_n,
                        task.decisions.last().cloned().unwrap_or_default(),
                    );
                } else {
                    let line =
                        orchestrator_answer_line(question_text, &decision.answer, &decision.reason);
                    app.broadcast_log(task_id, attempt_n, line.clone());
                    task.decisions.push(line);
                    record_answered_question(
                        task,
                        &asked,
                        &decision.answer,
                        AnsweredBy::Orchestrator,
                    );
                }
                task.question = None;
                task.status = TaskStatus::Drafting;
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
                app.broadcast_task(task);
                return Some(decision.answer);
            }
            brief::TriageAction::Escalate => {
                task.decisions
                    .push(orchestrator_escalate_line(&decision.reason));
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
                app.broadcast_task(task);
                let (text, options) = if keep_text {
                    (question_text.to_string(), options)
                } else {
                    (decision.question, decision.options)
                };
                return ask_plan_question(
                    app,
                    task_id,
                    &text,
                    options,
                    pending_answer,
                    cancel,
                    permit,
                )
                .await;
            }
        }
    }
    ask_plan_question(
        app,
        task_id,
        question_text,
        options,
        pending_answer,
        cancel,
        permit,
    )
    .await
}
