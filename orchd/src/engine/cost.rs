use super::*;

type Prices = std::collections::BTreeMap<String, crate::model::Price>;

/// What a Claude run spent, replayed from its saved `events.jsonl`: the
/// CLI's own figure when its `result` made it to disk, else the priced
/// per-message usage, marked estimated. `None` for a missing file or one
/// with nothing priced -- which is every Codex run, whose events have no
/// Claude-shaped message and report usage only at the end of a turn.
pub(super) fn replay_run_cost(events: &Path, prices: &Prices) -> Option<harness::RunOutcome> {
    let text = std::fs::read_to_string(events).ok()?;
    let mut outcome = harness::replay_events(Harness::Claude, &text);
    outcome.estimate_cost(prices);
    outcome.cost_usd.is_some().then_some(outcome)
}

/// The `events.jsonl` files of an attempt's own harness runs whose cost is
/// not in `cost_usd` yet. A plan attempt saves its first run's cost before
/// the retry run starts, so a recorded cost means only the retry is left.
fn uncounted_event_files(run_dir: &Path, attempt: &Attempt) -> Vec<PathBuf> {
    match attempt.stage {
        Stage::Plan => {
            let plan = run_dir.join("plan");
            let retry = plan.join("events-retry.jsonl");
            if attempt.cost_usd.is_some() {
                vec![retry]
            } else {
                vec![plan.join("events.jsonl"), retry]
            }
        }
        Stage::Implement | Stage::Review if attempt.cost_usd.is_none() => {
            vec![run_dir.join("events.jsonl")]
        }
        Stage::Implement | Stage::Review => vec![],
    }
}

/// For an attempt that ended before all its spend was recorded (stopped, or
/// the daemon died under it): adds whatever its runs left in their events
/// -- see [`replay_run_cost`] -- to the attempt and the task. That covers
/// the attempt's review run too (`runs/<n>/review`), kept in
/// `review_cost_usd`. Claude only, for the attempt's own runs; the review
/// may run on either harness.
pub(super) fn settle_unfinished_cost(task: &mut Task, idx: usize, run_dir: &Path, prices: &Prices) {
    let attempt = &task.attempts[idx];
    if attempt.harness == Harness::Claude {
        let mut total = None;
        let mut estimated = false;
        let mut usage = Usage {
            input: 0,
            output: 0,
            cached: 0,
        };
        for path in uncounted_event_files(run_dir, attempt) {
            let Some(outcome) = replay_run_cost(&path, prices) else {
                continue;
            };
            let cost = outcome.cost_usd.unwrap_or(0.0);
            *total.get_or_insert(0.0) += cost;
            estimated |= outcome.cost_estimated;
            usage.input += outcome.usage_input;
            usage.output += outcome.usage_output;
            usage.cached += outcome.usage_cached;
        }
        if let Some(cost) = total {
            let attempt = &mut task.attempts[idx];
            attempt.cost_usd = Some(attempt.cost_usd.unwrap_or(0.0) + cost);
            attempt.cost_estimated |= estimated;
            let saved = attempt.usage.get_or_insert(Usage {
                input: 0,
                output: 0,
                cached: 0,
            });
            saved.input += usage.input;
            saved.output += usage.output;
            saved.cached += usage.cached;
            task.cost_usd += cost;
        }
    }
    // An attempt reviews once, and its review_cost_usd is saved right after.
    let attempt = &task.attempts[idx];
    if attempt.stage == Stage::Implement && attempt.review_cost_usd.is_none() {
        if let Some(o) = replay_run_cost(&run_dir.join("review").join("events.jsonl"), prices) {
            let cost = o.cost_usd.unwrap_or(0.0);
            task.attempts[idx].review_cost_usd = Some(cost);
            task.cost_usd += cost;
        }
    }
}

/// On daemon start, for a task that was between attempts: an advisor run
/// about its last failed attempt that the daemon died under. Its cost was
/// never saved (`advisor_cost_usd` is saved the moment one ends), so it is
/// replayed from `runs/<n>/advisor/events.jsonl`. Returns whether it added
/// anything.
pub(super) fn settle_interrupted_advisor(
    task: &mut Task,
    run_dir_of: impl Fn(u32) -> PathBuf,
    prices: &Prices,
) -> bool {
    let Some(idx) = task
        .attempts
        .iter()
        .rposition(|a| a.stage == Stage::Implement)
    else {
        return false;
    };
    let attempt = &task.attempts[idx];
    if attempt.status != AttemptStatus::Failed || attempt.advisor_cost_usd.is_some() {
        return false;
    }
    let events = run_dir_of(attempt.n).join("advisor").join("events.jsonl");
    let Some(o) = replay_run_cost(&events, prices) else {
        return false;
    };
    let cost = o.cost_usd.unwrap_or(0.0);
    task.attempts[idx].advisor_cost_usd = Some(cost);
    task.cost_usd += cost;
    true
}

/// Starts every question the budget gate asks; `task.answer` uses it to
/// apply a "raise" that arrives with no live loop to deliver it to.
const BUDGET_QUESTION_PREFIX: &str = "This task has spent";

pub(super) fn is_budget_raise(question: &Question, answer: &str) -> bool {
    question.text.starts_with(BUDGET_QUESTION_PREFIX) && answer.trim().eq_ignore_ascii_case("raise")
}

/// The gate before every stage run (plan, advisor, implement attempt,
/// review): while the task's spend meets its `variant.max_cost_usd` budget,
/// it waits for the owner -- "raise" adds the budget once more, "stop"
/// stops it -- instead of running. Never interrupts a run already going.
/// On `true` the task is back in `resume_status` holding a slot; on `false`
/// it is stopped (the wait already persisted that).
#[allow(clippy::too_many_arguments)]
pub(super) async fn wait_while_over_budget(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    run: &str,
    resume_status: TaskStatus,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> bool {
    let mut waited = false;
    while let Some(budget) = task.cost_budget().filter(|b| task.cost_usd >= *b) {
        let step = task.variant().max_cost_usd;
        task.decisions.push(format!(
            "Orchestrator: budget ${budget:.2} reached (${:.2} spent); the {run} run waits for the owner",
            task.cost_usd
        ));
        let question = Question {
            text: format!(
                "{BUDGET_QUESTION_PREFIX} ${:.2}, reaching its ${budget:.2} budget, before the {run} run. Raise the budget by ${step:.2}, or stop?",
                task.cost_usd
            ),
            options: vec!["raise".into(), "stop".into()],
            kind: QuestionKind::Budget,
        };
        task.question = Some(question.clone());
        task.status = TaskStatus::Waiting;
        task.updated_at = now_ms();
        waited = true;
        match wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await {
            None => return false,
            Some(answer) => {
                if is_budget_raise(&question, &answer) {
                    task.budget_raises += 1;
                }
            }
        }
    }
    if !waited {
        return true;
    }
    task.status = resume_status;
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    if permit.is_none() {
        let acquired = tokio::select! {
            _ = cancel.cancelled() => None,
            p = app.slots.clone().acquire_owned() => p.ok(),
        };
        match acquired {
            Some(p) => *permit = Some(p),
            None => {
                mark_stopped_if_not_already(app, task_id).await;
                return false;
            }
        }
    }
    true
}
