use super::*;

/// How many times the orchestrator may extend a task's budget on its own
/// before "attempts keep failing" goes to the owner: enough to push past a
/// fixable failure or a wrong review finding, not enough to burn the
/// budget in a loop.
pub(super) const MAX_ORCHESTRATOR_CONTINUES: usize = 2;

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
                by_owner: !app.take_agent_answer(task_id),
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
        codex_mcp: &[],
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
        RunTrack::No,
        &worktree,
        &req,
        CostTag::task("triage", &route.id),
        &brief_text,
        &events_path,
        cancel,
        Guard::off(),
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
    let answer = park_for_answer(app, task_id, task, pending_answer, cancel, permit).await;
    app.take_agent_answer(task_id);
    answer
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
    wake_orchestrator_for_waiting(app, task);
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
            // The chat or an MCP client answered: an orchestrator line, which
            // also counts toward the continue cap. Peeked, not taken: the
            // caller reads it again to decide `by_owner`.
            let by_agent = app.agent_answers.lock().unwrap().contains(task_id);
            let question = task.question.take();
            record_answer(task, question.as_ref(), answer, by_agent);
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

/// The question text an orchestrator answer line carries. Triage may
/// rewrite an "Attempts keep failing" question when it escalates, so that
/// kind always uses the fixed text the continue cap counts by.
pub(super) fn answer_line_question(q: &Question) -> &str {
    if q.kind == QuestionKind::AttemptsFailing {
        EXHAUSTED_QUESTION
    } else {
        &q.text
    }
}

/// Records a parked question's answer: the owner's, or the orchestrator
/// agent's (an `Orchestrator:` decision line and an orchestrator-answered
/// question, which count toward the triage and continue bounds).
pub(super) fn record_answer(
    task: &mut Task,
    question: Option<&Question>,
    answer: &str,
    by_agent: bool,
) {
    match (question, by_agent) {
        (Some(q), true) => {
            task.decisions.push(orchestrator_answer_line(
                answer_line_question(q),
                answer,
                "answered by the orchestrator agent",
            ));
            record_answered_question(task, q, answer, AnsweredBy::Orchestrator);
        }
        (q, _) => {
            task.decisions.push(format!("Owner: {answer}"));
            if let Some(q) = q {
                record_answered_question(task, q, answer, AnsweredBy::Owner);
            }
        }
    }
}

impl App {
    /// Whether the answer that just arrived for `task_id` came from the chat
    /// or an MCP client rather than the app; clears the mark.
    pub(super) fn take_agent_answer(&self, task_id: &str) -> bool {
        self.agent_answers.lock().unwrap().remove(task_id)
    }
}

/// Wakes the orchestrator chat (an overseer turn) for a task that just parked
/// on a question nobody answered (triage escalated or was off), when
/// `auto_answer` is on. Parking happens once per question, so this never wakes
/// twice for the same one. An owner-only kind never wakes it: the owner
/// answers those, and `task_answer` would refuse the agent anyway.
pub(super) fn wake_orchestrator_for_waiting(app: &App, task: &Task) {
    let Some(question) = task.question.as_ref() else {
        return;
    };
    if !app.settings.read().unwrap().auto_answer || owner_only(question.kind) {
        return;
    }
    let prompt = overseer_prompt("overseer_waiting", task);
    chat::wake_overseer(app, &task.repo, prompt, false);
}

/// Wakes the overseer once, at the moment the task loop moves a top-level
/// task into `done`, when `auto_answer` is on and nothing landed or opened a
/// pull request yet. The task remembers it (`overseer_woken`), so a restart,
/// an unarchive or a rebroadcast never wakes it again. A busy chat drops the
/// wake; the next digest mentions the task.
pub(super) fn wake_orchestrator_for_done(app: &App, task: &mut Task) {
    if task.status != TaskStatus::Done
        || task.parent.is_some()
        || task.archived
        || task.overseer_woken
        || task.pr_url.is_some()
        || task.landed_sha.is_some()
        || !app.settings.read().unwrap().auto_answer
    {
        return;
    }
    let prompt = overseer_prompt("overseer_done", task);
    // Marked either way: this is the one transition, not a retry loop.
    task.overseer_woken = true;
    let _ = app.store.save_task(task);
    chat::wake_overseer(app, &task.repo, prompt, false);
}

