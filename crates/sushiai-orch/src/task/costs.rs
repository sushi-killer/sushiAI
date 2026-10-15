use crate::task::{Task, Usage};

#[derive(Clone, Copy)]
pub(crate) enum HelperStage {
    Review,
    Advisor,
}

pub(crate) fn add_attempt_spend(
    task: &mut Task,
    index: usize,
    cost: f64,
    estimated: bool,
    usage: Usage,
) {
    let task = &mut task.data;
    let attempt = &mut task.attempts[index];
    attempt.cost_usd = Some(attempt.cost_usd.unwrap_or(0.0) + cost);
    attempt.cost_estimated |= estimated;
    let saved = attempt.usage.get_or_insert(Usage {
        input: 0,
        cached: 0,
        output: 0,
    });
    saved.input += usage.input;
    saved.cached += usage.cached;
    saved.output += usage.output;
    task.cost_usd += cost;
}

pub(crate) fn charge_helper(task: &mut Task, index: usize, stage: HelperStage, cost: f64) {
    let task = &mut task.data;
    let attempt = &mut task.attempts[index];
    let saved = match stage {
        HelperStage::Review => &mut attempt.review_cost_usd,
        HelperStage::Advisor => &mut attempt.advisor_cost_usd,
    };
    *saved = Some(saved.unwrap_or(0.0) + cost);
    task.cost_usd += cost;
}

pub(crate) fn settle_helper(task: &mut Task, index: usize, stage: HelperStage, cost: f64) -> bool {
    let attempt = &task.attempts[index];
    let saved = match stage {
        HelperStage::Review => attempt.review_cost_usd,
        HelperStage::Advisor => attempt.advisor_cost_usd,
    };
    if saved.is_some() {
        return false;
    }
    charge_helper(task, index, stage, cost);
    true
}

impl Task {
    pub(crate) fn charge(&mut self, cost: f64) {
        self.data.cost_usd += cost;
    }
    pub(crate) fn raise_budget(&mut self) {
        self.data.budget_raises += 1;
    }
    pub(crate) fn allow_daily_budget(&mut self, day: String) {
        self.data.daily_budget_ok_day = Some(day);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::test_support::sample_task;
    use crate::task::{AttemptStatus, TaskStatus};

    #[test]
    fn replayed_attempt_spend_accumulates_tokens_and_helper_settlement_is_once() {
        let mut task = sample_task("charge", TaskStatus::Running, AttemptStatus::Interrupted);
        add_attempt_spend(
            &mut task,
            0,
            0.3,
            false,
            Usage {
                input: 10,
                cached: 4,
                output: 2,
            },
        );
        add_attempt_spend(
            &mut task,
            0,
            0.2,
            true,
            Usage {
                input: 20,
                cached: 8,
                output: 3,
            },
        );
        assert_eq!(task.attempts[0].cost_usd, Some(0.5));
        assert!(task.attempts[0].cost_estimated);
        let usage = task.attempts[0].usage.as_ref().unwrap();
        assert_eq!((usage.input, usage.cached, usage.output), (30, 12, 5));
        assert!(settle_helper(&mut task, 0, HelperStage::Review, 0.1));
        assert!(!settle_helper(&mut task, 0, HelperStage::Review, 99.0));
        assert!(settle_helper(&mut task, 0, HelperStage::Advisor, 0.2));
        assert_eq!(task.attempts[0].review_cost_usd, Some(0.1));
        assert_eq!(task.attempts[0].advisor_cost_usd, Some(0.2));
        assert!((task.cost_usd - 0.8).abs() < 1e-12);
    }
}
