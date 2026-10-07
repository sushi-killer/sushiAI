//! Where each criterion of a task stands after an implement attempt: the
//! grounded checks tied to it and the reviewer's ruling on it, merged.

use super::*;

/// One criterion's evidence from a single source (a check or a ruling).
struct Signal {
    status: CriterionStatus,
    evidence: Option<String>,
}

/// One entry per task criterion. Failing beats met beats pending: a check
/// that exited non-zero in this attempt or a `met: false` ruling fails it,
/// a passing check or a `met: true` ruling meets it when nothing failed it,
/// and everything else (still running, no check or review yet, review off, a
/// `null` ruling) is pending.
pub(super) fn criteria_results(task: &Task, attempt: &Attempt) -> Vec<CriterionResult> {
    let mut signals: Vec<Vec<Signal>> = task.criteria.iter().map(|_| Vec::new()).collect();
    let mut check_signal = |criterion: usize, command: &str| {
        let Some(list) = signals.get_mut(criterion) else {
            return;
        };
        if let Some(v) = attempt.verify.iter().find(|v| v.command == command) {
            let (status, verb) = if v.code == Some(0) {
                (CriterionStatus::Met, "passed")
            } else {
                (CriterionStatus::Failing, "failed")
            };
            list.push(Signal {
                status,
                evidence: Some(format!("check {verb}: {command}")),
            });
        }
    };
    for check in &task.checks {
        check_signal(check.criterion, &check.run);
    }
    if let Some(held) = &task.held_out {
        check_signal(held.criterion, &brief::held_out_label(held.criterion));
    }
    if let Some(review) = &attempt.review {
        for ruling in &review.criteria {
            let Some(i) = criterion_index(task, &ruling.criterion) else {
                continue;
            };
            let status = match ruling.met {
                Some(true) => CriterionStatus::Met,
                Some(false) => CriterionStatus::Failing,
                None => CriterionStatus::Pending,
            };
            signals[i].push(Signal {
                status,
                evidence: ruling.evidence.clone(),
            });
        }
    }
    signals
        .into_iter()
        .enumerate()
        .map(|(i, list)| {
            let winner = [
                CriterionStatus::Failing,
                CriterionStatus::Met,
                CriterionStatus::Pending,
            ]
            .into_iter()
            .find_map(|s| list.iter().find(|x| x.status == s));
            CriterionResult {
                criterion: i,
                text: task.criteria[i].clone(),
                status: winner.map_or(CriterionStatus::Pending, |w| w.status),
                evidence: winner.and_then(|w| w.evidence.clone()),
            }
        })
        .collect()
}

/// Recomputes `criteria_results` of an implement attempt in place.
pub(super) fn refresh_criteria_results(task: &mut Task, idx: usize) {
    if task.attempts[idx].stage != Stage::Implement {
        return;
    }
    let results = criteria_results(task, &task.attempts[idx]);
    task.attempts[idx].criteria_results = results;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_and_attempt() -> (Task, Attempt) {
        let mut task = task_with_status(TaskStatus::Running);
        task.criteria = vec![
            "Alpha works -- check: read it".into(),
            "Beta works".into(),
            "Gamma works".into(),
            "Delta works".into(),
        ];
        let attempt = attempt_with_failure(1, "x");
        (task, attempt)
    }

    fn outcome(command: &str, code: i32) -> VerifyOutcome {
        VerifyOutcome {
            command: command.into(),
            code: Some(code),
            tail: String::new(),
            ms: 1,
        }
    }

    fn ruling(criterion: &str, met: Option<bool>) -> CriterionRuling {
        CriterionRuling {
            criterion: criterion.into(),
            met,
            evidence: Some(format!("because {criterion}")),
        }
    }

    fn statuses(task: &Task, attempt: &Attempt) -> Vec<CriterionStatus> {
        criteria_results(task, attempt)
            .into_iter()
            .map(|r| r.status)
            .collect()
    }

    #[test]
    fn everything_is_pending_before_a_check_or_review_ran() {
        let (task, attempt) = task_and_attempt();
        let results = criteria_results(&task, &attempt);
        assert_eq!(results.len(), 4);
        assert!(results.iter().all(|r| r.status == CriterionStatus::Pending));
        assert_eq!(results[1].text, "Beta works");
        assert_eq!(results[1].criterion, 1);
    }

    #[test]
    fn checks_and_rulings_merge_with_failing_over_met_over_pending() {
        use CriterionStatus::*;
        let (mut task, mut attempt) = task_and_attempt();
        task.checks = vec![
            Check {
                criterion: 0,
                run: "t0".into(),
                baseline: None,
            },
            Check {
                criterion: 1,
                run: "t1".into(),
                baseline: None,
            },
            Check {
                criterion: 2,
                run: "t2".into(),
                baseline: None,
            },
        ];
        task.held_out = Some(Check {
            criterion: 3,
            run: "secret".into(),
            baseline: None,
        });
        attempt.verify = vec![
            outcome("t0", 0),
            outcome("t1", 1),
            outcome("t2", 0),
            outcome(&brief::held_out_label(3), 0),
        ];
        assert_eq!(statuses(&task, &attempt), vec![Met, Failing, Met, Met]);
        // A passing check does not outrank a `met: false` ruling, and a
        // failing check is not rescued by `met: true`.
        attempt.review = Some(ReviewResult {
            verdict: Verdict::Fail,
            findings: vec![],
            repeated: vec![],
            severities: vec![],
            criteria: vec![
                ruling("Alpha works", Some(false)),
                ruling("Beta works", Some(true)),
                ruling("Gamma works", None),
            ],
        });
        let results = criteria_results(&task, &attempt);
        let got: Vec<_> = results.iter().map(|r| r.status).collect();
        assert_eq!(got, vec![Failing, Failing, Met, Met]);
        assert_eq!(results[0].evidence.as_deref(), Some("because Alpha works"));
        assert!(!results[3].evidence.clone().unwrap().contains("secret"));
    }

    #[test]
    fn rulings_alone_decide_and_a_null_ruling_stays_pending() {
        use CriterionStatus::*;
        let (task, mut attempt) = task_and_attempt();
        attempt.review = Some(ReviewResult {
            verdict: Verdict::Pass,
            findings: vec![],
            repeated: vec![],
            severities: vec![],
            criteria: vec![
                ruling("Alpha works -- check: read it", Some(true)),
                ruling("Beta works", None),
                ruling("Gamma works", Some(false)),
                ruling("Not a criterion of this task", Some(false)),
            ],
        });
        assert_eq!(
            statuses(&task, &attempt),
            vec![Met, Pending, Failing, Pending]
        );
    }
}
