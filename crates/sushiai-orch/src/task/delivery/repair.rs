use crate::task::{AttemptStatus, Failure, Task};
use serde::{Deserialize, Serialize};

/// The one ordinary agent step granted for a persistent post-carry check failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrCheckRepair {
    pub cause: String,
    pub step: u32,
    pub failure: Failure,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub completed_causes: Vec<String>,
}

impl PartialEq for PrCheckRepair {
    fn eq(&self, other: &Self) -> bool {
        self.cause == other.cause
            && self.step == other.step
            && self.failure.kind == other.failure.kind
            && self.failure.detail == other.failure.detail
            && self.failure.signature == other.failure.signature
            && self.completed_causes == other.completed_causes
    }
}

impl Task {
    /// A completed repair of this exact cause goes to the owner; interrupted
    /// work retains its owed step, and a different cause gets its own step.
    pub(crate) fn schedule_pr_check_repair(&mut self, cause: String, failure: Failure) -> bool {
        let mut previous = self.data.queue.pr_check_repair.take();
        if let Some(repair) = previous.as_mut() {
            let completed = self.data.attempts.iter().any(|attempt| {
                attempt.n == repair.step
                    && !matches!(
                        attempt.status,
                        AttemptStatus::Running | AttemptStatus::Interrupted
                    )
            });
            if completed && !repair.completed_causes.contains(&repair.cause) {
                repair.completed_causes.push(repair.cause.clone());
            }
        }
        let already_completed = previous
            .as_ref()
            .is_some_and(|repair| repair.completed_causes.contains(&cause));
        let step = match previous.as_ref() {
            Some(repair) if repair.cause == cause || already_completed => repair.step,
            _ => crate::task::attempts::next_number(&self.data),
        };
        self.data.queue.pr_check_repair = Some(PrCheckRepair {
            cause,
            step,
            failure,
            completed_causes: previous.map_or_else(Vec::new, |repair| repair.completed_causes),
        });
        !already_completed
    }

    pub(crate) fn record_pr_check_repair_step(&mut self, number: u32) {
        // Ordinary retry admission owns the attempt budget. A native timeout
        // continuation still owes the repair's check settlement.
        let timed_out = self.queue.pr_check_repair.as_ref().is_some_and(|repair| {
            self.attempts.iter().any(|attempt| {
                attempt.n == repair.step
                    && attempt
                        .failure
                        .as_ref()
                        .is_some_and(|failure| failure.kind == crate::task::FailureKind::TimeBudget)
            })
        });
        if self.pr_check_repair_due() || timed_out {
            self.data.queue.pr_check_repair.as_mut().unwrap().step = number;
        }
    }

    pub(crate) fn authorize_pr_check_retry(&mut self) -> bool {
        let step = self
            .data
            .attempts
            .iter()
            .map(|attempt| attempt.n)
            .max()
            .unwrap_or(0)
            + 1;
        let Some(repair) = self.data.queue.pr_check_repair.as_mut() else {
            return false;
        };
        repair.step = step;
        self.data.pr_pending = true;
        true
    }

    pub(crate) fn clear_pr_check_repair(&mut self) {
        self.data.queue.pr_check_repair = None;
    }

    pub(crate) fn pr_check_repair_due(&self) -> bool {
        self.queue.pr_check_repair.as_ref().is_some_and(|repair| {
            !self.attempts.iter().any(|attempt| {
                attempt.n == repair.step
                    && !matches!(
                        attempt.status,
                        AttemptStatus::Running | AttemptStatus::Interrupted
                    )
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::test_support::sample_task;
    use crate::task::{FailureKind, TaskStatus};

    fn failure(detail: &str) -> Failure {
        Failure {
            kind: FailureKind::Verify,
            detail: detail.into(),
            signature: detail.into(),
        }
    }

    #[test]
    fn a_same_cause_gets_one_completed_step_and_a_different_cause_gets_its_own() {
        let mut task = sample_task("repair", TaskStatus::Done, AttemptStatus::Passed);
        let original = task.attempts[0].clone();
        assert!(task.schedule_pr_check_repair("case-1".into(), failure("test one")));
        let step = task.queue.pr_check_repair.as_ref().unwrap().step;
        assert_eq!(step, original.n + 1);
        assert!(task.pr_check_repair_due());
        assert!(task.schedule_pr_check_repair("case-1".into(), failure("same, new output")));
        let mut repair = original;
        repair.n = step;
        repair.status = AttemptStatus::Interrupted;
        task.fixture_data().attempts.push(repair);
        assert!(task.schedule_pr_check_repair("case-1".into(), failure("interrupted")));
        task.fixture_data().attempts.last_mut().unwrap().status = AttemptStatus::Failed;
        assert!(!task.pr_check_repair_due());
        assert!(!task.schedule_pr_check_repair("case-1".into(), failure("test one remains")));
        assert_eq!(
            task.queue.pr_check_repair.as_ref().unwrap().failure.detail,
            "test one remains"
        );
        assert!(task.schedule_pr_check_repair("case-2".into(), failure("different test")));
        assert_eq!(task.queue.pr_check_repair.as_ref().unwrap().step, step + 1);
    }

    #[test]
    fn an_earlier_completed_cause_is_not_granted_again_after_another_failure() {
        let mut task = sample_task("repair", TaskStatus::Running, AttemptStatus::Passed);
        let route = crate::policy::settings::Settings::default().routes[0].clone();
        for cause in ["case-a", "case-b"] {
            assert!(task.schedule_pr_check_repair(cause.into(), failure(cause)));
            let index = crate::task::attempts::start(
                &mut task,
                crate::task::Stage::Implement,
                &route,
                "repair",
                1,
            );
            crate::task::attempts::end(
                &mut task,
                index,
                crate::task::attempts::AttemptEnd::Failed(failure(cause)),
                2,
            );
        }
        let mut task: Task = serde_json::from_value(serde_json::to_value(task).unwrap()).unwrap();
        assert!(!task.schedule_pr_check_repair("case-a".into(), failure("case-a returned")));
        assert!(!task.pr_check_repair_due());
        assert_eq!(
            task.queue
                .pr_check_repair
                .as_ref()
                .unwrap()
                .completed_causes,
            ["case-a", "case-b"]
        );
        assert!(task.authorize_pr_check_retry());
        assert!(task.pr_check_repair_due());
        let index = crate::task::attempts::start(
            &mut task,
            crate::task::Stage::Implement,
            &route,
            "owner retry",
            3,
        );
        crate::task::attempts::end(
            &mut task,
            index,
            crate::task::attempts::AttemptEnd::Failed(failure("case-a")),
            4,
        );
        assert!(!task.schedule_pr_check_repair("case-a".into(), failure("case-a persists")));
    }

    #[test]
    fn progress_roundtrips_without_changing_existing_pending_plan() {
        let mut task = sample_task("repair", TaskStatus::Done, AttemptStatus::Passed);
        task.record_pr_pending(true);
        task.schedule_pr_check_repair("cause".into(), failure("command + test + output"));
        let encoded = serde_json::to_value(&task).unwrap();
        let restored: Task = serde_json::from_value(encoded).unwrap();
        assert!(restored.pr_pending);
        assert_eq!(
            restored
                .queue
                .pr_check_repair
                .as_ref()
                .unwrap()
                .failure
                .detail,
            "command + test + output"
        );
        task.clear_pr_check_repair();
        assert!(task.queue.pr_check_repair.is_none());
        assert!(task.pr_pending);
    }
}
