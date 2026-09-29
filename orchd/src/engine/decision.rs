use super::*;

const ERROR_KEYWORDS: &[&str] = &["error", "fail", "assert", "panic", "exception"];

/// The first line that looks like it's reporting a failure (contains one of
/// `ERROR_KEYWORDS`, case-insensitive); `None` when nothing in `text` does.
fn find_error_line(text: &str) -> Option<&str> {
    text.lines().find(|l| {
        let lower = l.to_ascii_lowercase();
        ERROR_KEYWORDS.iter().any(|k| lower.contains(k))
    })
}

/// Strips digits and absolute-path-looking tokens (so `/tmp/xyz123/a.ts:42`
/// across two runs still normalizes to the same signature) and collapses
/// whitespace.
pub(crate) fn normalize_signature_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' {
            while let Some(&next) = chars.peek() {
                if next.is_whitespace() {
                    break;
                }
                chars.next();
            }
            continue;
        }
        if c.is_ascii_digit() {
            continue;
        }
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// spec step 8 (as sharpened by review): "kind + normalized first
/// error-looking line of the tail (lines containing
/// error|fail|assert|panic|exception, case-insensitive; strip digits and
/// absolute paths), fallback to first tail line."
pub fn failure_signature(kind: FailureKind, detail: &str) -> String {
    let line = find_error_line(detail).unwrap_or_else(|| detail.lines().next().unwrap_or(""));
    let normalized = normalize_signature_line(line.trim());
    format!("{}:{}", kind.as_str(), truncate_chars(&normalized, 120))
}

/// Count trailing attempts (including the most recent) whose failure
/// signature matches, i.e. how many times in a row this exact failure has
/// happened.
pub fn consecutive_same_signature(attempts: &[Attempt], signature: &str) -> u32 {
    let mut count = 0;
    for a in attempts.iter().rev() {
        match &a.failure {
            Some(f) if f.signature == signature => count += 1,
            _ => break,
        }
    }
    count
}

pub struct FailureDecisionInput<'a> {
    pub tier: Tier,
    pub signature: &'a str,
    pub previous_signature: Option<&'a str>,
    pub consecutive_same: u32,
    /// Implement attempts on this task the loop detector stopped, this one
    /// included.
    pub loops: u32,
    /// Implement attempts on this task whose review failed with a finding the
    /// reviewer marked as repeating an earlier one, this one included.
    pub repeated_reviews: u32,
    pub attempt_n: u32,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FailureDecision {
    NextAttempt { tier: Tier },
    Waiting { question: String },
}

/// spec step 8: same signature as previous, a second loop, or a first
/// repeated review finding -> tier up; same signature 3x in a row, a second
/// repeated review finding, or attempts exhausted -> waiting.
pub(super) const EXHAUSTED_QUESTION: &str = "Attempts keep failing";

/// The decision line an orchestrator answer to that question leaves
/// (see `orchestrator_answer_line`).
pub(super) const EXHAUSTED_ANSWER_PREFIX: &str = "Orchestrator: Attempts keep failing";

pub fn decide_after_failure(input: &FailureDecisionInput) -> FailureDecision {
    if input.consecutive_same >= 3
        || input.repeated_reviews >= 2
        || input.attempt_n >= input.max_attempts
    {
        return FailureDecision::Waiting {
            question: format!(
                "{EXHAUSTED_QUESTION} with {}: continue, change approach, or stop?",
                input.signature
            ),
        };
    }
    let tier = if input.previous_signature == Some(input.signature)
        || input.loops >= 2
        || input.repeated_reviews >= 1
    {
        input.tier.up()
    } else {
        input.tier
    };
    FailureDecision::NextAttempt { tier }
}

pub(super) fn record_failure(task: &mut Task, idx: usize, kind: FailureKind, detail: String) {
    let signature = failure_signature(kind, &detail);
    let a = &mut task.attempts[idx];
    a.status = AttemptStatus::Failed;
    a.ended_at = Some(now_ms());
    a.failure = Some(Failure {
        kind,
        detail,
        signature,
    });
}

/// How many *implement* attempts a task has made -- the plan attempt (if
/// any) never counts against `maxAttempts`.
pub(super) fn implement_attempt_count(task: &Task) -> u32 {
    task.attempts
        .iter()
        .filter(|a| a.stage == Stage::Implement)
        .count() as u32
}

