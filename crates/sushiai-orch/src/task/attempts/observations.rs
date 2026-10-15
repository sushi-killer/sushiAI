use crate::policy::routing::{Harness, Route};
use crate::task::{
    Candidate, Decline, DiffStat, Fingerprint, ReviewResult, ReviewScope, Task, Usage,
    VerifyOutcome,
};

/// The task-relevant measurements of a completed native run.
pub(crate) struct RunCompletion {
    pub(crate) session: Option<String>,
    pub(crate) fingerprint: Option<Fingerprint>,
    pub(crate) usage: Usage,
    pub(crate) first_turn_tokens: Option<u64>,
    pub(crate) cost: Option<f64>,
    pub(crate) estimated: bool,
}

pub(crate) fn record_run_completion(task: &mut Task, index: usize, run: RunCompletion) {
    let task = &mut task.data;
    let attempt = &mut task.attempts[index];
    attempt.session_id = run.session;
    attempt.fingerprint = run.fingerprint;
    attempt.usage = Some(run.usage);
    if attempt.harness == Harness::Claude {
        attempt.prefix_tokens = run.first_turn_tokens;
    }
    attempt.cost_usd = run.cost;
    attempt.cost_estimated = run.estimated;
    if let Some(cost) = run.cost {
        task.cost_usd += cost;
    }
}

pub(crate) struct BestOfSelection {
    pub(crate) winner: Option<Route>,
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) extra_cost: f64,
}

pub(crate) fn record_best_of(task: &mut Task, index: usize, selection: BestOfSelection) {
    let task = &mut task.data;
    task.cost_usd += selection.extra_cost;
    let attempt = &mut task.attempts[index];
    attempt.candidates = selection.candidates;
    if let Some(winner) = selection.winner {
        attempt.route_id = winner.id;
        attempt.harness = winner.harness;
        attempt.model = winner.model.unwrap_or_default();
        attempt.reason = format!("{}; best-of winner", attempt.reason);
    }
}

pub(crate) fn record_gate_blocks(task: &mut Task, index: usize, blocks: u32) {
    let task = &mut task.data;
    task.attempts[index].gate_blocks = blocks;
}

pub(crate) struct AttemptReport {
    pub(crate) summary: Option<String>,
    pub(crate) handoff: Option<String>,
    pub(crate) declines: Vec<Decline>,
}

pub(crate) fn record_report(task: &mut Task, index: usize, report: AttemptReport) {
    let task = &mut task.data;
    let attempt = &mut task.attempts[index];
    attempt.summary = report.summary;
    attempt.handoff = report.handoff;
    attempt.declines = report.declines;
}

pub(crate) fn record_changed_files(task: &mut Task, index: usize, files: Vec<String>) {
    let task = &mut task.data;
    task.attempts[index].changed_files = files;
}

pub(crate) enum VerifyObservation {
    Replaced(Vec<VerifyOutcome>),
    Appended(Vec<VerifyOutcome>),
}

pub(crate) fn record_verify(task: &mut Task, index: usize, observation: VerifyObservation) {
    let task = &mut task.data;
    match observation {
        VerifyObservation::Replaced(results) => task.attempts[index].verify = results,
        VerifyObservation::Appended(results) => task.attempts[index].verify.extend(results),
    }
}

pub(crate) enum EvidenceObservation {
    Captured {
        files: Vec<String>,
        tree: Option<String>,
        base: String,
    },
    Reused {
        files: Vec<String>,
        from: u32,
    },
    Additional(Vec<String>),
}

pub(crate) fn record_evidence(task: &mut Task, index: usize, observation: EvidenceObservation) {
    let task = &mut task.data;
    let attempt = &mut task.attempts[index];
    match observation {
        EvidenceObservation::Captured { files, tree, base } => {
            attempt.evidence = files;
            attempt.evidence_tree = tree;
            attempt.evidence_base = Some(base);
        }
        EvidenceObservation::Reused { files, from } => {
            attempt.evidence = files;
            attempt.evidence_from = Some(from);
        }
        EvidenceObservation::Additional(files) => {
            for file in files {
                if !attempt.evidence.contains(&file) {
                    attempt.evidence.push(file);
                }
            }
            attempt.evidence.sort();
        }
    }
}

