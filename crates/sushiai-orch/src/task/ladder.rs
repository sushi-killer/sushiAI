use crate::policy::routing::Tier;
use crate::task::{Task, TaskStatus, UnreviewedPart};

impl Task {
    pub(crate) fn change_approach(&mut self, tier: Option<Tier>, attempt: u32, decision: String) {
        if let Some(tier) = tier {
            self.data.tier = tier;
            self.data.tier_fallback = None;
        }
        self.data.loop_state.approach_changed_after = Some(attempt);
        self.data.decisions.push(decision);
    }
    pub(crate) fn inherit_unreviewed_part(&mut self, part: UnreviewedPart, decision: String) {
        self.data.unreviewed = true;
        if !self
            .loop_state
            .unreviewed_parts
            .iter()
            .any(|p| p.id == part.id)
        {
            self.data.loop_state.unreviewed_parts.push(part);
            self.data.decisions.push(decision);
        }
    }
    pub(crate) fn record_agent_restart(&mut self, decision: String) {
        self.data.loop_state.agent_restarted = true;
        self.data.decisions.push(decision);
    }
    pub(crate) fn accept_unreviewed_attempt(&mut self, attempt: u32, decision: String, now: i64) {
        self.data.question = None;
        self.data.unreviewed = true;
        self.data.loop_state.unreviewed_attempt = Some(attempt);
        self.data.decisions.push(decision);
        self.data.status = TaskStatus::Queued;
        self.data.updated_at = now;
    }
}