/// Whether the latest implement attempt failed on a landing conflict: the
/// next one only resolves it.
pub(super) fn last_failure_is_conflict(task: &Task) -> bool {
    task.attempts
        .iter()
        .rfind(|a| a.stage == Stage::Implement)
        .and_then(|a| a.failure.as_ref())
        .is_some_and(|f| f.kind == FailureKind::Conflict)
}

/// Implement attempts whose review failed with a repeated finding.
pub(super) fn repeated_review_count(task: &Task) -> u32 {
    task.attempts
        .iter()
        .filter(|a| a.stage == Stage::Implement)
        .filter(|a| a.failure.as_ref().map(|f| f.kind) == Some(FailureKind::Review))
        .filter(|a| a.review.as_ref().is_some_and(|r| !r.repeated.is_empty()))
        .count() as u32
}

/// Whether the latest implement attempt failed its review on a finding the
/// reviewer marked as repeating.
pub(super) fn last_review_repeated(task: &Task) -> bool {
    task.attempts
        .iter()
        .rfind(|a| a.stage == Stage::Implement)
        .is_some_and(|a| {
            a.failure.as_ref().map(|f| f.kind) == Some(FailureKind::Review)
                && a.review.as_ref().is_some_and(|r| !r.repeated.is_empty())
        })
}

/// Whether the owner may be offered `brief::ACCEPT_LAST_ATTEMPT`: the last
/// attempt failed only its review or its screenshot evidence, and everything
/// that ran on it exited 0. A change to a protected path is not offered after
/// an evidence failure, because that gate runs before the protected-path
/// approval and accepting must never skip it.
pub(super) fn accept_offered(last: &Attempt, protected_paths: &[String]) -> bool {
    let Some(failure) = &last.failure else {
        return false;
    };
    let clean = last.verify.iter().all(|v| v.code == Some(0));
    match failure.kind {
        FailureKind::Review => clean,
        FailureKind::Evidence => {
            clean
                && !last
                    .changed_files
                    .iter()
                    .any(|f| matches_any_protected(f, protected_paths))
        }
        _ => false,
    }
}

pub(super) fn advance_after_failure(
    task: &mut Task,
    max_attempts: u32,
    protected_paths: &[String],
) -> bool {
    let last = task.attempts.last().expect("failure just recorded");
    let signature = last
        .failure
        .as_ref()
        .map(|f| f.signature.clone())
        .unwrap_or_default();
    let consecutive = consecutive_same_signature(&task.attempts, &signature);
    let previous_signature = if task.attempts.len() >= 2 {
        task.attempts[task.attempts.len() - 2]
            .failure
            .as_ref()
            .map(|f| f.signature.clone())
    } else {
        None
    };
    let input = FailureDecisionInput {
        tier: task.tier,
        signature: &signature,
        previous_signature: previous_signature.as_deref(),
        consecutive_same: consecutive,
        loops: task
            .attempts
            .iter()
            .filter(|a| a.failure.as_ref().map(|f| f.kind) == Some(FailureKind::Loop))
            .count() as u32,
        repeated_reviews: repeated_review_count(task),
        attempt_n: implement_attempt_count(task),
        max_attempts,
    };
    match decide_after_failure(&input) {
        FailureDecision::NextAttempt { tier } => {
            // The fallback note stays until the tier actually changes.
            if tier != task.tier {
                task.tier_fallback = None;
            }
            if last_review_repeated(task) {
                task.decisions.push(format!(
                    "Orchestrator: a review finding repeated -> advisor, tier {}",
                    tier.as_str()
                ));
            }
            task.tier = tier;
            task.status = TaskStatus::Queued;
            true
        }
        FailureDecision::Waiting { question } => {
            let mut options: Vec<String> = vec!["continue".into(), "change approach".into()];
            if accept_offered(&task.attempts[task.attempts.len() - 1], protected_paths) {
                options.push(brief::ACCEPT_LAST_ATTEMPT.into());
            }
            options.push("stop".into());
            task.question = Some(Question::new(
                question,
                options,
                QuestionKind::AttemptsFailing,
                AskedBy::Implement,
            ));
            task.status = TaskStatus::Waiting;
            false
        }
    }
}

pub(super) enum LoopSignal {
    Continue,
    Stop,
}

/// [`LoopSignal`] for the failures the owner may accept as done.
pub(super) enum FailSignal {
    Continue,
    Stop,
    /// The owner accepted the last attempt as done: its final checks and
    /// commit still run (`finish_after_review`).
    Accept,
}