pub(crate) struct ReviewView {
    pub(crate) tree: Option<String>,
    pub(crate) base: String,
    pub(crate) scope: ReviewScope,
}

pub(crate) fn record_review_view(task: &mut Task, index: usize, view: Option<ReviewView>) {
    let task = &mut task.data;
    let attempt = &mut task.attempts[index];
    attempt.review_tree = view.as_ref().and_then(|view| view.tree.clone());
    attempt.review_base = view.as_ref().map(|view| view.base.clone());
    attempt.review_scope = view.map(|view| view.scope);
}

pub(crate) enum ReviewState {
    Recorded(ReviewResult),
    Discarded,
}

pub(crate) fn record_review(task: &mut Task, index: usize, state: ReviewState) {
    let task = &mut task.data;
    task.attempts[index].review = match state {
        ReviewState::Recorded(review) => Some(review),
        ReviewState::Discarded => None,
    };
}

pub(crate) fn record_review_cost(
    task: &mut Task,
    index: usize,
    cost: f64,
    fingerprint: Option<Fingerprint>,
) {
    let task = &mut task.data;
    task.cost_usd += cost;
    let attempt = &mut task.attempts[index];
    if fingerprint.is_some() {
        attempt.review_fingerprint = fingerprint;
    }
    attempt.review_cost_usd = Some(attempt.review_cost_usd.unwrap_or(0.0) + cost);
}

pub(crate) fn record_diff_stat(task: &mut Task, index: usize, stat: DiffStat) {
    let task = &mut task.data;
    task.attempts[index].diff_stat = Some(stat);
    task.diff_stat = Some(stat);
}

pub(crate) struct PlanRunObservation {
    pub(crate) cost: Option<f64>,
    pub(crate) estimated: bool,
    pub(crate) fingerprint: Option<Fingerprint>,
    pub(crate) usage: Option<Usage>,
}

pub(crate) fn record_plan_run(task: &mut Task, index: usize, run: PlanRunObservation) {
    let task = &mut task.data;
    let attempt = &mut task.attempts[index];
    if let Some(cost) = run.cost {
        attempt.cost_usd = Some(attempt.cost_usd.unwrap_or(0.0) + cost);
        attempt.cost_estimated |= run.estimated;
        task.cost_usd += cost;
    }
    attempt.fingerprint = run.fingerprint;
    if let Some(usage) = run.usage {
        let saved = attempt.usage.get_or_insert(Usage {
            input: 0,
            output: 0,
            cached: 0,
        });
        saved.input += usage.input;
        saved.output += usage.output;
        saved.cached += usage.cached;
    }
}

pub(crate) fn record_advisor(
    task: &mut Task,
    index: usize,
    cost: f64,
    fingerprint: Option<Fingerprint>,
) {
    let attempt = &mut task.data.attempts[index];
    attempt.advisor_cost_usd = Some(cost);
    attempt.advisor_fingerprint = fingerprint;
}

pub(crate) fn record_advice(task: &mut Task, index: usize, advice: String) {
    task.data.attempts[index].advice = Some(advice);
}

pub(crate) fn record_failure_signature(task: &mut Task, index: usize, signature: String) {
    if let Some(failure) = &mut task.data.attempts[index].failure {
        failure.signature = signature;
    }
}

pub(crate) fn record_cause(task: &mut Task, cause: Option<crate::task::Blame>) {
    if let Some(attempt) = task
        .data
        .attempts
        .iter_mut()
        .rev()
        .find(|a| a.stage == crate::task::Stage::Implement)
    {
        attempt.cause = cause;
    }
}

pub(crate) fn retain_evidence(task: &mut Task, present: &[String]) {
    for attempt in &mut task.data.attempts {
        attempt.evidence.retain(|path| present.contains(path));
    }
}
