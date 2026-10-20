//! First-pass metrics of a finished task (`Task.metrics`): computed once when
//! the task loop ends on `done` or `failed`, and summed into one `done` entry
//! of the overseer log for a top-level task. Everything here but
//! [`App::record_task_metrics`] is pure.

#[cfg(test)]
use crate::task::AnsweredQuestion;
#[cfg(test)]
use crate::task::Attempt;
#[cfg(test)]
use crate::task::FindingRecord;
#[cfg(test)]
use crate::task::PlanFinding;
#[cfg(test)]
use crate::task::ReviewResult;
#[cfg(test)]
use serde_json::json;

use crate::api::events::Event;
use crate::clock::now_ms;
use crate::engine::decision::implement_attempt_count;
use crate::engine::state::App;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
use crate::store;
use crate::store::overseer;
use crate::task::review::severity_of;
use crate::task::AnsweredBy;
use crate::task::AttemptStatus;
use crate::task::Failure;
use crate::task::FailureCause;
use crate::task::FailureKind;
use crate::task::QuestionKind;
use crate::task::Severity;
use crate::task::Stage;
use crate::task::Task;
use crate::task::TaskMetrics;
use crate::task::TaskStatus;
use crate::task::COST_PHASES;

use crate::store::costs::CostRecord;
use std::collections::{BTreeMap, HashSet};

/// How many finished top-level tasks the repo line of the entry covers.
const REPO_WINDOW: usize = 20;

/// A parent's subtasks (the repo's tasks that name it as parent).
fn children<'a>(task: &Task, all: &'a [Task]) -> Vec<&'a Task> {
    all.iter()
        .filter(|t| t.parent.as_deref() == Some(task.id.as_str()))
        .collect()
}

/// Whether a review finding blocks: every severity but P3 does, and a finding
/// that names none counts as blocking.
fn blocks(severity: Option<u8>) -> bool {
    severity != Some(3)
}

/// A leaf is first-pass when it is done, ran one implement attempt that
/// passed, and neither any attempt's review nor the finding registry holds a
/// blocking finding.
fn leaf_first_pass(task: &Task) -> bool {
    if task.status != TaskStatus::Done || implement_attempt_count(task) != 1 {
        return false;
    }
    let implement_passed = task
        .attempts
        .iter()
        .any(|a| a.stage == Stage::Implement && a.status == AttemptStatus::Passed);
    let review_blocks = task
        .attempts
        .iter()
        .filter_map(|a| a.review.as_ref())
        .any(|r| (0..r.findings.len()).any(|i| blocks(r.severities.get(i).copied().flatten())));
    // A record stored before severities were typed carries its level only in its text.
    let registry_blocks = task
        .findings
        .iter()
        .any(|f| blocks(f.severity.or_else(|| severity_of(&f.text))));
    implement_passed && !review_blocks && !registry_blocks
}

fn first_pass_on_path<'a>(task: &'a Task, all: &'a [Task], path: &mut Vec<&'a str>) -> bool {
    if path.contains(&task.id.as_str()) {
        return false;
    }
    let kids = children(task, all);
    if kids.is_empty() {
        return leaf_first_pass(task);
    }
    if task.status != TaskStatus::Done || implement_attempt_count(task) != 0 {
        return false;
    }
    path.push(task.id.as_str());
    let ok = kids.iter().all(|c| first_pass_on_path(c, all, path));
    path.pop();
    ok
}

/// First-pass, the same definition as the eval export's. A leaf is done with
/// one passed implement attempt and no blocking finding (any review or the
/// finding registry; only P3 does not block). A parent is done, ran no
/// implement attempt of its own, and every subtask is first-pass in turn.
pub(super) fn first_pass(task: &Task, all: &[Task]) -> bool {
    first_pass_on_path(task, all, &mut Vec::new())
}

