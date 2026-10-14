use super::*;

/// An answer on its way to a parked loop. `default_evidence` is set when the
/// waiting SLA gave it (`wait_sla.rs`): the loop records a policy assumption
/// with that evidence instead of an owner or orchestrator line.
#[derive(Debug)]
pub(super) struct Reply {
    pub text: String,
    pub default_evidence: Option<String>,
    /// The overseer answered like an earlier answer (`task.answer {like}`).
    pub like: Option<String>,
}

#[cfg(test)]
impl From<&str> for Reply {
    fn from(text: &str) -> Self {
        Reply {
            text: text.to_string(),
            default_evidence: None,
            like: None,
        }
    }
}

/// How many times non-owner sessions may extend a task's budget on their own
/// before "attempts keep failing" goes to the owner: enough to push past a
/// fixable failure or a wrong review finding, not enough to burn the
/// budget in a loop.
pub(super) const MAX_ORCHESTRATOR_CONTINUES: usize = 2;

/// The "attempts keep failing" question: the overseer decides it (it sees the
/// last failure, e.g. the review findings, and can say what to fix or that a
/// finding is wrong), up to [`MAX_ORCHESTRATOR_CONTINUES`] times per task;
/// after that, or with auto-answer off, the failure ladder takes one changed
/// approach and then fails the task (`ladder.rs`): the owner is never asked by
/// the ladder. The answer policy goes first: it continues once on its own.
pub(super) async fn wait_for_exhausted_answer(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> Option<Answered> {
    if task.queue.park.is_some() || task.status == TaskStatus::Failed {
        task.updated_at = now_ms();
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
        app.advance_queue(&task.repo);
        return None;
    }
    if let Some(text) = try_policy_answer(app, task, cancel).await {
        return Some(Answered { text });
    }
    let auto_answer = app.settings.read().unwrap().auto_answer;
    if (task.loop_state.non_owner_continues as usize) < MAX_ORCHESTRATOR_CONTINUES && auto_answer {
        match park(app, task_id, task, pending_answer, cancel, permit, true).await {
            Parked::Answer(text) => {
                let text = text?;
                app.take_agent_answer(task_id);
                return Some(Answered { text });
            }
            // No overseer turn started: the ladder decides, not a wait with
            // nobody to answer.
            Parked::NoOverseer => {}
        }
    }
    // The next rung is never an open question: one changed approach, then
    // the task fails with a report (`ladder.rs`).
    task.question = None;
    if ladder::approach_changed(task) {
        ladder::end_ladder(app, task);
        return None;
    }
    task.status = TaskStatus::Queued;
    Some(Answered {
        text: "change approach".to_string(),
    })
}

/// What an answer to "attempts keep failing" does besides extending the
/// budget: `change approach` changes how the next attempt works.
pub(super) fn settle_exhausted_answer(app: &App, task: &mut Task, idx: usize, answer: &str) {
    if is_option(answer, "change approach") {
        let settings = app.settings.read().unwrap().clone();
        let attempt_n = task.attempts[idx].n;
        ladder::change_approach(task, &settings, attempt_n);
        task.queue.rate_pauses = 0;
        // The loop reloads the task from disk before its next attempt.
        task.updated_at = now_ms();
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
}

/// An answer to a question that parked a loop.
pub(super) struct Answered {
    pub text: String,
}

/// [`park_for_answer`] after one shot at the answer policy: a rule or the
/// judge may answer the question before it reaches the owner.
pub(super) async fn wait_for_answer(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
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
pub(super) async fn park_for_answer(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> Option<String> {
    match park(app, task_id, task, pending_answer, cancel, permit, false).await {
        Parked::Answer(answer) => answer,
        Parked::NoOverseer => None,
    }
}

/// How a park ended.
enum Parked {
    Answer(Option<String>),
    /// `need_overseer` was set and the wake neither started a turn nor held
    /// the question: nobody will answer it. The question is still open.
    NoOverseer,
}

async fn park(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
    need_overseer: bool,
) -> Parked {
    if cancel.is_cancelled() {
        app.apply_cancellation(task);
        task.updated_at = now_ms();
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
        app.advance_queue(&task.repo);
        return Parked::Answer(None);
    }
    // A task waiting for its owner holds no slot: parallel limits running
    // agents, not open questions.
    permit.take();
    let (tx, rx) = oneshot::channel();
    *pending_answer.lock().unwrap() = Some(tx);
    app.save_task_logged(task);
    // The wake may hold the question for the overseer; the broadcast that
    // shows the waiting task carries that, so the app never sees it unheld first.
    wake_orchestrator_for_waiting(app, task);
    if let (Some(q), Ok(Some(saved))) = (task.question.as_mut(), app.store.load_task(task_id)) {
        if let Some(s) = saved.question {
            (q.held_for_overseer, q.held_at) = (s.held_for_overseer, s.held_at);
        }
    }
    if need_overseer && !task.question.as_ref().is_some_and(|q| q.held_for_overseer) {
        *pending_answer.lock().unwrap() = None;
        return Parked::NoOverseer;
    }
    app.broadcast_task(task);
    let reply = tokio::select! {
        _ = cancel.cancelled() => None,
        answer = rx => answer.ok(),
    };
    *pending_answer.lock().unwrap() = None;
    let evidence = reply.as_ref().and_then(|r| r.default_evidence.clone());
    let like = reply.as_ref().and_then(|r| r.like.clone());
    let result = reply.map(|r| r.text);
    match &result {
        // Keep the caller's copy in sync with either transition so its
        // next save cannot restore the open question.
        Some(answer) => {
            // Peeked: the caller reads it again to decide `by_owner`.
            let caller = app
                .agent_answers
                .lock()
                .unwrap()
                .get(task_id)
                .copied()
                .unwrap_or(AnsweredBy::Owner);
            let question = task.question.take();
            record_answer_for(
                task,
                question.as_ref(),
                answer,
                caller,
                evidence.as_deref(),
                like.as_deref(),
            );
            task.status = TaskStatus::Queued;
            task.updated_at = now_ms();
            app.save_task_logged(task);
            app.broadcast_task(task);
        }
        None => {
            if let Ok(Some(saved)) = app.store.load_task(task_id) {
                *task = saved;
                // The failure ladder ended this task: it stays failed.
                if task.status != TaskStatus::Failed {
                    app.apply_cancellation(task);
                }
                if task.status == TaskStatus::Stopped {
                    app.close_stopped_task(task);
                }
                task.updated_at = now_ms();
                app.save_task_logged(task);
                app.broadcast_task(task);
                app.advance_queue(&task.repo);
            }
        }
    }
    Parked::Answer(result)
}

/// The question text an orchestrator answer line carries. An "Attempts keep
/// failing" question always uses the fixed text the continue cap counts by,
/// not its own text, which names the failure.
pub(super) fn answer_line_question(q: &Question) -> &str {
    if q.kind == QuestionKind::AttemptsFailing {
        EXHAUSTED_QUESTION
    } else {
        &q.text
    }
}

/// Why `answer` to the waiting `question` of `task` is refused for a caller
/// that is not the owner: the gate that refuses it, with the reason the caller
/// is told. Owner-only kinds keep only refusals, a shared check is not dropped
/// by an agent, and the overseer and the answer policy stay under
/// the hard limits (`decision_memory.rs`); a lead or chat caller relays what the owner said,
/// so the hard limits do not bind it.
pub(super) fn non_owner_refusal(
    settings: &Settings,
    task: &Task,
    question: &Question,
    answer: &str,
    caller: AnsweredBy,
) -> Option<String> {
    if owner_only(question.kind) && !is_refusal(question, answer) && !is_recheck(question, answer) {
        return Some(format!(
            "this {:?} question is the owner's to allow: an agent can only stop or deny it",
            question.kind
        ));
    }
    if let Some(why) = shared_check_drop_refusal(settings, task, question, answer) {
        return Some(why);
    }
    if question.kind == QuestionKind::PreexistingFailure
        && !question.commands.is_empty()
        && is_option(answer, PRE_EXISTING_RETRY)
        && question
            .commands
            .iter()
            .all(|command| non_owner_base_retry(task, command, Some(&task.base_sha)))
    {
        return Some(
            "these checks already got a non-owner retry on this base: the owner decides now"
                .to_string(),
        );
    }
    if caller == AnsweredBy::Overseer && question.kind == QuestionKind::AttemptsFailing {
        let continues = answer.trim().to_ascii_lowercase().starts_with("continue");
        if !continues && !is_option(answer, "change approach") {
            return Some(
                "the overseer may only continue or change approach on attempts keep failing: the failure ladder ends the task otherwise"
                    .to_string(),
            );
        }
    }
    if matches!(caller, AnsweredBy::Overseer | AnsweredBy::Policy) && !owner_only(question.kind) {
        if let Some(limit) =
            hard_limit(question, answer).filter(|l| *l != HardLimit::SharedCheckDrop)
        {
            return Some(format!(
                "this question is under a hard limit ({limit:?}): the owner decides, not an agent"
            ));
        }
    }
    None
}

/// The options of the waiting `question` an overseer may answer with: the
/// ones [`non_owner_refusal`] lets through. Free text meets the same gates
/// when it is sent.
fn overseer_allowed_answers(settings: &Settings, task: &Task, question: &Question) -> Vec<String> {
    question
        .options
        .iter()
        .filter(|option| {
            non_owner_refusal(settings, task, question, option, AnsweredBy::Overseer).is_none()
        })
        .cloned()
        .collect()
}

/// The waiting question as an overseer turn gets it: its typed fields and the
/// answers its gates allow (`allowedAnswers`), after the wake prompt.
pub(super) fn waiting_prompt(app: &App, key: &str, task: &Task) -> String {
    let prompt = overseer_prompt(key, task);
    let Some(question) = task.question.as_ref() else {
        return prompt;
    };
    let allowed = overseer_allowed_answers(&app.settings.read().unwrap(), task, question);
    let typed = json!({
        "kind": question.kind,
        "text": question.text,
        "options": question.options,
        "allowedAnswers": allowed,
    });
    format!("{prompt}\n\nThe waiting question: {typed}")
}

/// Records a parked question's answer and its caller.
#[cfg(test)]
pub(super) fn record_answer(
    task: &mut Task,
    question: Option<&Question>,
    answer: &str,
    caller: AnsweredBy,
) {
    record_answer_like(task, question, answer, caller, None);
}

/// [`record_answer`], where an overseer answer may name what it was given like.
fn record_answer_like(
    task: &mut Task,
    question: Option<&Question>,
    answer: &str,
    caller: AnsweredBy,
    like: Option<&str>,
) {
    if question.is_some_and(|q| q.kind == QuestionKind::AttemptsFailing) {
        task.queue.rate_pauses = 0;
    }
    let text = question.map(answer_line_question).unwrap_or_default();
    let line = match caller {
        AnsweredBy::Owner => format!("Owner: {answer}"),
        AnsweredBy::Lead => format!("Lead: {answer}"),
        AnsweredBy::Overseer => match like {
            Some(like) => overseer_like_line(like, text, answer),
            None => orchestrator_answer_line(text, answer, "answered by the overseer"),
        },
        AnsweredBy::Orchestrator => {
            orchestrator_answer_line(text, answer, "answered by the orchestrator chat")
        }
        AnsweredBy::Policy => policy_answer_line(text, answer, "policy", ""),
        AnsweredBy::Judge => policy_answer_line(text, answer, "judge", ""),
    };
    task.decisions.push(line);
    if let Some(q) = question {
        record_answered_question(task, q, answer, caller);
    }
}

/// [`record_answer`], or a policy assumption when the waiting SLA gave this
/// answer (`wait_sla.rs`): a default the owner can overturn.
pub(super) fn record_answer_for(
    task: &mut Task,
    question: Option<&Question>,
    answer: &str,
    caller: AnsweredBy,
    default_evidence: Option<&str>,
    like: Option<&str>,
) {
    match (question, default_evidence) {
        (Some(q), Some(evidence)) => {
            let attempt_n = task.attempts.last().map(|a| a.n).unwrap_or(0);
            record_policy_answer(task, q, answer, "policy", evidence, attempt_n);
        }
        _ => record_answer_like(task, question, answer, caller, like),
    }
}

impl App {
    /// Takes the non-owner caller of the answer that just arrived.
    pub(super) fn take_agent_answer(&self, task_id: &str) -> Option<AnsweredBy> {
        self.agent_answers.lock().unwrap().remove(task_id)
    }
}

impl App {
    /// Holds the waiting questions an overseer turn of `reason` that has just
    /// started is about to look at: the one named by `task_id` for a
    /// `waiting` or `reminder` wake, every non-owner-only one in `repo` for a
    /// `restart` wake. Owner-only kinds are never held. Returns what it held
    /// as (task id, hold time), so only that hold is released at the turn's
    /// end. Saved and broadcast.
    pub(crate) fn hold_for_overseer(
        &self,
        repo: &str,
        reason: &str,
        task_id: Option<&str>,
    ) -> Vec<(String, i64)> {
        let tasks: Vec<Task> = match (reason, task_id) {
            ("waiting" | "reminder", Some(id)) => self
                .store
                .load_task(id)
                .ok()
                .flatten()
                .into_iter()
                .collect(),
            ("restart", _) => self
                .store
                .list_tasks()
                .unwrap_or_default()
                .into_iter()
                .filter(|t| t.repo == repo)
                .collect(),
            _ => return Vec::new(),
        };
        let at = now_ms();
        let mut held = Vec::new();
        for mut task in tasks {
            if task.status != TaskStatus::Waiting || task.archived {
                continue;
            }
            let Some(q) = task.question.as_mut().filter(|q| !owner_only(q.kind)) else {
                continue;
            };
            q.held_for_overseer = true;
            q.held_at = Some(at);
            held.push((task.id.clone(), at));
            self.save_task_logged(&task);
            self.broadcast_task(&task);
        }
        held
    }

    /// Ends the holds a finished turn made, each only if it is still the same
    /// hold (a newer turn or question replaced it otherwise) and no queued
    /// wake for that task is about to hold it again.
    pub(crate) fn release_holds(&self, repo: &str, held: &[(String, i64)]) {
        for (id, at) in held {
            if chat::wake_queued_for(self, repo, id) {
                continue;
            }
            let Ok(Some(mut task)) = self.store.load_task(id) else {
                continue;
            };
            if task
                .question
                .as_ref()
                .is_some_and(|q| q.held_at == Some(*at))
            {
                if task
                    .question
                    .as_ref()
                    .is_some_and(|q| q.kind == QuestionKind::AttemptsFailing)
                {
                    if self.step_unanswered_exhausted(&mut task) {
                        self.advance_queue(&task.repo);
                    }
                    continue;
                }
                if let Some(q) = task.question.as_mut() {
                    q.release();
                }
                self.save_task_logged(&task);
                self.broadcast_task(&task);
            }
        }
    }

    /// An overseer turn ended on "attempts keep failing" and did not answer
    /// it: the owner is not asked. The failure ladder takes its next rung, as
    /// when no overseer turn starts: one changed approach (the parked loop
    /// gets `change approach` as the answer; with no loop it is applied here
    /// and the task queued), then the end (failed, with its report and one
    /// notice) with the parked loop cancelled.
    /// Returns whether it queued the task for a new loop.
    pub(super) fn step_unanswered_exhausted(&self, task: &mut Task) -> bool {
        if !ladder::approach_changed(task) {
            let attempt_n = task
                .attempts
                .iter()
                .rfind(|a| a.stage == Stage::Implement)
                .map(|a| a.n);
            let live = self.controls.lock().unwrap().get(&task.id).map(|c| {
                c.pending_answer.lock().unwrap().take().is_some_and(|tx| {
                    tx.send(Reply {
                        text: "change approach".to_string(),
                        default_evidence: None,
                        like: None,
                    })
                    .is_ok()
                })
            });
            match (live, attempt_n) {
                // The parked loop settles the answer and saves.
                (Some(true), _) => return false,
                // A loop between two waits: its own copy decides next.
                (Some(false), _) => {
                    if let Some(q) = task.question.as_mut() {
                        q.release();
                    }
                    self.save_task_logged(task);
                    self.broadcast_task(task);
                    return false;
                }
                (None, Some(n)) => {
                    let settings = self.settings.read().unwrap().clone();
                    ladder::change_approach(task, &settings, n);
                    task.question = None;
                    task.status = TaskStatus::Queued;
                    task.updated_at = now_ms();
                    self.save_task_logged(task);
                    self.broadcast_task(task);
                    return true;
                }
                (None, None) => {}
            }
        }
        ladder::end_ladder(self, task);
        self.notify_task_failed(task);
        if let Some(control) = self.controls.lock().unwrap().get(&task.id) {
            control.cancel.cancel();
        }
        false
    }

    /// Releases every hold that `keep` does not keep.
    fn release_holds_where(&self, keep: impl Fn(&Question) -> bool) {
        for mut task in self.store.list_tasks().unwrap_or_default() {
            let Some(q) = task
                .question
                .as_mut()
                .filter(|q| q.held_for_overseer && !keep(q))
            else {
                continue;
            };
            // An "attempts keep failing" question is never left open: with
            // the hold gone and no queued wake to answer it, the ladder takes
            // its next rung. A queued wake keeps the question for its turn.
            if q.kind == QuestionKind::AttemptsFailing
                && !chat::wake_queued_for(self, &task.repo, &task.id)
            {
                if self.step_unanswered_exhausted(&mut task) {
                    self.advance_queue(&task.repo);
                }
                continue;
            }
            q.release();
            self.save_task_logged(&task);
            self.broadcast_task(&task);
        }
    }

    /// A daemon that stopped mid-turn left its holds behind: no turn runs now.
    /// An "attempts keep failing" question stays open: the restart wake asks
    /// the overseer again, and `retire_ladder_questions` steps the ladder when
    /// nobody will.
    pub(crate) fn release_stale_holds(&self) {
        for mut task in self.store.list_tasks().unwrap_or_default() {
            if let Some(q) = task.question.as_mut().filter(|q| q.held_for_overseer) {
                q.release();
                self.save_task_logged(&task);
                self.broadcast_task(&task);
            }
        }
    }

    /// Holds older than [`HOLD_TTL_MS`] end, whatever the turn is doing.
    pub(crate) fn release_expired_holds(&self, now: i64) {
        self.release_holds_where(|q| q.held_now(now));
    }
}

/// Wakes the orchestrator chat (an overseer turn) for a task that just parked
/// on a question nobody answered by rule or precedent, when
/// `auto_answer` is on. Parking happens once per question, so this never wakes
/// twice for the same one. An owner-only kind never wakes it: the owner
/// answers those, and `task_answer` would refuse the agent anyway.
pub(super) fn wake_orchestrator_for_waiting(app: &App, task: &Task) {
    let Some(question) = task.question.as_ref() else {
        return;
    };
    if owner_only(question.kind) {
        // Nobody wakes for it; quiet hours would leave the owner unaware.
        app.record_quiet_parked(task);
        return;
    }
    if !app.settings.read().unwrap().auto_answer {
        return;
    }
    if answer_like_precedent(app, task, question) {
        return;
    }
    wake_for_waiting(app, task);
}

/// Wakes the overseer for every question that parked while auto-answer was
/// off: their own park-time wake found it off and never fires again.
pub(super) fn wake_orchestrator_for_parked(app: &App) {
    for task in app.store.list_tasks().unwrap_or_default() {
        let parked = task.status == TaskStatus::Waiting
            && !task.archived
            && task
                .question
                .as_ref()
                .is_some_and(|q| !q.held_now(now_ms()));
        if parked {
            wake_orchestrator_for_waiting(app, &task);
        }
    }
}

fn wake_for_waiting(app: &App, task: &Task) {
    let prompt = app.with_precedents(waiting_prompt(app, "overseer_waiting", task), &[task]);
    chat::wake_overseer(app, &task.repo, prompt, false, "waiting", Some(&task.id));
}

/// Holds the question and answers it as the overseer, like the owner did
/// before (`decision_memory.rs`), with no overseer turn. Every guard of
/// `task.answer` still applies; one that refuses ends the hold and the
/// overseer is woken as usual. Whether it took the question.
fn answer_like_precedent(app: &App, task: &Task, question: &Question) -> bool {
    let Some((answer, like)) = app.precedent_answer(task, question) else {
        return false;
    };
    if tokio::runtime::Handle::try_current().is_err() || app.shutting_down.load(Ordering::SeqCst) {
        return false;
    }
    let held = app.hold_for_overseer(&task.repo, "waiting", Some(&task.id));
    if held.is_empty() {
        return false;
    }
    let app = app.arc();
    let (repo, id) = (task.repo.clone(), task.id.clone());
    tokio::spawn(async move {
        let params = json!({"id": id, "answer": answer, "source": "overseer", "like": like});
        if app.handle_task_answer(params).await.is_ok() {
            return;
        }
        app.release_holds(&repo, &held);
        if let Ok(Some(task)) = app.store.load_task(&id) {
            if task.status == TaskStatus::Waiting && task.question.is_some() {
                wake_for_waiting(&app, &task);
            }
        }
    });
    true
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
    app.save_task_logged(task);
    chat::wake_overseer(app, &task.repo, prompt, false, "done", Some(&task.id));
}

/// A server-built overseer prompt (`prompts/orchestrator.yaml`) naming the task.
fn overseer_prompt(key: &str, task: &Task) -> String {
    crate::prompts::get(key)
        .replace("{id}", &task.id)
        .replace("{title}", &task.title)
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
        let pending = Arc::new(StdMutex::new(None));
        let id = task.id.clone();
        let parked = {
            let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
            tokio::spawn(async move {
                park_for_answer(&app, &id, &mut task, &pending, &cancel, &mut None).await;
            })
        };
        while pending.lock().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        cancel.cancel();
        parked.await.unwrap();
        chat::wait_idle(&app, repo.to_str().unwrap()).await;
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
            vec!["continue".into(), "stop".into()],
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
        app.agent_answers
            .lock()
            .unwrap()
            .insert(id.clone(), AnsweredBy::Orchestrator);
        tx.send("continue".into()).unwrap();
        let (answered, task) = parked.await.unwrap();
        assert_eq!(answered.as_deref(), Some("continue"));
        assert_eq!(app.take_agent_answer(&id), Some(AnsweredBy::Orchestrator));
        assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
        assert_eq!(
            task.decisions
                .iter()
                .filter(|d| d.starts_with(EXHAUSTED_ANSWER_PREFIX))
                .count(),
            1
        );
    }

    #[test]
    fn the_overseer_is_only_offered_answers_the_gates_let_it_give() {
        let settings = Settings::default();
        let mut task = task_with_status(TaskStatus::Waiting);
        let permission = Question::new(
            "Allow the write?",
            vec![
                permissions::ALLOW_ONCE.into(),
                permissions::ALLOW_ALWAYS.into(),
                permissions::DENY.into(),
            ],
            QuestionKind::Permission,
            AskedBy::Implement,
        );
        task.question = Some(permission.clone());
        assert_eq!(
            overseer_allowed_answers(&settings, &task, &permission),
            vec![permissions::DENY.to_string()]
        );
        let deploy = Question::new(
            "Deploy to production now?",
            vec!["yes".into(), "no".into()],
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
        );
        assert!(overseer_allowed_answers(&settings, &task, &deploy).is_empty());
        let mut tagged = Question::new(
            "Which approach?",
            vec!["A".into(), "B".into()],
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
        );
        tagged.risk = vec![Risk::Security];
        assert!(overseer_allowed_answers(&settings, &task, &tagged).is_empty());
        let plain = Question::new(
            "Which way?",
            vec!["A".into(), "B".into()],
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
        );
        assert_eq!(
            overseer_allowed_answers(&settings, &task, &plain),
            vec!["A".to_string(), "B".to_string()]
        );
    }

    #[tokio::test]
    async fn the_overseer_wake_carries_the_typed_question_and_its_allowed_answers() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = true;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let mut task = task_with_status(TaskStatus::Waiting);
        task.repo = repo.to_string_lossy().to_string();
        task.question = Some(Question::new(
            "Which way?",
            vec!["A".into(), "B".into()],
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
        ));
        app.store.save_task(&task).unwrap();
        let cancel = CancelToken::new();
        let pending = Arc::new(StdMutex::new(None));
        let id = task.id.clone();
        let parked = {
            let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
            tokio::spawn(async move {
                park_for_answer(&app, &id, &mut task, &pending, &cancel, &mut None).await;
            })
        };
        while pending.lock().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        cancel.cancel();
        parked.await.unwrap();
        chat::wait_idle(&app, repo.to_str().unwrap()).await;
        let calls = std::fs::read_to_string(repo.join(".overseer-calls")).unwrap();
        assert!(
            calls.contains(r#""allowedAnswers":["A","B"]"#),
            "the overseer must be handed its allowed answers: {calls}"
        );
        assert!(calls.contains(r#""kind":"agent_question""#), "{calls}");
    }

    #[tokio::test]
    async fn an_exhausted_failure_goes_to_the_overseer_with_no_extra_turn() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = true;
        let repo = dir.path().join("repo");
        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.to_string_lossy().to_string();
        task.worktree = worktree.to_string_lossy().to_string();
        task.attempts
            .push(super::super::test_support::attempt_with_failure(
                1, "verify",
            ));
        let id = task.id.clone();
        let mut budget = 1;
        let pending = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
        let waiter = {
            let (app, pending, cancel) = (app.clone(), pending.clone(), cancel.clone());
            tokio::spawn(async move {
                let signal = fail_and_continue(
                    &app,
                    &id,
                    &mut task,
                    0,
                    FailureKind::Verify,
                    "npm test failed".into(),
                    &mut budget,
                    &pending,
                    &cancel,
                    &mut None,
                )
                .await;
                (matches!(signal, LoopSignal::Stop), task)
            })
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            while pending.lock().unwrap().is_none() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the failure never parked for the overseer");
        // Parked: the only agent run was the overseer's, in the repository.
        assert!(!worktree.join(".overseer-calls").exists());
        // Woken once for the question; the turn is left to end on its own.
        for _ in 0..400 {
            if chat::fake_overseer_calls(&repo) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(chat::fake_overseer_calls(&repo), 1);
        cancel.cancel();
        let (stopped, _) = waiter.await.unwrap();
        assert!(stopped);
    }

    #[test]
    fn the_overseer_may_only_continue_or_change_approach_on_attempts_failing() {
        let settings = Settings::default();
        let task = task_with_status(TaskStatus::Waiting);
        let question = Question::new(
            "Attempts keep failing with a verify failure: continue, change approach, or stop?",
            vec!["continue".into(), "change approach".into(), "stop".into()],
            QuestionKind::AttemptsFailing,
            AskedBy::Implement,
        );
        assert_eq!(
            overseer_allowed_answers(&settings, &task, &question),
            vec!["continue".to_string(), "change approach".to_string()]
        );
        for answer in ["ask the owner", "stop"] {
            assert!(
                non_owner_refusal(&settings, &task, &question, answer, AnsweredBy::Overseer)
                    .is_some(),
                "{answer}"
            );
        }
        assert!(non_owner_refusal(
            &settings,
            &task,
            &question,
            "continue - create fixed.txt",
            AnsweredBy::Overseer
        )
        .is_none());
    }

    #[tokio::test]
    async fn an_exhausted_question_the_overseer_leaves_open_ends_the_ladder() {
        let _harness = chat::fake_overseer_harness();
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = true;
        let repo = dir.path().join("repo");
        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.repo = repo.to_string_lossy().to_string();
        task.worktree = worktree.to_string_lossy().to_string();
        task.attempts
            .push(super::super::test_support::attempt_with_failure(
                1, "verify",
            ));
        task.loop_state.approach_changed_after = Some(1);
        let id = task.id.clone();
        let mut budget = 1;
        let pending = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
        app.controls.lock().unwrap().insert(
            id.clone(),
            TaskControl {
                cancel: cancel.clone(),
                pending_answer: pending.clone(),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle: tokio::spawn(async {}),
            },
        );
        let waiter = {
            let (app, pending, cancel, id) =
                (app.clone(), pending.clone(), cancel.clone(), id.clone());
            tokio::spawn(async move {
                let signal = fail_and_continue(
                    &app,
                    &id,
                    &mut task,
                    0,
                    FailureKind::Verify,
                    "npm test failed".into(),
                    &mut budget,
                    &pending,
                    &cancel,
                    &mut None,
                )
                .await;
                (matches!(signal, LoopSignal::Stop), task)
            })
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            while pending.lock().unwrap().is_none() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the failure never parked for the overseer");
        // The overseer's turn ends without an answer.
        chat::wait_idle(&app, repo.to_str().unwrap()).await;
        let (stopped, _) = tokio::time::timeout(Duration::from_secs(10), waiter)
            .await
            .expect("the ladder never ended the parked loop")
            .unwrap();
        assert!(stopped);
        let task = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert!(task.question.is_none());
        assert!(task
            .decisions
            .iter()
            .any(|d| d.starts_with("Ladder: stopped")));
    }

    #[tokio::test]
    async fn an_exhausted_failure_never_waits_when_no_overseer_turn_starts() {
        let (app, dir) = test_app();
        app.settings.write().unwrap().auto_answer = true;
        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        // The repository is not a directory: the wake does not start.
        task.repo = dir.path().join("missing").to_string_lossy().to_string();
        task.worktree = worktree.to_string_lossy().to_string();
        task.attempts
            .push(super::super::test_support::attempt_with_failure(
                1, "verify",
            ));
        let id = task.id.clone();
        app.store.save_task(&task).unwrap();
        let mut budget = 1;
        let pending = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
        let (app2, pending2, cancel2) = (app.clone(), pending.clone(), cancel.clone());
        let waiter = tokio::spawn(async move {
            fail_and_continue(
                &app2,
                &id,
                &mut task,
                0,
                FailureKind::Verify,
                "npm test failed".into(),
                &mut budget,
                &pending2,
                &cancel2,
                &mut None,
            )
            .await;
            task
        });
        let task = tokio::time::timeout(Duration::from_secs(10), waiter)
            .await
            .expect("the failure waited for an overseer that never started")
            .unwrap();
        assert_ne!(task.status, TaskStatus::Waiting);
        assert!(pending.lock().unwrap().is_none());
    }

    fn held_exhausted(app: &App, approach_changed: bool) -> Task {
        let mut task = exhausted_task();
        task.attempts = vec![super::super::test_support::attempt_with_failure(
            1, "verify",
        )];
        if approach_changed {
            task.loop_state.approach_changed_after = Some(1);
        }
        if let Some(q) = task.question.as_mut() {
            q.held_for_overseer = true;
            q.held_at = Some(1_000);
        }
        app.store.save_task(&task).unwrap();
        task
    }

    #[tokio::test]
    async fn a_hold_that_expires_on_an_exhausted_question_takes_the_next_ladder_rung() {
        let (app, _dir) = test_app();
        let task = held_exhausted(&app, true);
        app.release_expired_holds(1_000 + HOLD_TTL_MS + 1);
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.status, TaskStatus::Failed);
        assert!(saved.question.is_none());

        // Before the changed approach, the rung is the change, not the end.
        let (app, _dir) = test_app();
        let task = held_exhausted(&app, false);
        app.release_expired_holds(1_000 + HOLD_TTL_MS + 1);
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.status, TaskStatus::Queued);
        assert!(saved.question.is_none());
        assert!(ladder::approach_changed(&saved));
    }

    #[tokio::test]
    async fn a_hold_that_expires_while_an_overseer_wake_is_queued_keeps_the_question() {
        let (app, _dir) = test_app();
        let task = held_exhausted(&app, true);
        app.deferred_wakes.lock().unwrap().insert(
            task.repo.clone(),
            vec![chat::test_deferred_wake("waiting", &task.id)],
        );
        app.release_expired_holds(1_000 + HOLD_TTL_MS + 1);
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.status, TaskStatus::Waiting);
        let q = saved.question.expect("the question stays open");
        assert!(!q.held_for_overseer);
    }

    #[tokio::test]
    async fn a_restart_keeps_a_held_exhausted_question_for_the_restart_wake() {
        let (app, _dir) = test_app();
        let task = held_exhausted(&app, false);
        app.release_stale_holds();
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.status, TaskStatus::Waiting);
        assert!(!saved.question.unwrap().held_for_overseer);
        // With nobody to ask again, the start takes the changed approach.
        app.retire_ladder_questions();
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.status, TaskStatus::Queued);
        assert!(ladder::approach_changed(&saved));
    }

    #[test]
    fn the_answer_policy_stays_under_the_hard_limits_like_the_overseer() {
        let settings = Settings::default();
        let task = task_with_status(TaskStatus::Waiting);
        let mut question = Question::new(
            "How should the old data go?",
            vec!["delete the old data".into(), "keep the old data".into()],
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
        );
        question.risk = vec![Risk::Destructive];
        for caller in [AnsweredBy::Policy, AnsweredBy::Overseer] {
            assert!(
                non_owner_refusal(&settings, &task, &question, "keep the old data", caller)
                    .is_some(),
                "{caller:?}"
            );
        }
        assert!(non_owner_refusal(
            &settings,
            &task,
            &question,
            "keep the old data",
            AnsweredBy::Lead
        )
        .is_none());
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

#[cfg(test)]
mod answer_caller_tests {
    use super::*;

    async fn wait_until_parked(events: &mut broadcast::Receiver<Event>, id: &str) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Event::Task { task } = events.recv().await.unwrap() {
                    if task.id == id && task.status == TaskStatus::Waiting {
                        break;
                    }
                }
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn the_parked_loop_records_each_real_caller() {
        for (source, expected) in [
            (None, AnsweredBy::Owner),
            (Some("lead"), AnsweredBy::Lead),
            (Some("mcp"), AnsweredBy::Lead),
            (Some("overseer"), AnsweredBy::Overseer),
            (Some("chat"), AnsweredBy::Orchestrator),
            (Some("policy"), AnsweredBy::Policy),
        ] {
            let (app, _dir) = test_app();
            app.settings.write().unwrap().answer_policy = false;
            let mut task = task_with_status(TaskStatus::Waiting);
            task.question = Some(Question::new(
                "Which way?",
                vec!["B".into()],
                QuestionKind::AgentQuestion,
                AskedBy::Implement,
            ));
            let id = task.id.clone();
            let pending = Arc::new(StdMutex::new(None));
            let cancel = CancelToken::new();
            app.controls.lock().unwrap().insert(
                id.clone(),
                TaskControl {
                    cancel: cancel.clone(),
                    pending_answer: pending.clone(),
                    pending_amend: Arc::new(StdMutex::new(None)),
                    handle: tokio::spawn(async {}),
                },
            );
            let mut events = app.subscribe();
            let loop_app = app.clone();
            let waiter = tokio::spawn(async move {
                let id = task.id.clone();
                let reply =
                    wait_for_answer(&loop_app, &id, &mut task, &pending, &cancel, &mut None).await;
                assert_eq!(reply.as_deref(), Some("B"));
                task
            });
            wait_until_parked(&mut events, &id).await;
            let mut params = json!({"id": id, "answer": "B"});
            if let Some(source) = source {
                params["source"] = json!(source);
            }
            if expected == AnsweredBy::Policy {
                params["evidence"] = json!("SLA elapsed");
            }
            app.handle_task_answer(params).await.unwrap();
            let task = waiter.await.unwrap();
            assert_eq!(task.question_history.len(), 1);
            assert_eq!(task.question_history[0].answered_by, expected);
            if expected == AnsweredBy::Lead {
                assert!(task.decisions.contains(&"Lead: B".to_string()));
            }
            if expected != AnsweredBy::Owner {
                assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
            }
            assert!(app.agent_answers.lock().unwrap().is_empty());
            app.shutdown();
        }
    }

    #[tokio::test]
    async fn a_parked_plan_answer_records_the_lead_and_planner_context_keeps_it() {
        let (app, _dir) = test_app();
        let task = task_with_status(TaskStatus::Drafting);
        let id = task.id.clone();
        app.store.save_task(&task).unwrap();
        let pending = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
        app.controls.lock().unwrap().insert(
            id.clone(),
            TaskControl {
                cancel: cancel.clone(),
                pending_answer: pending.clone(),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle: tokio::spawn(async {}),
            },
        );
        let mut events = app.subscribe();
        let loop_app = app.clone();
        let loop_id = id.clone();
        let waiter = tokio::spawn(async move {
            ask_plan_question(
                &loop_app,
                &loop_id,
                "Which plan?",
                vec!["B".into()],
                (Vec::new(), None),
                &pending,
                &cancel,
                &mut None,
            )
            .await
        });
        wait_until_parked(&mut events, &id).await;
        app.handle_task_answer(json!({"id": id, "answer": "B", "source": "lead"}))
            .await
            .unwrap();
        assert_eq!(waiter.await.unwrap().as_deref(), Some("B"));
        let task = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!(task.question_history[0].answered_by, AnsweredBy::Lead);
        assert!(task.decisions.contains(&"Lead: B".to_string()));
        assert!(subtask_request(&task, "Build it").contains("Lead: B"));
        assert_eq!(app.take_agent_answer(&id), Some(AnsweredBy::Lead));
        app.shutdown();
    }
}

#[cfg(test)]
mod done_prompt_tests {
    use super::*;

    #[test]
    fn the_overseer_reports_a_done_task_and_never_opens_its_pull_request() {
        let task = super::super::test_support::task_with_status(TaskStatus::Done);
        let prompt = overseer_prompt("overseer_done", &task);
        assert!(!prompt.contains("task_pr"), "{prompt}");
    }
}
