use crate::task::{Attempt, CriterionResult, CriterionStatus, Stage, Task};

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
pub(crate) fn criteria_results(
    task: &crate::task::TaskData,
    attempt: &Attempt,
) -> Vec<CriterionResult> {
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
        check_signal(
            held.criterion,
            &super::commands::held_out_label(held.criterion),
        );
    }
    if let Some(review) = &attempt.review {
        for ruling in &review.criteria {
            let Some(i) = ruling.index.filter(|i| *i < signals.len()) else {
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
pub(crate) fn refresh_criteria_results(task: &mut Task, idx: usize) {
    if task.attempts[idx].stage != Stage::Implement {
        return;
    }
    let results = criteria_results(task, &task.attempts[idx]);
    task.data.attempts[idx].criteria_results = results;
}

#[cfg(test)]
#[path = "results/tests.rs"]
mod tests;
