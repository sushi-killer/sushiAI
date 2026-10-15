mod requirements;

use crate::engine::plan_gate;
use crate::engine::planning::publication::{publish, PlanProposal, Publication};
use crate::engine::planning::records::end_plan_stage;

use crate::engine::planning::round::PlannerRound;
use crate::engine::planning::run::PlanOutcome;
impl PlannerRound {
    pub(in crate::engine::planning) async fn review_plan(
        &mut self,
        proposal: PlanProposal,
        signals: &crate::engine::run_control::TaskSignals,
    ) -> Result<Option<crate::engine::planning::draft::PlanDraft>, PlanOutcome> {
        let draft =
            match publish(&self.app, &mut self.task, proposal, &self.settings, signals).await {
                Ok(Publication::Installed(draft)) => *draft,
                Ok(Publication::Retry) => return Ok(None),
                Ok(Publication::Ended) => {
                    return Err(end_plan_stage(&self.app, &self.task.id, Some(self.index)).await)
                }
                Err(_) => return Err(PlanOutcome::Ended),
            };
        let Some(draft) = self.check_requirements(draft, signals).await? else {
            return Ok(None);
        };
        let app = &self.app;
        let task_id = self.task.id.as_str();
        let idx = self.index;
        let attempt_n = self.task.attempts[idx].n;
        let cancel = &signals.cancel;
        // The plan gate: one typed plan review, then the code checks. A
        // critical finding replans once.
        match plan_gate::run(app, task_id, attempt_n, &draft, cancel).await {
            Ok(plan_gate::GateAction::Replan | plan_gate::GateAction::Changed) => return Ok(None),
            Ok(plan_gate::GateAction::Cancelled) => {
                return Err(end_plan_stage(app, task_id, Some(idx)).await);
            }
            Ok(plan_gate::GateAction::Proceed) => {}
            Err(error) => {
                eprintln!("orch: plan gate {task_id}: {error}");
                return Err(end_plan_stage(app, task_id, Some(idx)).await);
            }
        }
        if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
            self.task = reloaded;
        }

        Ok(Some(draft))
    }
}