/// A server-built overseer prompt (`prompts/orchestrator.yaml`) naming the task.
fn overseer_prompt(key: &str, task: &Task) -> String {
    crate::prompts::get(key)
        .replace("{id}", &task.id)
        .replace("{title}", &task.title)
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
        codex_mcp: &[],
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
        RunTrack::No,
        worktree,
        &req,
        CostTag::task("triage", &route.id),
        &brief_text,
        &events_path,
        cancel,
        Guard::off(),
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
                        answer_line_question(&question),
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
            by_owner: !app.take_agent_answer(task_id),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A waiting task in a real repo directory with the fake harness, and
    /// the number of overseer turns that ran for it.
    async fn waiting_wakes(auto_answer: bool, kind: QuestionKind) -> usize {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = auto_answer;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut task = task_with_status(TaskStatus::Waiting);
        task.repo = repo.to_string_lossy().to_string();
        task.question = Some(Question::new(
            "Which way?",
            vec!["A".into(), "B".into()],
            kind,
            AskedBy::Implement,
        ));
        app.store.save_task(&task).unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        let pending = Arc::new(StdMutex::new(None));
        let id = task.id.clone();
        park_for_answer(&app, &id, &mut task, &pending, &cancel, &mut None).await;
        chat::wait_idle(&app, task.repo.as_str()).await;
        chat::fake_overseer_calls(&repo)
    }

    #[tokio::test]
    async fn a_parked_task_wakes_the_overseer_once_only_with_auto_answer_on() {
        assert_eq!(waiting_wakes(true, QuestionKind::AgentQuestion).await, 1);
        assert_eq!(waiting_wakes(false, QuestionKind::AgentQuestion).await, 0);
        assert_eq!(waiting_wakes(true, QuestionKind::Budget).await, 0);
    }

    #[tokio::test]
    async fn only_the_done_transition_wakes_the_overseer_once() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = true;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut task = task_with_status(TaskStatus::Done);
        task.repo = repo.to_string_lossy().to_string();
        app.store.save_task(&task).unwrap();
        // A rebroadcast of an old done task wakes nothing.
        app.broadcast_task(&task);
        chat::wait_idle(&app, task.repo.as_str()).await;
        assert_eq!(chat::fake_overseer_calls(&repo), 0);
        wake_orchestrator_for_done(&app, &mut task);
        chat::wait_idle(&app, task.repo.as_str()).await;
        assert_eq!(chat::fake_overseer_calls(&repo), 1);
        assert!(
            app.store
                .load_task(&task.id)
                .unwrap()
                .unwrap()
                .overseer_woken
        );
        // Again (a restart, an unarchive) and a subtask: still one.
        task.archived = false;
        wake_orchestrator_for_done(&app, &mut task);
        let mut part = task_with_status(TaskStatus::Done);
        part.repo = task.repo.clone();
        part.parent = Some(task.id.clone());
        wake_orchestrator_for_done(&app, &mut part);
        let mut landed = task_with_status(TaskStatus::Done);
        landed.repo = task.repo.clone();
        landed.landed_sha = Some("abc".into());
        wake_orchestrator_for_done(&app, &mut landed);
        chat::wait_idle(&app, task.repo.as_str()).await;
        assert_eq!(chat::fake_overseer_calls(&repo), 1);
    }

    fn exhausted_task() -> Task {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.question = Some(Question::new(
            "Attempts keep failing with a review failure: continue, change approach, or stop?",
            vec![
                "continue".into(),
                brief::ACCEPT_LAST_ATTEMPT.into(),
                "stop".into(),
            ],
            QuestionKind::AttemptsFailing,
            AskedBy::Implement,
        ));
        task
    }

    #[tokio::test]
    async fn an_agent_sourced_answer_is_an_orchestrator_line_and_never_the_owners() {
        let (app, _dir) = test_app();
        let mut task = exhausted_task();
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        let pending = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
        let parked = {
            let (app, pending, cancel, id) =
                (app.clone(), pending.clone(), cancel.clone(), id.clone());
            tokio::spawn(async move {
                let answered =
                    park_for_answer(&app, &id, &mut task, &pending, &cancel, &mut None).await;
                (answered, task)
            })
        };
        let tx = loop {
            if let Some(tx) = pending.lock().unwrap().take() {
                break tx;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        app.agent_answers.lock().unwrap().insert(id.clone());
        tx.send("continue".into()).unwrap();
        let (answered, task) = parked.await.unwrap();
        assert_eq!(answered.as_deref(), Some("continue"));
        assert!(app.take_agent_answer(&id));
        assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
        assert_eq!(
            task.decisions
                .iter()
                .filter(|d| d.starts_with(EXHAUSTED_ANSWER_PREFIX))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn an_mcp_accept_is_refused_and_mcp_continues_stop_at_the_cap() {
        let (app, _dir) = test_app();
        let mut task = exhausted_task();
        app.store.save_task(&task).unwrap();
        let accept = json!({"id": task.id, "answer": brief::ACCEPT_LAST_ATTEMPT, "source": "mcp"});
        let err = app.handle_task_answer(accept).await.unwrap_err();
        assert!(err.contains("owner's to pick"), "{err}");
        for _ in 0..MAX_ORCHESTRATOR_CONTINUES {
            task.decisions
                .push(format!("{EXHAUSTED_ANSWER_PREFIX} -> continue (r)"));
        }
        app.store.save_task(&task).unwrap();
        let cont = json!({"id": task.id, "answer": "continue", "source": "mcp"});
        let err = app.handle_task_answer(cont).await.unwrap_err();
        assert!(err.contains("owner decides"), "{err}");
    }

    #[tokio::test]
    async fn an_escalated_exhausted_question_still_counts_toward_the_cap() {
        let (app, _dir) = test_app();
        let mut task = exhausted_task();
        // Triage rewrote the text on escalation.
        task.question.as_mut().unwrap().text = "Should I keep trying?".into();
        app.store.save_task(&task).unwrap();
        let q = task.question.clone();
        for _ in 0..MAX_ORCHESTRATOR_CONTINUES {
            record_answer(&mut task, q.as_ref(), "continue", true);
        }
        assert_eq!(
            task.decisions
                .iter()
                .filter(|d| d.starts_with(EXHAUSTED_ANSWER_PREFIX))
                .count(),
            MAX_ORCHESTRATOR_CONTINUES
        );
        app.store.save_task(&task).unwrap();
        let cont = json!({"id": task.id, "answer": "continue", "source": "chat"});
        assert!(app.handle_task_answer(cont).await.is_err());
    }

    #[tokio::test]
    async fn an_agent_stop_is_an_orchestrator_line_and_pr_refuses_a_branch() {
        let (app, _dir) = test_app();
        let task = exhausted_task();
        app.store.save_task(&task).unwrap();
        let stop = json!({"id": task.id, "answer": "stop", "source": "chat"});
        app.handle_task_answer(stop).await.unwrap();
        let task = app.store.load_task(&task.id).unwrap().unwrap();
        assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
        assert!(task
            .decisions
            .iter()
            .any(|d| d.starts_with("Orchestrator:")));
        let err = app
            .handle_task_pr(json!({"id": task.id, "branch": "main", "source": "chat"}))
            .await
            .unwrap_err();
        assert!(err.contains("own branch"), "{err}");
    }
}