/// Owner-answered questions plus `Amended: ` decision lines, over the task
/// and its subtasks.
pub(super) fn owner_touches(task: &Task, children: &[&Task]) -> u32 {
    std::iter::once(task)
        .chain(children.iter().copied())
        .map(|t| {
            let answered = t
                .question_history
                .iter()
                .filter(|q| q.answered_by == AnsweredBy::Owner)
                .count();
            let amended = t
                .decisions
                .iter()
                .filter(|d| d.starts_with("Amended: "))
                .count();
            (answered + amended) as u32
        })
        .sum()
}

/// USD per phase of `COST_PHASES` over the records of `ids`; every phase is
/// listed, 0 when it had no spend.
pub(super) fn cost_by_phase(records: &[CostRecord], ids: &HashSet<&str>) -> BTreeMap<String, f64> {
    let mut out: BTreeMap<String, f64> = COST_PHASES.iter().map(|p| (p.to_string(), 0.0)).collect();
    for r in records {
        let Some(id) = r.task_id.as_deref() else {
            continue;
        };
        if !ids.contains(id) {
            continue;
        }
        if let Some(sum) = out.get_mut(r.stage.as_str()) {
            *sum += r.cost_usd;
        }
    }
    out
}

/// The failure kind of one attempt mapped to a cause. A base-check failure
/// is a `Verify` failure whose signature starts with `base:`.
fn cause_of_failure(failure: &Failure) -> FailureCause {
    if failure.signature.starts_with("base:") {
        return FailureCause::Env;
    }
    match failure.kind {
        FailureKind::Error | FailureKind::Budget => FailureCause::Env,
        FailureKind::Review => FailureCause::ReviewLoop,
        FailureKind::Verify
        | FailureKind::Heldout
        | FailureKind::Loop
        | FailureKind::Stall
        | FailureKind::TimeBudget
        | FailureKind::Protected
        | FailureKind::NoDeliverable
        | FailureKind::Evidence
        | FailureKind::Conflict
        | FailureKind::Blocked => FailureCause::Impl,
    }
}

fn env_question(kind: QuestionKind) -> bool {
    matches!(
        kind,
        QuestionKind::Budget | QuestionKind::HarnessMissing | QuestionKind::PreexistingFailure
    )
}

/// Why a `failed` task failed, from its last failure: the latest of its
/// failed attempts and its env questions (budget, missing harness, a check
/// failing on the base). A failed or interrupted plan stage with no implement
/// attempt is `plan`; a failure that reads `impl` while the plan gate still
/// holds a critical finding is `plan` too (the gate let a bad plan through).
/// `None` for a task that is not failed.
pub(super) fn failure_cause(task: &Task, children: &[&Task]) -> Option<FailureCause> {
    if task.status != TaskStatus::Failed {
        return None;
    }
    let implemented = implement_attempt_count(task) > 0;
    // The latest signal wins, a later list entry wins a tie.
    let mut signals: Vec<(i64, FailureCause)> = Vec::new();
    for a in &task.attempts {
        let at = a.ended_at.unwrap_or(a.started_at);
        if a.stage == Stage::Plan {
            if !implemented
                && (a.failure.is_some()
                    || matches!(a.status, AttemptStatus::Failed | AttemptStatus::Interrupted))
            {
                signals.push((at, FailureCause::Plan));
            }
        } else if let Some(f) = &a.failure {
            signals.push((at, cause_of_failure(f)));
        }
    }
    for q in &task.question_history {
        if env_question(q.kind) {
            signals.push((q.answered_at, FailureCause::Env));
        }
    }
    if task.question.as_ref().is_some_and(|q| env_question(q.kind)) {
        signals.push((task.updated_at, FailureCause::Env));
    }
    let latest = signals
        .iter()
        .enumerate()
        .max_by_key(|(i, (at, _))| (*at, *i))
        .map(|(_, (_, cause))| *cause);
    let own = latest.or_else(|| {
        children
            .iter()
            .find(|c| c.status == TaskStatus::Failed)
            .and_then(|c| failure_cause(c, &[]))
    });
    let cause = own.unwrap_or(FailureCause::Env);
    let critical_left = task
        .plan_gate
        .findings
        .iter()
        .any(|f| f.severity == Severity::Critical);
    if cause == FailureCause::Impl && critical_left {
        return Some(FailureCause::Plan);
    }
    Some(cause)
}

