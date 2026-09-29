use super::*;

/// How many times the orchestrator may extend a task's budget on its own
/// before "attempts keep failing" goes to the owner: enough to push past a
/// fixable failure or a wrong review finding, not enough to burn the
/// budget in a loop.
const MAX_ORCHESTRATOR_CONTINUES: usize = 2;

/// The "attempts keep failing" question: the orchestrator triages it first
/// (it sees the last failure, e.g. the review findings, and can say what to
/// fix or that a finding is wrong), up to [`MAX_ORCHESTRATOR_CONTINUES`]
/// times per task; after that, or with auto-answer off, only the owner.
pub(super) async fn wait_for_exhausted_answer(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<String> {
    let continues = task
        .decisions
        .iter()
        .filter(|d| d.starts_with(EXHAUSTED_ANSWER_PREFIX))
        .count();
    if continues < MAX_ORCHESTRATOR_CONTINUES {
        wait_for_answer_with_triage(app, task_id, task, idx, pending_answer, cancel, permit).await
    } else {
        wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await
    }
}

/// Parks on a fresh one-shot channel until either `task.answer` delivers an
/// answer or `cancel` fires. `task` must already have its `question`/
/// `status: Waiting` set by the caller -- it's persisted here, *after* the
/// one-shot is installed, so a `task.answer` that arrives the instant the
/// broadcast reaches a client can never beat the one-shot into existence
/// and get wrongly rejected as "no live parked loop". Persists the
/// decision/status transition either way afterwards, so a caller never has
/// to duplicate that bookkeeping.
pub(super) async fn wait_for_answer(
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
            task.question = None;
            task.status = TaskStatus::Queued;
            task.updated_at = now_ms();
            let _ = app.store.save_task(task);
            app.broadcast_task(task);
        }
        None => {
            if let Ok(Some(mut task)) = app.store.load_task(task_id) {
                if task.status != TaskStatus::Stopped {
                    task.status = TaskStatus::Stopped;
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
    if !settings.auto_answer {
        return None;
    }
    let route = chat::orchestrator_route(&settings)?;

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
        resume: None,
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
        Err(RunError::Io(_)) => Some(TriageRun {
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
    let question = task.question.clone().unwrap_or(Question {
        text: String::new(),
        options: vec![],
    });
    let attempt_n = task.attempts[idx].n;
    let worktree = PathBuf::from(&task.worktree);
    if let Some(TriageRun { decision, cost_usd }) = run_triage(
        app,
        task_id,
        attempt_n,
        task,
        &worktree,
        &question.text,
        &question.options,
        cancel,
    )
    .await
    {
        if let Some(cost) = cost_usd {
            task.cost_usd += cost;
        }
        match decision.action {
            brief::TriageAction::Answer => {
                let line =
                    orchestrator_answer_line(&question.text, &decision.answer, &decision.reason);
                app.broadcast_log(task_id, attempt_n, line.clone());
                task.decisions.push(line);
                task.question = None;
                task.status = TaskStatus::Queued;
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
                app.broadcast_task(task);
                return Some(decision.answer);
            }
            brief::TriageAction::Escalate => {
                task.question = Some(Question {
                    text: decision.question,
                    options: decision.options,
                });
                task.decisions
                    .push(orchestrator_escalate_line(&decision.reason));
            }
        }
    }
    wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await
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
    if let Some(TriageRun { decision, cost_usd }) = run_triage(
        app,
        task_id,
        attempt_n,
        task,
        worktree,
        question_text,
        &options,
        cancel,
    )
    .await
    {
        if let Some(cost) = cost_usd {
            task.cost_usd += cost;
        }
        match decision.action {
            brief::TriageAction::Answer => {
                let line =
                    orchestrator_answer_line(question_text, &decision.answer, &decision.reason);
                app.broadcast_log(task_id, attempt_n, line.clone());
                task.decisions.push(line);
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

pub(super) async fn classify_answerable(
    app: &Arc<App>,
    task: &Task,
    question: &str,
) -> Option<f64> {
    let settings = app.settings.read().unwrap().classifier.clone();
    let key = app.secrets.read().unwrap().classifier_key.clone();
    let base_url = app.secrets.read().unwrap().classifier_base_url.clone();
    let state = json!({"goal": task.goal, "criteria": task.criteria, "question": question});
    let questions = vec![classify::QuestionSpec::Noul {
        name: "answerable".to_string(),
        prompt: "Is this question answerable from the repository and task, without the owner?"
            .to_string(),
    }];
    let start = std::time::Instant::now();
    let s2 = settings.clone();
    let k2 = key.clone();
    let b2 = base_url.clone();
    let q2 = questions.clone();
    let state2 = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        classify::decide(&s2, k2.as_deref(), b2.as_deref(), &state2, &q2)
    })
    .await
    .unwrap_or_else(|e| Err(classify::ClassifyError(e.to_string())));
    app.journal(&task.id, "blocked_question", &result, start.elapsed());
    result
        .ok()
        .and_then(|answers| answers.get("answerable").and_then(|a| a.noul))
}