/// The decision line an accepted attempt leaves; the post-restart relaunch
/// finds a pending acceptance by it (`pending_acceptance`).
pub(super) fn accept_line(attempt_n: u32) -> String {
    format!("Owner: accepted attempt {attempt_n} as done; review skipped")
}

pub(super) fn is_accept_option(answer: &str) -> bool {
    answer
        .trim()
        .eq_ignore_ascii_case(brief::ACCEPT_LAST_ATTEMPT)
}

/// Remembers the owner's acceptance as an assumption and a decision line, the
/// line after the `Owner: <answer>` one.
pub(super) fn record_owner_accept(
    task: &mut Task,
    question: &str,
    answer: &str,
    evidence: &str,
    attempt_n: u32,
) {
    task.assumptions.push(Assumption {
        question: question.to_string(),
        answer: answer.to_string(),
        evidence: evidence.to_string(),
        by: "owner".to_string(),
        kind: Some(QuestionKind::AttemptsFailing),
        attempt: Some(attempt_n),
        overturned: false,
        owner_answer: None,
    });
    task.decisions.push(accept_line(attempt_n));
}

/// Whether an owner's answer to `shown` accepts the last attempt, and why:
/// the option itself, or free text the orchestrator route reads as
/// accepting. Only when the option was on the question the owner saw.
async fn owner_accept_reason(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    answered: &Answered,
    cancel: &CancelToken,
) -> Option<&'static str> {
    let shown = answered.shown.as_ref()?;
    if !answered.by_owner || !shown.options.iter().any(|o| is_accept_option(o)) {
        return None;
    }
    if is_accept_option(&answered.text) {
        return Some("owner picked the option");
    }
    if shown
        .options
        .iter()
        .any(|o| o.trim().eq_ignore_ascii_case(answered.text.trim()))
    {
        return None;
    }
    owner_accepts(app, task_id, task, idx, shown, &answered.text, cancel)
        .await
        .then_some("owner free text classified as accept")
}

