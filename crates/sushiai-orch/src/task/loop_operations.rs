use super::{AttemptStatus, EvalCheck, Task};
use crate::policy::routing::Tier;

impl Task {
    pub(crate) fn take_dirty_wait(&mut self) -> Option<i64> {
        self.data.queue.dirty_since.take()
    }

    pub(crate) fn count_rereview(&mut self) {
        self.data.loop_state.rereviews += 1;
    }

    pub(crate) fn clear_unreviewed_attempt(&mut self) {
        self.data.loop_state.unreviewed_attempt = None;
    }

    pub(crate) fn mark_missing_tool_asked(&mut self) {
        self.data.brief_check.missing_tool_asked = true;
        self.data.brief_check.missing_tool = None;
    }

    pub(crate) fn select_tier(&mut self, tier: Tier, fallback: Option<String>) {
        self.data.tier = tier;
        self.data.tier_fallback = fallback;
    }

    pub(crate) fn record_carry_conflict_step(&mut self, attempt: u32) {
        self.data.queue.carry_conflict_step = Some(attempt);
    }

    pub(crate) fn record_discarded_tests(&mut self, files: Vec<String>) {
        self.data.planner_tests.discarded = files;
    }

    pub(crate) fn mark_attempt_blocked(&mut self, index: usize) {
        self.data.attempts[index].status = AttemptStatus::Blocked;
    }

    pub(crate) fn record_carry_conflicts(&mut self, files: Vec<String>) {
        self.data.queue.carry_conflicts = files;
        self.data.queue.carry_conflict_step = None;
    }

    pub(crate) fn settle_carry_conflicts(&mut self) {
        let Some(number) = self.queue.carry_conflict_step else {
            return;
        };
        let ended = self.attempts.iter().any(|attempt| {
            attempt.n == number
                && !matches!(
                    attempt.status,
                    AttemptStatus::Running | AttemptStatus::Interrupted
                )
        });
        if ended {
            self.data.queue.carry_conflicts.clear();
            self.data.queue.carry_conflict_step = None;
        }
    }

    pub(crate) fn record_unreviewed(&mut self, unreviewed: bool) {
        self.data.unreviewed = unreviewed;
    }

    pub(crate) fn record_waited_out_base(&mut self, base: String) {
        self.data.loop_state.waited_out_bases.push(base);
    }

    pub(crate) fn record_eval_check(&mut self, check: EvalCheck) {
        self.data.eval_check = Some(check);
    }

    pub(crate) fn interrupt_running_attempts(&mut self, now: i64) {
        for attempt in &mut self.data.attempts {
            if attempt.status == AttemptStatus::Running {
                attempt.status = AttemptStatus::Interrupted;
                attempt.ended_at = Some(now);
            }
        }
    }

    pub(crate) fn take_new_decisions(&mut self, before: usize) -> Vec<String> {
        self.data.decisions.split_off(before)
    }
}
