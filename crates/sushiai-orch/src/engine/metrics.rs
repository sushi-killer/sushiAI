//! First-pass metrics of a finished task (`Task.metrics`): computed once when
//! the task loop ends on `done` or `failed`, and summed into one `done` entry
//! of the overseer log for a top-level task. Everything here but
//! [`App::record_task_metrics`] is pure.

use super::*;
use crate::costs::{self, CostRecord};
use std::collections::{BTreeMap, HashSet};

/// How many finished top-level tasks the repo line of the entry covers.
const REPO_WINDOW: usize = 20;

/// A parent's subtasks (the repo's tasks that name it as parent).
fn children<'a>(task: &Task, all: &'a [Task]) -> Vec<&'a Task> {
    all.iter()
        .filter(|t| t.parent.as_deref() == Some(task.id.as_str()))
        .collect()
}

fn owner_answered(task: &Task) -> bool {
    task.question_history
        .iter()
        .any(|q| q.answered_by == AnsweredBy::Owner)
}

fn solo_first_pass(task: &Task) -> bool {
    task.status == TaskStatus::Done && implement_attempt_count(task) == 1 && !owner_answered(task)
}

/// A task is first-pass when it is done with exactly one implement attempt
/// and no question the owner answered. A parent is first-pass when every
/// subtask is and it ran no implement attempt of its own.
pub(super) fn first_pass(task: &Task, children: &[&Task]) -> bool {
    if children.is_empty() {
        return solo_first_pass(task);
    }
    task.status == TaskStatus::Done
        && implement_attempt_count(task) == 0
        && !owner_answered(task)
        && children.iter().all(|c| solo_first_pass(c))
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
        first_pass: first_pass(task, &kids),
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
                let _ = self.update_task(task_id, |t| t.metrics = None);
            }
            return;
        }
        let mut all = self.repo_tasks(&task.repo);
        let metrics = task.metrics.clone().unwrap_or_else(|| {
            let records = costs::read_all(&self.data_dir);
            compute(&task, &all, &records, now_ms())
        });
        if task.parent.is_none() {
            if let Some(current) = all.iter_mut().find(|current| current.id == task.id) {
                current.metrics = Some(metrics.clone());
            }
            let line = entry_text(&metrics);
            let detail = format!("{line}\n{}", repo_line(&all));
            let entry = overseer::Entry::new("done", &task.repo, Some(&task.id), &line)
                .with_detail(&detail);
            if matches!(overseer::append_task_once(&self.data_dir, &entry), Ok(true)) {
                let _ = self.events_tx.send(Event::Overseer {
                    entry: Box::new(entry),
                });
            }
        }
        // Only the metrics are written, under the task lock: the slow work
        // above must not save a stale copy over an owner's mark.
        let _ = self.update_task(task_id, |t| t.metrics = Some(metrics));
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
        t.attempts = vec![attempt(
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
        t.status = TaskStatus::Done;
        assert_eq!(cause(&t), None);
    }

    #[test]
    fn a_base_check_failure_is_env() {
        let mut t = failed_with(FailureKind::Verify);
        t.attempts[0].failure.as_mut().unwrap().signature = "base:npm test".into();
        assert_eq!(cause(&t), Some(FailureCause::Env));
    }

    #[test]
    fn env_questions_are_env_and_the_latest_signal_wins() {
        let mut t = failed_with(FailureKind::Review);
        t.question_history.push(AnsweredQuestion {
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
        t.question_history[0].answered_at = 5;
        assert_eq!(cause(&t), Some(FailureCause::ReviewLoop));
        t.question_history[0].kind = QuestionKind::PreexistingFailure;
        t.question_history[0].answered_at = 30;
        assert_eq!(cause(&t), Some(FailureCause::Env));
    }

    #[test]
    fn a_failed_plan_stage_without_implementing_is_plan() {
        let mut t = task_with_status(TaskStatus::Failed);
        t.attempts = vec![attempt(
            Stage::Plan,
            AttemptStatus::Failed,
            Some(FailureKind::Error),
            10,
        )];
        assert_eq!(cause(&t), Some(FailureCause::Plan));
        t.attempts[0].failure = None;
        t.attempts[0].status = AttemptStatus::Interrupted;
        assert_eq!(cause(&t), Some(FailureCause::Plan));
        t.attempts.push(attempt(
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
        t.plan_gate.findings.push(PlanFinding {
            id: "coverage-1".into(),
            severity: Severity::Critical,
            kind: "coverage".into(),
            text: "x".into(),
        });
        assert_eq!(cause(&t), Some(FailureCause::Plan));
        t.attempts[0].failure.as_mut().unwrap().kind = FailureKind::Review;
        assert_eq!(cause(&t), Some(FailureCause::ReviewLoop));
    }

    #[test]
    fn first_pass_needs_one_implement_attempt_and_no_owner_answer() {
        let mut t = task_with_status(TaskStatus::Done);
        t.attempts = vec![attempt(Stage::Implement, AttemptStatus::Passed, None, 10)];
        assert!(first_pass(&t, &[]));
        assert_eq!(owner_touches(&t, &[]), 0);
        t.decisions.push("Amended: verify".into());
        assert_eq!(owner_touches(&t, &[]), 1);
        t.question_history.push(AnsweredQuestion {
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
        assert!(!first_pass(&t, &[]));
        assert_eq!(owner_touches(&t, &[]), 2);
        t.question_history[0].answered_by = AnsweredBy::Policy;
        assert!(first_pass(&t, &[]));
        t.attempts
            .push(attempt(Stage::Implement, AttemptStatus::Passed, None, 11));
        assert!(!first_pass(&t, &[]));
    }

    #[test]
    fn a_parent_is_first_pass_when_every_subtask_is() {
        let mut part = task_with_status(TaskStatus::Done);
        part.attempts = vec![attempt(Stage::Implement, AttemptStatus::Passed, None, 1)];
        let mut other = part.clone();
        let parent = task_with_status(TaskStatus::Done);
        assert!(first_pass(&parent, &[&part, &other]));
        other.attempts.push(part.attempts[0].clone());
        assert!(!first_pass(&parent, &[&part, &other]));
        let mut own = parent.clone();
        own.attempts = part.attempts.clone();
        assert!(!first_pass(&own, &[&part]));
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
            t.metrics = Some(m);
            tasks.push(t);
        }
        // Two oldest fall out of the window; subtasks never count.
        let mut sub = task_with_status(TaskStatus::Done);
        sub.parent = Some("p".into());
        sub.metrics = Some(metrics(true, None, &[]));
        tasks.push(sub);
        // window = ended_at 2..=21: 10 landed, first pass on 4,8,..,20 = 5.
        assert_eq!(
            repo_line(&tasks),
            "Repo (last 20 finished): first-pass rate 25% (5/20); average cost per landed task $2.00 (10 landed)"
        );
        let mut failed = task_with_status(TaskStatus::Failed);
        failed.metrics = Some(metrics(false, Some(FailureCause::Env), &[]));
        assert!(repo_line(&[failed]).ends_with("average cost per landed task n/a (0 landed)"));
    }
}
