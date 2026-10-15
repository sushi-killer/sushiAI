use crate::clock::now_ms;
use crate::engine::attempts::pricing::price_claude_resume;
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::state::App;
use crate::harness;
use crate::policy::routing::Harness;
use crate::store::Store;
use crate::task::AskedBy;
use crate::task::Attempt;
use crate::task::AttemptStatus;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::Stage;
use crate::task::Task;
use crate::task::Usage;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

type Prices = std::collections::BTreeMap<String, crate::policy::pricing::Price>;

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
        Stage::Implement if attempt.cost_usd.is_none() => {
            vec![
                run_dir.join("resume-rejected.events.jsonl"),
                run_dir.join("events.jsonl"),
            ]
        }
        Stage::Review if attempt.cost_usd.is_none() => {
            vec![run_dir.join("events.jsonl")]
        }
        Stage::Implement | Stage::Review => vec![],
    }
}

/// For an attempt that ended before all its spend was recorded (stopped, or
/// the daemon died under it): adds whatever its runs left in their events
/// -- see [`replay_run_cost`] -- to the attempt and the task. That covers
/// the attempt's review helper runs too, kept in
/// `review_cost_usd`. Claude only, for the attempt's own runs; the review
/// may run on either harness. Rejected-resume archives retain their own metadata.
pub(super) fn settle_unfinished_cost(task: &mut Task, idx: usize, store: &Store, prices: &Prices) {
    let run_dir = store.run_dir(&task.id, task.attempts[idx].n);
    let attempt = &task.attempts[idx];
    if attempt.harness == Harness::Claude || run_dir.join("resume-rejected.events.jsonl").exists() {
        let mut total = None;
        let mut estimated = false;
        let mut usage = Usage {
            input: 0,
            output: 0,
            cached: 0,
        };
        for path in uncounted_event_files(&run_dir, attempt) {
            let Some(outcome) = replay_attempt_run(task, attempt, &path, store, prices) else {
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
            crate::task::costs::add_attempt_spend(task, idx, cost, estimated, usage);
        }
    }
    settle_interrupted_review(task, idx, store, prices);
}

pub(super) fn rejected_resume_cost(
    task: &Task,
    idx: usize,
    store: &Store,
    prices: &Prices,
) -> Option<harness::RunOutcome> {
    let attempt = &task.attempts[idx];
    let path = store
        .run_dir(&task.id, attempt.n)
        .join("resume-rejected.events.jsonl");
    replay_attempt_run(task, attempt, &path, store, prices)
}

fn replay_attempt_run(
    task: &Task,
    attempt: &Attempt,
    path: &Path,
    store: &Store,
    prices: &Prices,
) -> Option<harness::RunOutcome> {
    let mut outcome = if attempt.harness == Harness::Claude {
        replay_run_cost(path, prices)?
    } else {
        let text = std::fs::read_to_string(path).ok()?;
        let mut outcome = harness::replay_events(attempt.harness, &text);
        let price = prices.get(&attempt.model)?;
        outcome.cost_usd = Some(price.codex_cost(
            outcome.usage_input,
            outcome.usage_cached,
            outcome.usage_output,
        ));
        outcome
    };
    if attempt.stage == Stage::Implement && attempt.harness == Harness::Claude {
        let metadata = crate::harness::run_files::RunFiles::of(path).meta;
        let argv: Vec<String> = std::fs::read_to_string(metadata)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|meta| serde_json::from_value(meta["argv"].clone()).ok())
            .unwrap_or_default();
        if let Some(resume) = argv.windows(2).find(|w| w[0] == "--resume") {
            price_claude_resume(
                &crate::engine::attempts::pricing::session_attempts(task, store, prices),
                attempt.n,
                &resume[1],
                prices,
                &mut outcome,
            );
        }
    }
    Some(outcome)
}

/// A review's cost is saved right after it ends. Replay the attempt's helper
/// runs when the daemon died before any review cost was saved.
pub(super) fn settle_interrupted_review(
    task: &mut Task,
    idx: usize,
    store: &Store,
    prices: &Prices,
) {
    let attempt = &task.attempts[idx];
    if attempt.stage == Stage::Implement && attempt.review_cost_usd.is_none() {
        let cost = store
            .helper_run_dirs(&task.id, "review", Some(attempt.n))
            .iter()
            .filter_map(|dir| replay_run_cost(&dir.join("events.jsonl"), prices))
            .map(|o| o.cost_usd.unwrap_or(0.0))
            .reduce(|a, b| a + b);
        if let Some(cost) = cost {
            crate::task::costs::settle_helper(
                task,
                idx,
                crate::task::costs::HelperStage::Review,
                cost,
            );
        }
    }
}

