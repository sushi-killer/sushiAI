use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::policy::keys::normalize_signature_line;
#[cfg(test)]
use crate::task::checks::results::criteria_results;
#[cfg(test)]
use crate::task::Baseline;
#[cfg(test)]
use crate::task::Check;
#[cfg(test)]
use crate::task::CriterionRuling;
#[cfg(test)]
use crate::task::ReviewResult;
#[cfg(test)]
use crate::task::Verdict;

use crate::clock::now_ms;
use crate::engine::answer_policy::authority::check_authority;
use crate::engine::questions::exhausted::settle_exhausted_answer;
use crate::engine::questions::exhausted::wait_for_exhausted_answer;
use crate::engine::rate_limit::counted_attempts;
use crate::engine::state::App;
#[cfg(test)]
use crate::engine::test_support::attempt_with_failure;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
use crate::policy::routing::Tier;
use crate::policy::settings::Settings;
use crate::prompts::clip::truncate_chars;
use crate::task::checks::authority::Origin;
use crate::task::AskedBy;
use crate::task::Attempt;
use crate::task::AttemptStatus;
#[cfg(test)]
use crate::task::CriterionStatus;
use crate::task::Failure;
use crate::task::FailureKind;
use crate::task::FindingStatus;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::Stage;
use crate::task::Task;
use crate::task::TaskStatus;
use std::sync::Arc;

const ERROR_KEYWORDS: &[&str] = &["error", "fail", "assert", "panic", "exception"];

/// Boilerplate a tool prints around a failure without saying what failed.
fn is_boilerplate(lower: &str) -> bool {
    lower.starts_with("to rerun pass")
        || lower.starts_with("error: test failed")
        || lower.starts_with("error: could not compile")
        || lower.starts_with("error: process didn't exit successfully")
        || lower.starts_with("for more information about this error")
        || lower.starts_with("---")
            && (lower.contains("(tail)") || lower.contains("stderr") || lower.contains("stdout"))
            && !lower.ends_with(" stdout ----")
}

/// A line that reports success or a clean summary even though it contains a
/// failure word: a passing test named `... Error ...`, `0 errors`.
fn is_passing(lower: &str) -> bool {
    lower.starts_with('\u{2714}')
        || lower.starts_with('\u{2713}')
        || lower.starts_with("ok ")
        || lower.starts_with("test result: ok")
        || lower.ends_with("... ok")
        || lower.contains("(0 errors")
        || lower.contains(" 0 errors")
        || lower.contains(" 0 failed")
        || lower.contains("(0 fail")
}

/// A line naming the one thing that failed: a cargo test, a node test, a TAP
/// line or the prettier verdict.
fn is_named_failure(line: &str) -> bool {
    let l = line.trim();
    let lower = l.to_ascii_lowercase();
    (l.starts_with("test ") && l.contains(" ... ") && l.ends_with("FAILED"))
        || (l.starts_with('\u{2716}')
            && !lower.contains(" problem")
            && !lower.contains("failing tests"))
        || lower.starts_with("not ok ")
        || lower.contains("code style issues found")
}

/// The line that names the failure. Named failures win over keyword lines,
/// so a tool's stderr boilerplate or a lint summary printed first does not
/// hide the failing test; boilerplate and passing lines never count.
fn find_error_line(text: &str) -> Option<&str> {
    let usable = |l: &&str| {
        let lower = l.trim().to_ascii_lowercase();
        !lower.is_empty() && !is_boilerplate(&lower) && !is_passing(&lower)
    };
    text.lines()
        .filter(usable)
        .find(|l| is_named_failure(l))
        .or_else(|| {
            text.lines().filter(usable).find(|l| {
                let t = l.trim();
                t.starts_with("---- ") && t.ends_with(" stdout ----")
            })
        })
        .or_else(|| {
            text.lines().filter(usable).find(|l| {
                let lower = l.to_ascii_lowercase();
                ERROR_KEYWORDS.iter().any(|k| lower.contains(k))
            })
        })
}

/// spec step 8 (as sharpened by review): "kind + normalized first
/// error-looking line of the tail (lines containing
/// error|fail|assert|panic|exception, case-insensitive; strip digits and
/// absolute paths), fallback to first tail line."
pub fn failure_signature(kind: FailureKind, detail: &str) -> String {
    let line = find_error_line(detail)
        .unwrap_or_else(|| detail.lines().find(|l| !l.trim().is_empty()).unwrap_or(""));
    let normalized = normalize_signature_line(line.trim());
    format!("{}:{}", kind.as_str(), truncate_chars(&normalized, 120))
}

