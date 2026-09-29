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
    pub attempt_n: u32,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FailureDecision {
    NextAttempt { tier: Tier },
    Waiting { question: String },
}

/// spec step 8: same signature as previous, or a second loop -> tier up; same signature 3x in
/// a row, or attempts exhausted -> waiting.
pub(super) const EXHAUSTED_QUESTION: &str = "Attempts keep failing";

/// The decision line an orchestrator answer to that question leaves
/// (see `orchestrator_answer_line`).
pub(super) const EXHAUSTED_ANSWER_PREFIX: &str = "Orchestrator: Attempts keep failing";

pub fn decide_after_failure(input: &FailureDecisionInput) -> FailureDecision {
    if input.consecutive_same >= 3 || input.attempt_n >= input.max_attempts {
        return FailureDecision::Waiting {
            question: format!(
                "{EXHAUSTED_QUESTION} with {}: continue, change approach, or stop?",
                input.signature
            ),
        };
    }
    let tier = if input.previous_signature == Some(input.signature) || input.loops >= 2 {
        input.tier.up()
    } else {
        input.tier
    };
    FailureDecision::NextAttempt { tier }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockedDecision {
    AnswerSelf,
    Waiting,
}

/// spec step 8, blocked branch: classifier p >= 0.7 -> agent answers itself.
pub fn decide_blocked_question(answerable_p: Option<f64>) -> BlockedDecision {
    match answerable_p {
        Some(p) if p >= 0.7 => BlockedDecision::AnswerSelf,
        _ => BlockedDecision::Waiting,
    }
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

pub(super) fn advance_after_failure(task: &mut Task, max_attempts: u32) -> bool {
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
        attempt_n: implement_attempt_count(task),
        max_attempts,
    };
    match decide_after_failure(&input) {
        FailureDecision::NextAttempt { tier } => {
            // The fallback note stays until the tier actually changes.
            if tier != task.tier {
                task.tier_fallback = None;
            }
            task.tier = tier;
            task.status = TaskStatus::Queued;
            true
        }
        FailureDecision::Waiting { question } => {
            task.question = Some(Question {
                text: question,
                options: vec!["continue".into(), "change approach".into(), "stop".into()],
            });
            task.status = TaskStatus::Waiting;
            false
        }
    }
}

pub(super) enum LoopSignal {
    /// `answered` is `true` when this iteration ends because the owner just
    /// answered a waiting question -- the caller uses it to decide whether
    /// the *next* attempt is still allowed to resume a previous session
    /// (spec: "never resume after a waiting/answer cycle").
    Continue {
        answered: bool,
    },
    Stop,
}

/// Record a failure, apply the tier/waiting rules, and -- when now waiting
/// -- park for an answer (the orchestrator's, within its bound, else the
/// owner's), extending the attempt budget by 2 on any
/// answer (spec step 9; every waiting state `advance_after_failure` reaches
/// is the "attempts exhausted" kind).
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
    record_failure(task, idx, kind, detail);
    let should_continue = advance_after_failure(task, *attempt_budget);
    if should_continue {
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
        return LoopSignal::Continue { answered: false };
    }
    // Not persisted here: the wait does it itself, after the one-shot is
    // installed (see `wait_for_answer`).
    match wait_for_exhausted_answer(app, task_id, task, idx, pending_answer, cancel, permit).await {
        Some(_answer) => {
            *attempt_budget += 2;
            LoopSignal::Continue { answered: true }
        }
        None => LoopSignal::Stop,
    }
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
    Question {
        text: format!(
            "Criterion {} cannot be met as written: {}. Evidence: {}",
            claim.criterion, task.criteria[claim.criterion], claim.evidence
        ),
        options: IMPOSSIBLE_OPTIONS.iter().map(|o| o.to_string()).collect(),
    }
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