/// The metrics of a finished task, from the repo's tasks and cost records.
pub(super) fn compute(task: &Task, all: &[Task], records: &[CostRecord], now: i64) -> TaskMetrics {
    let kids = children(task, all);
    let ids: HashSet<&str> = std::iter::once(task.id.as_str())
        .chain(kids.iter().map(|c| c.id.as_str()))
        .collect();
    TaskMetrics {
        first_pass: first_pass(task, all),
        owner_touches: owner_touches(task, &kids),
        cost_by_phase: cost_by_phase(records, &ids),
        failure_cause: failure_cause(task, &kids),
        ended_at: now,
    }
}

fn total(m: &TaskMetrics) -> f64 {
    m.cost_by_phase.values().sum()
}

/// `first pass: yes; owner touches: 0; cause: none; cost: plan $0.02, ...`
pub(super) fn entry_text(m: &TaskMetrics) -> String {
    let phases: Vec<String> = COST_PHASES
        .iter()
        .filter_map(|p| {
            let usd = m.cost_by_phase.get(*p).copied().unwrap_or(0.0);
            (format!("{usd:.2}") != "0.00").then(|| format!("{p} ${usd:.2}"))
        })
        .collect();
    let cost = if phases.is_empty() {
        "$0.00".to_string()
    } else {
        phases.join(", ")
    };
    format!(
        "first pass: {}; owner touches: {}; cause: {}; cost: {cost}",
        if m.first_pass { "yes" } else { "no" },
        m.owner_touches,
        m.failure_cause.map_or("none", FailureCause::as_str),
    )
}

/// The repo line of the entry detail: first-pass rate and average cost per
/// landed task over the last `REPO_WINDOW` finished top-level tasks.
pub(super) fn repo_line(repo_tasks: &[Task]) -> String {
    let mut done: Vec<(&Task, &TaskMetrics)> = repo_tasks
        .iter()
        .filter(|t| t.parent.is_none())
        .filter_map(|t| t.metrics.as_ref().map(|m| (t, m)))
        .collect();
    done.sort_by(|a, b| b.1.ended_at.cmp(&a.1.ended_at).then(a.0.id.cmp(&b.0.id)));
    done.truncate(REPO_WINDOW);
    let n = done.len();
    let first = done.iter().filter(|(_, m)| m.first_pass).count();
    let landed = done
        .iter()
        .filter(|(t, _)| t.status == TaskStatus::Done)
        .count();
    let spend: f64 = done.iter().map(|(_, m)| total(m)).sum();
    let rate = (first * 100).checked_div(n).unwrap_or(0);
    let average = if landed == 0 {
        "n/a".to_string()
    } else {
        format!("${:.2}", spend / landed as f64)
    };
    format!(
        "Repo (last {REPO_WINDOW} finished): first-pass rate {rate}% ({first}/{n}); average cost per landed task {average} ({landed} landed)"
    )
}