/// Count trailing attempts (including the most recent) whose failure
/// signature matches, i.e. how many times in a row this exact failure has
/// happened.
/// An attempt for which `skip(index)` holds is passed over: it neither counts
/// nor ends the run (an environment failure says nothing about the approach).
pub fn consecutive_same_signature(
    attempts: &[Attempt],
    signature: &str,
    skip: impl Fn(usize) -> bool,
) -> u32 {
    let mut count = 0;
    for (i, a) in attempts.iter().enumerate().rev() {
        if a.status == AttemptStatus::Interrupted {
            continue;
        }
        match &a.failure {
            Some(f) if f.signature == signature => {
                if !skip(i) {
                    count += 1;
                }
            }
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

/// A failure signature (`kind:normalized line`) as the owner reads it:
/// "a verify failure (npm test failed)", "an error (Claude exited with an
/// error)". The normalized line has its digits stripped, so punctuation
/// left dangling at its end goes too.
pub(super) fn signature_phrase(signature: &str) -> String {
    let (kind, line) = signature.split_once(':').unwrap_or((signature, ""));
    let line = line
        .trim()
        .trim_end_matches(|c: char| c == '.' || c == ':' || c.is_whitespace());
    let what = match kind {
        "error" => "an error".to_string(),
        other => format!("a {} failure", other.replace('_', " ")),
    };
    if line.is_empty() {
        what
    } else {
        format!("{what} ({line})")
    }
}

pub fn decide_after_failure(input: &FailureDecisionInput) -> FailureDecision {
    if input.consecutive_same >= 3
        || input.repeated_reviews >= 2
        || input.attempt_n >= input.max_attempts
    {
        return FailureDecision::Waiting {
            question: format!(
                "{EXHAUSTED_QUESTION} with {}: continue, change approach, or stop?",
                signature_phrase(input.signature)
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

pub(super) fn failure(kind: FailureKind, detail: String) -> Failure {
    Failure {
        signature: failure_signature(kind, &detail),
        kind,
        detail,
    }
}

pub(super) fn record_failure(task: &mut Task, idx: usize, kind: FailureKind, detail: String) {
    crate::task::attempts::end(
        task,
        idx,
        crate::task::attempts::AttemptEnd::Failed(failure(kind, detail)),
        now_ms(),
    );
}

/// How many *implement* attempts a task has made -- the plan attempt (if
/// any) never counts against `maxAttempts`.
pub(super) fn implement_attempt_count(task: &Task) -> u32 {
    crate::task::attempts::implement_count(task)
}

/// Implement attempts that ran to an end: an interrupted one (a restart, a
/// limit pause) is no attempt for "has this task started implementing".
pub(super) fn completed_implement_attempts(task: &Task) -> u32 {
    crate::task::attempts::completed_implement_count(task)
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

/// The hard bound behind the relaxed `maxAttempts` count: every implement
/// attempt, however it ended, except those a usage or rate limit paused, stops
/// the task at twice the budget.
fn ceiling_hit(task: &Task, max_attempts: u32) -> bool {
    implement_attempt_count(task).saturating_sub(task.queue.paused_attempts)
        >= max_attempts.saturating_mul(2)
}

#[cfg(test)]
pub(super) fn advance_after_failure(task: &mut Task, max_attempts: u32) -> bool {
    advance_after_failure_with(task, max_attempts, &|_| None)
}

/// [`advance_after_failure`] with the base checkout's output per command,
/// which backs the environment verdict of a flaky or outside-diff failure.
pub(super) fn advance_after_failure_with(
    task: &mut Task,
    max_attempts: u32,
    base: super::ladder::BaseTail,
) -> bool {
    let last = task.attempts.last().expect("failure just recorded");
    let signature = last
        .failure
        .as_ref()
        .map(|f| f.signature.clone())
        .unwrap_or_default();
    let mut consecutive = consecutive_same_signature(&task.attempts, &signature, |i| {
        super::ladder::environment_failure(task, i, base)
    });
    // A finding that was turned into a check or sent to the judge gets
    // another attempt within the budget, not the owner.
    if task
        .findings
        .iter()
        .any(|f| f.handled && f.status == FindingStatus::Open && f.attempt == last.n)
    {
        consecutive = consecutive.min(2);
    }
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
        attempt_n: if ceiling_hit(task, max_attempts) {
            max_attempts.max(counted_attempts(task))
        } else {
            counted_attempts(task)
        },
        max_attempts,
    };
    match decide_after_failure(&input) {
        FailureDecision::NextAttempt { tier } => {
            if last_review_repeated(task) {
                task.record_decision(format!(
                    "Orchestrator: a review finding repeated -> advisor, tier {}",
                    tier.as_str()
                ));
            }
            task.record_next_tier(tier);
            task.resume_phase(TaskStatus::Queued, crate::clock::now_ms());
            true
        }
        FailureDecision::Waiting { question } => {
            let options: Vec<String> =
                vec!["continue".into(), "change approach".into(), "stop".into()];
            let mut question = Question::new(
                question,
                options,
                QuestionKind::AttemptsFailing,
                AskedBy::Implement,
                crate::clock::now_ms(),
            );
            question.failure_signature = Some(signature);
            task.ask(question, now_ms());
            task.resume_phase(TaskStatus::Waiting, crate::clock::now_ms());
            false
        }
    }
}

pub(super) enum LoopSignal {
    Continue,
    Changed,
    Stop,
}

/// Record a failure, apply the tier/waiting rules, and -- when attempts are
/// exhausted -- walk the failure ladder (`ladder.rs`): a policy or overseer
/// answer, one changed approach, then `failed`. The owner is never asked by
/// the ladder itself.
/// A continue extends the attempt budget by 2.
pub(super) async fn fail_and_continue(
    app: &Arc<App>,
    task: &mut Task,
    failure: Failure,
    attempt_budget: &mut u32,
    slot: &mut QuestionSlot<'_>,
) -> LoopSignal {
    let idx = task.attempts.len() - 1;
    crate::task::attempts::end(
        task,
        idx,
        crate::task::attempts::AttemptEnd::Failed(failure),
        now_ms(),
    );
    let base = super::ladder::base_tails(app, &task.base_sha);
    let should_continue = advance_after_failure_with(task, *attempt_budget, &base);
    if should_continue {
        app.save_task_logged(task);
        app.broadcast_task(task);
        return LoopSignal::Continue;
    }
    // Not persisted here: the wait does it itself, after the one-shot is
    // installed (see `wait_for_answer`).
    let answered = match wait_for_exhausted_answer(app, task, slot).await {
        Ok(answered) => answered,
        Err(WaitEnd::Changed) => return LoopSignal::Changed,
        Err(WaitEnd::Stopped) => return LoopSignal::Stop,
    };
    settle_exhausted_answer(app, task, idx, &answered.text);
    *attempt_budget += 2;
    LoopSignal::Continue
}

/// Owner answer `drop criterion`: removes criterion `n`, its checks and a
/// held-out check on it, and shifts the later checks down by one. Shared by
/// the live loop and the post-restart branch of `task.answer`.
pub(super) fn drop_criterion(
    settings: &Settings,
    task: &mut Task,
    n: usize,
    origin: Origin,
) -> Result<(), String> {
    let Some(edit) = crate::task::checks::snapshot::CriterionEdit::capture(task, n) else {
        return Ok(());
    };
    crate::task::checks::criteria::drop_criterion(
        &check_authority(settings, task),
        task,
        &edit,
        origin,
    )
}

const IMPOSSIBLE_OPTIONS: [&str; 3] = ["drop criterion", "retry", "stop"];

/// The owner question for a `sushi-impossible` claim.
pub(super) fn impossible_question(
    task: &Task,
    claim: &crate::engine::attempts::report::Impossible,
) -> Question {
    let mut question = Question::new(
        format!(
            "Criterion {} cannot be met as written: {}. Evidence: {}",
            claim.criterion, task.criteria[claim.criterion], claim.evidence
        ),
        IMPOSSIBLE_OPTIONS.iter().map(|o| o.to_string()).collect(),
        QuestionKind::Impossible,
        AskedBy::Implement,
        crate::clock::now_ms(),
    );
    question.criterion = Some(claim.criterion);
    question.evidence = Some(claim.evidence.clone());
    question
}

/// The criterion an [`impossible_question`] is about, and whether `answer`
/// is its `drop criterion` option.
pub(super) fn impossible_drop_target(question: &Question, answer: &str) -> Option<usize> {
    if !answer.trim().eq_ignore_ascii_case("drop criterion") {
        return None;
    }
    question.criterion
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
        task.fixture_data().criteria = vec!["zero".into(), "one".into(), "two".into()];
        task.fixture_data().checks =
            vec![check(0, "a"), check(1, "b"), check(2, "c"), check(1, "b2")];
        task.fixture_data().held_out = Some(check(2, "h"));
        let _ = drop_criterion(&Settings::default(), &mut task, 1, Origin::Owner);
        assert_eq!(task.criteria, vec!["zero", "two"]);
        assert_eq!(task.checks, vec![check(0, "a"), check(1, "c")]);
        assert_eq!(task.held_out, Some(check(1, "h")));
        assert_eq!(
            task.decisions,
            vec!["Orchestrator: dropped criterion 1 (one) as the owner answered"]
        );
        // The held-out check goes with its own criterion.
        let _ = drop_criterion(&Settings::default(), &mut task, 1, Origin::Owner);
        assert_eq!(task.held_out, None);
        assert_eq!(task.checks, vec![check(0, "a")]);
        // Out of range: nothing happens.
        let before = task.decisions.len();
        let _ = drop_criterion(&Settings::default(), &mut task, 9, Origin::Owner);
        assert_eq!(task.decisions.len(), before);
    }

    #[test]
    fn dropping_a_criterion_shifts_the_stored_rulings_so_a_later_one_is_not_settled() {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.fixture_data().criteria = vec!["A".into(), "B".into(), "C".into()];
        let ruling = |index, met| CriterionRuling {
            criterion: String::new(),
            index: Some(index),
            met: Some(met),
            evidence: None,
        };
        let mut attempt = attempt_with_failure(1, "x");
        attempt.review = Some(ReviewResult {
            verdict: Verdict::Fail,
            findings: Vec::new(),
            repeated: Vec::new(),
            severities: Vec::new(),
            details: Vec::new(),
            criteria: vec![ruling(0, false), ruling(1, true), ruling(2, false)],
        });
        task.fixture_data().attempts = vec![attempt];
        let _ = drop_criterion(&Settings::default(), &mut task, 0, Origin::Owner);
        let kept: Vec<(Option<usize>, Option<bool>)> = task.attempts[0]
            .review
            .as_ref()
            .unwrap()
            .criteria
            .iter()
            .map(|r| (r.index, r.met))
            .collect();
        assert_eq!(kept, [(Some(0), Some(true)), (Some(1), Some(false))]);
        let results = criteria_results(&task, &task.attempts[0]);
        assert_eq!(results[0].status, CriterionStatus::Met);
        assert_eq!(results[1].status, CriterionStatus::Failing);
    }

    #[test]
    fn dropping_a_criterion_removes_its_command_from_every_place() {
        let cmd = "node scripts/x.mjs";
        let mut task = task_with_status(TaskStatus::Waiting);
        task.fixture_data().criteria = vec![format!("zero -- check: {cmd}"), "one".into()];
        task.fixture_data().checks = vec![check(0, cmd), check(1, cmd), check(1, "keep")];
        task.fixture_data().held_out = Some(check(1, cmd));
        task.fixture_data().verify = vec![cmd.into(), "node scripts/x.mjs --more".into()];
        task.fixture_data().final_verify = vec![cmd.into()];
        drop_criterion(&Settings::default(), &mut task, 0, Origin::Owner).unwrap();
        assert_eq!(task.verify, vec!["node scripts/x.mjs --more"]);
        assert!(task.final_verify.is_empty());
        assert_eq!(task.checks, vec![check(0, "keep")]);
        assert_eq!(task.held_out, None);
        assert_eq!(
            task.decisions.last().unwrap(),
            &format!(
                "Orchestrator: dropping criterion 0 also removed `{cmd}` from verify, finalVerify, checks, heldOut"
            )
        );
        assert_eq!(task.decisions.len(), 2);
    }

    #[test]
    fn a_non_owner_drop_keeps_a_repo_sourced_verify_duplicate() {
        let mut task = task_with_status(TaskStatus::Waiting);
        task.fixture_data().criteria = vec!["zero -- check: npm run ci".into(), "one".into()];
        task.fixture_data().checks = vec![check(0, "npm run ci")];
        task.fixture_data().verify = vec!["npm run ci".into()];
        let r = drop_criterion(&Settings::default(), &mut task, 0, Origin::Other);
        assert!(r.is_err());
        assert_eq!(task.criteria.len(), 2);
        assert_eq!(task.verify, vec!["npm run ci"]);
        assert_eq!(task.checks.len(), 1);
    }

    #[test]
    fn the_impossible_question_round_trips_to_its_criterion() {
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().criteria = vec!["zero".into(), "one".into()];
        let claim = crate::engine::attempts::report::Impossible {
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