/// On daemon start, for a task that was between attempts: an advisor run
/// about its last failed attempt that the daemon died under. Its cost was
/// never saved (`advisor_cost_usd` is saved the moment one ends), so it is
/// replayed from its advisor helper folders. Returns whether it added
/// anything.
pub(super) fn settle_interrupted_advisor(task: &mut Task, store: &Store, prices: &Prices) -> bool {
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
    let cost = store
        .helper_run_dirs(&task.id, "advisor", Some(attempt.n))
        .iter()
        .filter_map(|dir| replay_run_cost(&dir.join("events.jsonl"), prices))
        .map(|o| o.cost_usd.unwrap_or(0.0))
        .reduce(|a, b| a + b);
    let Some(cost) = cost else {
        return false;
    };
    crate::task::costs::settle_helper(task, idx, crate::task::costs::HelperStage::Advisor, cost);
    true
}

/// Starts the text of the budget question.
const BUDGET_QUESTION_PREFIX: &str = "This task has spent";

/// Whether `answer` raises the budget; `task.answer` applies a "raise" that
/// arrives with no live loop to deliver it to.
pub(super) fn is_budget_raise(question: &Question, answer: &str) -> bool {
    question.kind == QuestionKind::Budget && answer.trim().eq_ignore_ascii_case("raise")
}

/// The option of the daily budget question that lets the task go on.
const DAILY_RUN_ANYWAY: &str = "run anyway";

pub(super) fn is_daily_budget_run_anyway(question: &Question, answer: &str) -> bool {
    question.kind == QuestionKind::DailyBudget
        && answer.trim().eq_ignore_ascii_case(DAILY_RUN_ANYWAY)
}

/// The daily budget question to ask before `run`, when `dailyBudgetUsd` is
/// on, today's spend (every record of the UTC day) has reached it and the
/// owner has not already said "run anyway" today. Plan and implement runs
/// only: an attempt's review and advisor are never held.
async fn daily_budget_question(app: &Arc<App>, task: &mut Task, run: &str) -> Option<Question> {
    let budget = app.settings.read().unwrap().daily_budget_usd;
    if !matches!(run, "plan" | "implement")
        || budget.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
    {
        return None;
    }
    let data = app.data_dir.clone();
    let now = now_ms();
    let records = tokio::task::spawn_blocking(move || crate::store::costs::read_all(&data))
        .await
        .ok()?;
    let (day, spent) = crate::costs::spend_today(&records, now);
    if spent < budget || task.daily_budget_ok_day.as_deref() == Some(day.as_str()) {
        return None;
    }
    task.record_decision(format!(
        "Orchestrator: daily budget ${budget:.2} reached (${spent:.2} spent on {day} UTC); the {run} run waits for the owner"
    ));
    Some(Question::new(
        format!(
            "Today's spend is ${spent:.2}, reaching the ${budget:.2} daily budget (UTC {day}); the {run} run was not started. Run anyway for the rest of {day}, or stop?"
        ),
        vec![DAILY_RUN_ANYWAY.into(), "stop".into()],
        QuestionKind::DailyBudget,
        asked_by_run(run), crate::clock::now_ms()))
}

/// The stage a budget gate's `run` argument names.
fn asked_by_run(run: &str) -> AskedBy {
    match run {
        "plan" => AskedBy::Plan,
        "review" => AskedBy::Review,
        _ => AskedBy::Implement,
    }
}

/// The gate before every stage run (plan, advisor, implement attempt,
/// review): while the task's spend meets its `variant.max_cost_usd` budget,
/// it waits for the owner -- "raise" adds the budget once more, "stop"
/// stops it -- instead of running. Never interrupts a run already going.
/// Restores the entering phase and slot after an answer; changes return to
/// lifecycle admission and cancellation remains a separate outcome.
pub(super) async fn wait_while_over_budget(
    app: &Arc<App>,
    task: &mut Task,
    run: &str,
    slot: &mut QuestionSlot<'_>,
) -> Result<(), WaitEnd> {
    let resume_status = task.status;
    let mut waited = false;
    loop {
        let question = if let Some(budget) = task.cost_budget().filter(|b| task.cost_usd >= *b) {
            let step = task.variant().max_cost_usd;
            task.record_decision(format!(
                "Orchestrator: budget ${budget:.2} reached (${:.2} spent); the {run} run waits for the owner",
                task.cost_usd
            ));
            Question::new(
                format!(
                    "{BUDGET_QUESTION_PREFIX} ${:.2}, reaching its ${budget:.2} budget, before the {run} run. Raise the budget by ${step:.2}, or stop?",
                    task.cost_usd
                ),
                vec!["raise".into(), "stop".into()],
                QuestionKind::Budget,
                asked_by_run(run), crate::clock::now_ms())
        } else if let Some(q) = daily_budget_question(app, task, run).await {
            q
        } else {
            break;
        };
        task.ask(question.clone(), now_ms());
        waited = true;
        let answer = slot.answer(app, task).await?;
        if is_budget_raise(&question, &answer) {
            task.raise_budget();
        }
        if is_daily_budget_run_anyway(&question, &answer) {
            task.allow_daily_budget(crate::policy::calendar::day(now_ms()));
        }
    }
    if !waited {
        return Ok(());
    }
    if slot.resume(app, task, resume_status).await {
        Ok(())
    } else {
        Err(WaitEnd::Stopped)
    }
}