/// Record a failure, apply the tier/waiting rules, and -- when now waiting
/// -- park for an answer (the orchestrator's, within its bound, else the
/// owner's), extending the attempt budget by 2 on any
/// answer (spec step 9; every waiting state `advance_after_failure` reaches
/// is the "attempts exhausted" kind). Only a review or evidence failure can
/// come back as `LoopSignal::Accept`; use [`fail_or_accept`] there.
#[allow(clippy::too_many_arguments)]
pub(super) async fn fail_and_continue(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    kind: FailureKind,
    detail: String,
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> LoopSignal {
    match fail_or_accept(
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
        FailSignal::Continue => LoopSignal::Continue,
        FailSignal::Stop => LoopSignal::Stop,
        FailSignal::Accept => {
            debug_assert!(false, "only review and evidence failures offer acceptance");
            LoopSignal::Continue
        }
    }
}

/// [`fail_and_continue`] for the failures the owner may accept as done.
/// Accepting never extends `attempt_budget`; the attempt is cleared of its
/// failure and set running again, ready for its final checks.
#[allow(clippy::too_many_arguments)]
pub(super) async fn fail_or_accept(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    kind: FailureKind,
    detail: String,
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> FailSignal {
    record_failure(task, idx, kind, detail);
    let protected = app.settings.read().unwrap().protected_paths.clone();
    let should_continue = advance_after_failure(task, *attempt_budget, &protected);
    if should_continue {
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
        return FailSignal::Continue;
    }
    // Not persisted here: the wait does it itself, after the one-shot is
    // installed (see `wait_for_answer`).
    let Some(answered) =
        wait_for_exhausted_answer(app, task_id, task, idx, pending_answer, cancel, permit).await
    else {
        return FailSignal::Stop;
    };
    if let Some(evidence) = owner_accept_reason(app, task_id, task, idx, &answered, cancel).await {
        let question = answered.shown.map(|q| q.text).unwrap_or_default();
        let attempt_n = task.attempts[idx].n;
        record_owner_accept(task, &question, &answered.text, evidence, attempt_n);
        let a = &mut task.attempts[idx];
        a.status = AttemptStatus::Running;
        a.ended_at = None;
        a.failure = None;
        task.status = TaskStatus::Running;
        task.updated_at = now_ms();
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
        return FailSignal::Accept;
    }
    *attempt_budget += 2;
    FailSignal::Continue
}

/// A task relaunched after the owner accepted its last attempt while no
/// loop was live: that attempt (failed on review or evidence) and its index.
pub(super) fn pending_acceptance(task: &Task) -> Option<usize> {
    let idx = task
        .attempts
        .iter()
        .rposition(|a| a.stage == Stage::Implement)?;
    let a = &task.attempts[idx];
    let failed = a.status == AttemptStatus::Failed
        && matches!(
            a.failure.as_ref().map(|f| f.kind),
            Some(FailureKind::Review | FailureKind::Evidence)
        );
    (failed && task.decisions.last() == Some(&accept_line(a.n))).then_some(idx)
}

/// Owner answer `drop criterion`: removes criterion `n`, its checks and a
/// held-out check on it, and shifts the later checks down by one. Shared by
/// the live loop and the post-restart branch of `task.answer`.
pub(super) fn drop_criterion(task: &mut Task, n: usize) {
    if n >= task.criteria.len() {
        return;
    }
    let text = task.criteria.remove(n);
    task.checks.retain(|c| c.criterion != n);
    if task.held_out.as_ref().is_some_and(|h| h.criterion == n) {
        task.held_out = None;
    }
    for c in task.checks.iter_mut().chain(task.held_out.as_mut()) {
        if c.criterion > n {
            c.criterion -= 1;
        }
    }
    task.decisions.push(format!(
        "Orchestrator: dropped criterion {n} ({text}) as the owner answered"
    ));
}

const IMPOSSIBLE_OPTIONS: [&str; 3] = ["drop criterion", "retry", "stop"];

/// The owner question for a `sushi-impossible` claim.
pub(super) fn impossible_question(task: &Task, claim: &brief::Impossible) -> Question {
    Question::new(
        format!(
            "Criterion {} cannot be met as written: {}. Evidence: {}",
            claim.criterion, task.criteria[claim.criterion], claim.evidence
        ),
        IMPOSSIBLE_OPTIONS.iter().map(|o| o.to_string()).collect(),
        QuestionKind::Impossible,
        AskedBy::Implement,
    )
}

/// The criterion an [`impossible_question`] is about, and whether `answer`
/// is its `drop criterion` option.
pub(super) fn impossible_drop_target(question: &Question, answer: &str) -> Option<usize> {
    if !answer.trim().eq_ignore_ascii_case("drop criterion") {
        return None;
    }
    question
        .text
        .strip_prefix("Criterion ")?
        .split_once(" cannot be met as written")?
        .0
        .parse()
        .ok()
}

#[cfg(test)]
mod grounded_checks_tests {
    use super::*;

    fn check(criterion: usize, run: &str) -> Check {
        Check {
            criterion,
            run: run.into(),
            baseline: Some(Baseline::Fail),
        }
    }

    #[test]
    fn dropping_a_criterion_removes_its_checks_and_shifts_the_later_ones() {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.criteria = vec!["zero".into(), "one".into(), "two".into()];
        task.checks = vec![check(0, "a"), check(1, "b"), check(2, "c"), check(1, "b2")];
        task.held_out = Some(check(2, "h"));
        drop_criterion(&mut task, 1);
        assert_eq!(task.criteria, vec!["zero", "two"]);
        assert_eq!(task.checks, vec![check(0, "a"), check(1, "c")]);
        assert_eq!(task.held_out, Some(check(1, "h")));
        assert_eq!(
            task.decisions,
            vec!["Orchestrator: dropped criterion 1 (one) as the owner answered"]
        );
        // The held-out check goes with its own criterion.
        drop_criterion(&mut task, 1);
        assert_eq!(task.held_out, None);
        assert_eq!(task.checks, vec![check(0, "a")]);
        // Out of range: nothing happens.
        let before = task.decisions.len();
        drop_criterion(&mut task, 9);
        assert_eq!(task.decisions.len(), before);
    }

    #[test]
    fn the_impossible_question_round_trips_to_its_criterion() {
        let mut task = task_with_status(TaskStatus::Running);
        task.criteria = vec!["zero".into(), "one".into()];
        let claim = brief::Impossible {
            criterion: 1,
            evidence: "it contradicts zero".into(),
        };
        let q = impossible_question(&task, &claim);
        assert_eq!(
            q.text,
            "Criterion 1 cannot be met as written: one. Evidence: it contradicts zero"
        );
        assert_eq!(q.options, vec!["drop criterion", "retry", "stop"]);
        assert_eq!(impossible_drop_target(&q, "drop criterion"), Some(1));
        assert_eq!(impossible_drop_target(&q, "retry"), None);
    }
}