impl App {
    /// Called when a task loop ends. A task that is `done` or `failed` and
    /// has no metrics gets them, and a top-level one gets one `done` entry in
    /// the overseer log. Metrics left on a task that is no longer finished
    /// are cleared. Idempotent.
    pub(super) fn record_task_metrics(&self, task_id: &str) {
        let Ok(Some(task)) = self.store.load_task(task_id) else {
            return;
        };
        let finished = matches!(task.status, TaskStatus::Done | TaskStatus::Failed);
        if !finished {
            if task.metrics.is_some() {
                let _ = self.update_task(task_id, |t| t.record_metrics(None));
            }
            return;
        }
        let mut all = self.repo_tasks(&task.repo);
        let metrics = task.metrics.clone().unwrap_or_else(|| {
            let records = store::costs::read_all(&self.data_dir);
            compute(&task, &all, &records, now_ms())
        });
        if task.parent.is_none() {
            if let Some(current) = all.iter_mut().find(|current| current.id == task.id) {
                current.record_metrics(Some(metrics.clone()));
            }
            let line = entry_text(&metrics);
            let detail = format!("{line}\n{}", repo_line(&all));
            let entry = crate::task::notes::overseer::Entry::new(
                "done",
                &task.repo,
                Some(&task.id),
                &line,
                crate::clock::now_ms(),
            )
            .with_detail(&detail);
            if matches!(overseer::append_task_once(&self.data_dir, &entry), Ok(true)) {
                let _ = self.events_tx.send(Event::Overseer {
                    entry: Box::new(entry),
                });
            }
        }
        // Only the metrics are written, under the task lock: the slow work
        // above must not save a stale copy over an owner's mark.
        let _ = self.update_task(task_id, |t| t.record_metrics(Some(metrics)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(stage: Stage, status: AttemptStatus, kind: Option<FailureKind>, at: i64) -> Attempt {
        let mut a: Attempt = serde_json::from_value(json!({
            "n": 1, "stage": "implement", "routeId": "r", "harness": "claude",
            "model": "m", "reason": "", "startedAt": at, "status": "running",
        }))
        .unwrap();
        a.stage = stage;
        a.status = status;
        a.ended_at = Some(at);
        a.failure = kind.map(|kind| Failure {
            kind,
            detail: String::new(),
            signature: String::new(),
        });
        a
    }

    fn failed_with(kind: FailureKind) -> Task {
        let mut t = task_with_status(TaskStatus::Failed);
        t.fixture_data().attempts = vec![attempt(
            Stage::Implement,
            AttemptStatus::Failed,
            Some(kind),
            10,
        )];
        t
    }

    fn cause(t: &Task) -> Option<FailureCause> {
        failure_cause(t, &[])
    }

    #[test]
    fn every_failure_kind_maps_to_one_cause() {
        use FailureCause::*;
        use FailureKind as K;
        let table = [
            (K::Error, Env),
            (K::Budget, Env),
            (K::Verify, Impl),
            (K::Heldout, Impl),
            (K::Loop, Impl),
            (K::Stall, Impl),
            (K::TimeBudget, Impl),
            (K::Protected, Impl),
            (K::NoDeliverable, Impl),
            (K::Evidence, Impl),
            (K::Conflict, Impl),
            (K::Blocked, Impl),
            (K::Review, ReviewLoop),
        ];
        for (kind, want) in table {
            assert_eq!(cause(&failed_with(kind)), Some(want), "{kind:?}");
        }
    }

    #[test]
    fn a_done_task_has_no_cause() {
        let mut t = failed_with(FailureKind::Verify);
        t.resume_phase(TaskStatus::Done, crate::clock::now_ms());
        assert_eq!(cause(&t), None);
    }

    #[test]
    fn a_base_check_failure_is_env() {
        let mut t = failed_with(FailureKind::Verify);
        t.fixture_data().attempts[0]
            .failure
            .as_mut()
            .unwrap()
            .signature = "base:npm test".into();
        assert_eq!(cause(&t), Some(FailureCause::Env));
    }

    #[test]
    fn env_questions_are_env_and_the_latest_signal_wins() {
        let mut t = failed_with(FailureKind::Review);
        t.fixture_data().question_history.push(AnsweredQuestion {
            question: "q".into(),
            options: vec![],
            kind: QuestionKind::HarnessMissing,
            asked_by: None,
            asked_at: None,
            answer: "stop".into(),
            answered_at: 20,
            answered_by: AnsweredBy::Owner,
            failure_signature: None,
            commands: Vec::new(),
        });
        assert_eq!(cause(&t), Some(FailureCause::Env));
        t.fixture_data().question_history[0].answered_at = 5;
        assert_eq!(cause(&t), Some(FailureCause::ReviewLoop));
        t.fixture_data().question_history[0].kind = QuestionKind::PreexistingFailure;
        t.fixture_data().question_history[0].answered_at = 30;
        assert_eq!(cause(&t), Some(FailureCause::Env));
    }

    #[test]
    fn a_failed_plan_stage_without_implementing_is_plan() {
        let mut t = task_with_status(TaskStatus::Failed);
        t.fixture_data().attempts = vec![attempt(
            Stage::Plan,
            AttemptStatus::Failed,
            Some(FailureKind::Error),
            10,
        )];
        assert_eq!(cause(&t), Some(FailureCause::Plan));
        t.fixture_data().attempts[0].failure = None;
        t.fixture_data().attempts[0].status = AttemptStatus::Interrupted;
        assert_eq!(cause(&t), Some(FailureCause::Plan));
        t.fixture_data().attempts.push(attempt(
            Stage::Implement,
            AttemptStatus::Failed,
            Some(FailureKind::Loop),
            20,
        ));
        assert_eq!(cause(&t), Some(FailureCause::Impl));
    }

    #[test]
    fn a_critical_plan_gate_finding_makes_an_impl_failure_plan() {
        let mut t = failed_with(FailureKind::Verify);
        t.fixture_data().plan_gate.findings.push(PlanFinding {
            id: "coverage-1".into(),
            severity: Severity::Critical,
            kind: "coverage".into(),
            text: "x".into(),
        });
        assert_eq!(cause(&t), Some(FailureCause::Plan));
        t.fixture_data().attempts[0].failure.as_mut().unwrap().kind = FailureKind::Review;
        assert_eq!(cause(&t), Some(FailureCause::ReviewLoop));
    }

    fn done_with(attempts: Vec<Attempt>) -> Task {
        let mut t = task_with_status(TaskStatus::Done);
        t.fixture_data().attempts = attempts;
        t
    }

    fn passed() -> Attempt {
        attempt(Stage::Implement, AttemptStatus::Passed, None, 10)
    }

    fn review_with(findings: &[&str], severities: Vec<Option<u8>>) -> ReviewResult {
        serde_json::from_value(json!({
            "verdict": "PASS",
            "findings": findings,
            "severities": severities,
        }))
        .unwrap()
    }

    fn finding(text: &str) -> FindingRecord {
        serde_json::from_value(json!({
            "id": "F-000001", "text": text, "status": "open", "attempt": 1, "round": 1,
        }))
        .unwrap()
    }

    fn child_of(parent: &Task, mut t: Task) -> Task {
        t.fixture_data().parent = Some(parent.id.clone());
        t
    }

    #[test]
    fn first_pass_needs_one_passed_implement_attempt() {
        let mut t = done_with(vec![passed()]);
        assert!(first_pass(&t, &[]));
        assert_eq!(owner_touches(&t, &[]), 0);
        t.record_decision("Amended: verify".into());
        assert_eq!(owner_touches(&t, &[]), 1);
        t.fixture_data().attempts.push(passed());
        assert!(!first_pass(&t, &[]));
        let failed_once = done_with(vec![attempt(
            Stage::Implement,
            AttemptStatus::Failed,
            None,
            10,
        )]);
        assert!(!first_pass(&failed_once, &[]));
        let mut stopped = done_with(vec![passed()]);
        stopped.resume_phase(TaskStatus::Stopped, 1);
        assert!(!first_pass(&stopped, &[]));
    }

    #[test]
    fn a_blocking_review_finding_in_any_attempt_blocks_first_pass() {
        let mut t = done_with(vec![passed()]);
        t.fixture_data().attempts[0].review = Some(review_with(&["P2: x"], vec![Some(2)]));
        assert!(!first_pass(&t, &[]));
        t.fixture_data().attempts[0].review = Some(review_with(&["P3: x"], vec![Some(3)]));
        assert!(first_pass(&t, &[]));
        t.fixture_data().attempts[0].review = Some(review_with(&["x"], vec![]));
        assert!(!first_pass(&t, &[]));
        t.fixture_data().attempts[0].review = Some(review_with(&["x"], vec![None]));
        assert!(!first_pass(&t, &[]));
        t.fixture_data().attempts[0].review = Some(review_with(&["P3: x", "y"], vec![Some(3)]));
        assert!(!first_pass(&t, &[]));
    }

    #[test]
    fn a_blocking_registry_finding_blocks_first_pass() {
        let mut t = done_with(vec![passed()]);
        t.fixture_data().findings = vec![finding("P3: x")];
        assert!(first_pass(&t, &[]));
        t.fixture_data().findings = vec![finding("P1: x")];
        assert!(!first_pass(&t, &[]));
    }

    // An object-form finding's text does not start with its severity, so the
    // registry must keep the level the review parsed.
    #[test]
    fn an_object_form_p3_finding_stays_clean_through_the_registry() {
        for (level, clean) in [("P3", true), ("P2", false)] {
            let mut t = done_with(vec![passed()]);
            let reply = json!({
                "verdict": "PASS",
                "findings": [{"severity": level, "file": "a.rs", "issue": "typo"}],
            })
            .to_string();
            let mut review = crate::verify::review::reply::parse_review(&reply).unwrap();
            crate::task::findings::register_findings(&mut t, 1, 1, &mut review);
            t.fixture_data().attempts[0].review = Some(review);
            assert_eq!(first_pass(&t, &[]), clean, "{level}");
        }
    }

    // A later round that names the same finding at a lower level does not erase
    // the blocker an earlier round raised.
    #[test]
    fn a_finding_downgraded_on_a_later_round_still_blocks() {
        let mut t = done_with(vec![passed()]);
        let round = |finding: serde_json::Value| {
            crate::verify::review::reply::parse_review(
                &json!({"verdict": "PASS", "findings": [finding]}).to_string(),
            )
            .unwrap()
        };
        let mut first = round(json!({"severity": "P1", "issue": "x"}));
        crate::task::findings::register_findings(&mut t, 1, 1, &mut first);
        let id = t.findings[0].id.clone();
        let mut second = round(json!({"id": id, "severity": "P3", "issue": "x"}));
        crate::task::findings::register_findings(&mut t, 1, 2, &mut second);
        assert_eq!(t.findings.len(), 1);
        t.fixture_data().attempts[0].review = Some(second);
        assert!(!first_pass(&t, &[]));
    }

    #[test]
    fn an_owner_answered_question_does_not_matter() {
        let mut t = done_with(vec![passed()]);
        t.fixture_data().question_history.push(AnsweredQuestion {
            question: "q".into(),
            options: vec![],
            kind: QuestionKind::AgentQuestion,
            asked_by: None,
            asked_at: None,
            answer: "a".into(),
            answered_at: 1,
            answered_by: AnsweredBy::Owner,
            failure_signature: None,
            commands: Vec::new(),
        });
        assert!(first_pass(&t, &[]));
        assert_eq!(owner_touches(&t, &[]), 1);
    }

    #[test]
    fn a_parent_is_first_pass_when_every_descendant_is() {
        let parent = task_with_status(TaskStatus::Done);
        let part = child_of(&parent, done_with(vec![passed()]));
        let mut other = child_of(&parent, done_with(vec![passed()]));
        let all = |kids: &[&Task]| {
            let mut v = vec![parent.clone()];
            v.extend(kids.iter().map(|t| (*t).clone()));
            v
        };
        assert!(first_pass(&parent, &all(&[&part, &other])));
        other.fixture_data().attempts.push(passed());
        assert!(!first_pass(&parent, &all(&[&part, &other])));
        let mut own = parent.clone();
        own.fixture_data().attempts = vec![passed()];
        assert!(!first_pass(&own, &all(&[&part])));
        let mut open = part.clone();
        open.resume_phase(TaskStatus::Queued, 1);
        assert!(!first_pass(&parent, &all(&[&open])));
    }

    #[test]
    fn a_bad_grandchild_makes_the_grandparent_not_first_pass() {
        let grand = task_with_status(TaskStatus::Done);
        let parent = child_of(&grand, task_with_status(TaskStatus::Done));
        let leaf = child_of(&parent, done_with(vec![passed()]));
        let mut all = vec![grand.clone(), parent.clone(), leaf.clone()];
        assert!(first_pass(&grand, &all));
        all[2].fixture_data().attempts = vec![
            attempt(Stage::Implement, AttemptStatus::Failed, None, 5),
            passed(),
        ];
        assert!(!first_pass(&grand, &all));
        assert!(!first_pass(&parent, &all));
    }

    #[test]
    fn a_parent_cycle_is_not_first_pass() {
        let mut a = task_with_status(TaskStatus::Done);
        let mut b = task_with_status(TaskStatus::Done);
        a.fixture_data().parent = Some(b.id.clone());
        b.fixture_data().parent = Some(a.id.clone());
        let all = vec![a.clone(), b];
        assert!(!first_pass(&a, &all));
    }

    // The shapes are copied from the eval export self-check and must change
    // together with it. Two differ on purpose, by the owner's P3 rule: `p3` (a
    // P3-only review is clean) and `tfind` (the export's task-level finding is
    // "P3", here "P2", because a P3 registry finding does not block).
    #[test]
    fn first_pass_matches_the_eval_export_shapes() {
        let plan = || attempt(Stage::Plan, AttemptStatus::Passed, None, 1);
        let bad = || attempt(Stage::Implement, AttemptStatus::Failed, None, 2);
        let mut tasks: Vec<Task> = Vec::new();
        let mut add = |name: &str, status: TaskStatus, parent: Option<&str>, a: Vec<Attempt>| {
            let mut t = task_with_status(status);
            let d = t.fixture_data();
            d.id = name.into();
            d.parent = parent.map(String::from);
            d.attempts = a;
            tasks.push(t);
        };
        use TaskStatus::{Done, Queued, Stopped};
        add("ok", Done, None, vec![plan(), passed()]);
        add("twice", Done, None, vec![bad(), passed()]);
        add("review", Done, None, vec![passed()]);
        add("p3", Done, None, vec![passed()]);
        add("tfind", Done, None, vec![passed()]);
        add("notdone", Stopped, None, vec![passed()]);
        add("firstfail", Done, None, vec![bad()]);
        add("p_ok", Done, None, vec![plan()]);
        add("p_ok_a", Done, Some("p_ok"), vec![passed()]);
        add("p_ok_b", Done, Some("p_ok"), vec![passed()]);
        add("p_bad", Done, None, vec![]);
        add("p_bad_a", Done, Some("p_bad"), vec![passed()]);
        add("p_bad_b", Done, Some("p_bad"), vec![bad(), passed()]);
        add("p_own", Done, None, vec![passed()]);
        add("p_own_a", Done, Some("p_own"), vec![passed()]);
        add("p_open", Done, None, vec![]);
        add("p_open_a", Queued, Some("p_open"), vec![]);
        add("orphan", Done, Some("gone"), vec![passed()]);
        for t in &mut tasks {
            match t.id.as_str() {
                "review" => {
                    t.fixture_data().attempts[0].review =
                        Some(review_with(&["P2: x"], vec![Some(2)]));
                }
                "p3" => {
                    t.fixture_data().attempts[0].review =
                        Some(review_with(&["P3: x"], vec![Some(3)]));
                }
                "tfind" => t.fixture_data().findings = vec![finding("P2: x")],
                _ => {}
            }
        }
        let got: std::collections::BTreeSet<&str> = tasks
            .iter()
            .filter(|t| first_pass(t, &tasks))
            .map(|t| t.id.as_str())
            .collect();
        let want: std::collections::BTreeSet<&str> = [
            "ok", "p3", "p_ok", "p_ok_a", "p_ok_b", "p_bad_a", "p_own_a", "orphan",
        ]
        .into_iter()
        .collect();
        assert_eq!(got, want);
    }

    fn record(task: &str, stage: &str, usd: f64) -> CostRecord {
        serde_json::from_value(json!({
            "ts": 1, "repo": "/repo", "taskId": task, "stage": stage,
            "routeId": "r", "harness": "claude", "model": "m", "costUsd": usd,
        }))
        .unwrap()
    }

    #[test]
    fn cost_sums_the_task_and_its_subtasks_by_phase() {
        let records = vec![
            record("a", "plan", 0.02),
            record("a", "implement", 0.01),
            record("b", "implement", 0.5),
            record("c", "implement", 1.0),
            record("a", "audit", 9.0),
        ];
        let ids: HashSet<&str> = ["a", "b"].into_iter().collect();
        let by = cost_by_phase(&records, &ids);
        assert_eq!(by.len(), 8);
        assert!((by["plan"] - 0.02).abs() < 1e-9);
        assert!((by["implement"] - 0.51).abs() < 1e-9);
        assert_eq!(by["pick"], 0.0);
    }

    fn metrics(first: bool, cause: Option<FailureCause>, spend: &[(&str, f64)]) -> TaskMetrics {
        let mut cost: BTreeMap<String, f64> =
            COST_PHASES.iter().map(|p| (p.to_string(), 0.0)).collect();
        for (p, usd) in spend {
            cost.insert(p.to_string(), *usd);
        }
        TaskMetrics {
            first_pass: first,
            owner_touches: 2,
            cost_by_phase: cost,
            failure_cause: cause,
            ended_at: 1,
        }
    }

    #[test]
    fn the_entry_text_lists_nonzero_phases_in_fixed_order() {
        let m = metrics(
            true,
            None,
            &[("review", 0.004), ("plan", 0.02), ("implement", 0.014)],
        );
        assert_eq!(
            entry_text(&m),
            "first pass: yes; owner touches: 2; cause: none; cost: plan $0.02, implement $0.01"
        );
        let none = metrics(false, Some(FailureCause::ReviewLoop), &[]);
        assert_eq!(
            entry_text(&none),
            "first pass: no; owner touches: 2; cause: review_loop; cost: $0.00"
        );
    }

    #[test]
    fn the_repo_line_covers_the_last_twenty_top_level_tasks() {
        let mut tasks = Vec::new();
        for i in 0..22 {
            let mut t = task_with_status(if i % 2 == 0 {
                TaskStatus::Done
            } else {
                TaskStatus::Failed
            });
            let mut m = metrics(i % 4 == 0, None, &[("implement", 1.0)]);
            m.ended_at = i;
            t.record_metrics(Some(m));
            tasks.push(t);
        }
        // Two oldest fall out of the window; subtasks never count.
        let mut sub = task_with_status(TaskStatus::Done);
        sub.fixture_data().parent = Some("p".into());
        sub.fixture_data().metrics = Some(metrics(true, None, &[]));
        tasks.push(sub);
        // window = ended_at 2..=21: 10 landed, first pass on 4,8,..,20 = 5.
        assert_eq!(
            repo_line(&tasks),
            "Repo (last 20 finished): first-pass rate 25% (5/20); average cost per landed task $2.00 (10 landed)"
        );
        let mut failed = task_with_status(TaskStatus::Failed);
        failed.fixture_data().metrics = Some(metrics(false, Some(FailureCause::Env), &[]));
        assert!(repo_line(&[failed]).ends_with("average cost per landed task n/a (0 landed)"));
    }
}
