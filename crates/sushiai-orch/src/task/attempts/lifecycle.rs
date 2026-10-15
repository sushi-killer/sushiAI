use crate::policy::routing::Route;
use crate::task::{Attempt, AttemptStatus, Failure, FailureKind, Fingerprint, Stage, Task};

pub(crate) fn next_number(task: &crate::task::TaskData) -> u32 {
    task.attempts
        .iter()
        .map(|attempt| attempt.n)
        .max()
        .unwrap_or(0)
        + 1
}

pub(crate) fn start(task: &mut Task, stage: Stage, route: &Route, reason: &str, now: i64) -> usize {
    let task = &mut task.data;
    let attempt = Attempt {
        n: next_number(task),
        stage,
        route_id: route.id.clone(),
        harness: route.harness,
        model: route.model.clone().unwrap_or_default(),
        reason: reason.to_string(),
        session_id: None,
        pgid: None,
        started_at: now,
        ended_at: None,
        status: AttemptStatus::Running,
        summary: None,
        handoff: None,
        declines: vec![],
        changed_files: vec![],
        verify: vec![],
        gate_blocks: 0,
        prefix_tokens: None,
        first_edit_at: None,
        review: None,
        failure: None,
        usage: None,
        cost_usd: None,
        cost_estimated: false,
        review_cost_usd: None,
        evidence: vec![],
        evidence_tree: None,
        review_tree: None,
        review_base: None,
        review_scope: None,
        evidence_base: None,
        evidence_from: None,
        advice: None,
        advisor_cost_usd: None,
        fingerprint: None,
        review_fingerprint: None,
        advisor_fingerprint: None,
        candidates: vec![],
        criteria_results: vec![],
        cause: None,
        diff_stat: None,
    };
    task.attempts.push(attempt);
    task.updated_at = now;
    task.attempts.len() - 1
}

#[derive(Debug, Clone)]
pub(crate) enum RunObservation {
    Session(String),
    ProcessGroup(i32),
    Fingerprint(Fingerprint),
    FirstEdit,
}

/// Records an observation by the existing attempt number, including events
/// arriving before the run ends. Returns whether the attempt still exists.
pub(crate) fn observe_run(
    task: &mut Task,
    number: u32,
    observation: RunObservation,
    now: i64,
) -> bool {
    let task = &mut task.data;
    task.updated_at = now;
    let Some(attempt) = task.attempts.iter_mut().find(|attempt| attempt.n == number) else {
        return false;
    };
    match observation {
        RunObservation::Session(session) => attempt.session_id = Some(session),
        RunObservation::ProcessGroup(pgid) => attempt.pgid = Some(pgid),
        RunObservation::Fingerprint(fingerprint) => attempt.fingerprint = Some(fingerprint),
        RunObservation::FirstEdit => {
            if attempt.stage == Stage::Implement && attempt.first_edit_at.is_none() {
                attempt.first_edit_at = Some(now);
            }
        }
    }
    true
}

pub(crate) enum AttemptEnd {
    Passed,
    Blocked,
    Interrupted,
    Failed(Failure),
}

/// Ending an attempt does not decide the task's next phase or erase diagnostics.
pub(crate) fn end(task: &mut Task, index: usize, outcome: AttemptEnd, now: i64) {
    let task = &mut task.data;
    let attempt = &mut task.attempts[index];
    attempt.status = match outcome {
        AttemptEnd::Passed => AttemptStatus::Passed,
        AttemptEnd::Blocked => AttemptStatus::Blocked,
        AttemptEnd::Interrupted => AttemptStatus::Interrupted,
        AttemptEnd::Failed(failure) => {
            attempt.failure = Some(failure);
            AttemptStatus::Failed
        }
    };
    attempt.ended_at = Some(now);
}

/// The caller established that the harness never started, so no attempt is spent.
pub(crate) fn discard(task: &mut Task, index: usize, now: i64) {
    let task = &mut task.data;
    task.attempts.remove(index);
    task.updated_at = now;
}

pub(crate) fn implement_count(task: &crate::task::TaskData) -> u32 {
    task.attempts
        .iter()
        .filter(|attempt| attempt.stage == Stage::Implement)
        .count() as u32
}

pub(crate) fn completed_implement_count(task: &crate::task::TaskData) -> u32 {
    task.attempts
        .iter()
        .filter(|attempt| attempt.stage == Stage::Implement)
        .filter(|attempt| attempt.status != AttemptStatus::Interrupted)
        .count() as u32
}

pub(crate) fn counted_implement_count(task: &crate::task::TaskData) -> u32 {
    task.attempts
        .iter()
        .filter(|attempt| attempt.stage == Stage::Implement)
        .filter(|attempt| attempt.status != AttemptStatus::Interrupted)
        .filter(|attempt| {
            !(attempt.changed_files.is_empty()
                && attempt.failure.as_ref().map(|failure| failure.kind) == Some(FailureKind::Error))
        })
        .count() as u32
}
