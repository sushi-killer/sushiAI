use crate::clock::now_ms;
use crate::engine::brief_check::check_brief;
use crate::engine::brief_check::redraft_request_note;
use crate::engine::brief_check::BriefAction;
use crate::engine::plan_gate;
use crate::engine::planning::correction::correct_draft;
use crate::engine::planning::correction::Uncorrected;
use crate::engine::planning::publication::{publish, PlanProposal, Publication};
use crate::engine::planning::records::end_plan_stage;
use crate::engine::verification::healing::preflight::start_base_checks;
use std::path::PathBuf;

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
        let app = &self.app;
        let task_id = self.task.id.clone();
        let task_id = task_id.as_str();
        let idx = self.index;
        let attempt_n = self.task.attempts[idx].n;
        let _worktree = PathBuf::from(&self.task.worktree);
        let route = &self.route;
        let cancel = &signals.cancel;
        let task = &mut self.task;
        // The base checks of the draft run beside the brief check. A split
        // parent never implements, so its parts check their own commands.
        if draft.subtasks.is_empty() {
            start_base_checks(app, task, cancel);
        }

        // Do the draft's own requirements contradict each other? One
        // redraft, once per task; the check runs again on the new draft.
        let mut draft = draft;
        match check_brief(app, task_id, true, cancel).await {
            BriefAction::Redraft(conflict) => {
                // The planner's own session fixes the contradiction when it
                // can; otherwise a fresh draft reads the request again.
                match correct_draft(app, task_id, idx, route, &conflict, signals).await {
                    Ok(corrected) => {
                        draft = corrected;
                        // The task has had its one redraft: a second
                        // conflict is kept for the implement brief.
                        if let BriefAction::Cancelled =
                            check_brief(app, task_id, true, cancel).await
                        {
                            return Err(end_plan_stage(app, task_id, Some(idx)).await);
                        }
                    }
                    Err(Uncorrected::Superseded) => return Ok(None),
                    Err(Uncorrected::Ended) => return Err(PlanOutcome::Ended),
                    Err(Uncorrected::Failed) => {
                        return Err(end_plan_stage(app, task_id, None).await)
                    }
                    Err(Uncorrected::Cancelled(at)) => {
                        return Err(end_plan_stage(app, task_id, Some(at)).await);
                    }
                    Err(Uncorrected::Fresh(why)) => {
                        if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                            let request = t.request.clone().unwrap_or_default();
                            t.record_request(format!(
                                "{request}\n\n{}",
                                redraft_request_note(&conflict)
                            ));
                            t.record_decision(format!(
                                "Brief check: the planner session was not resumed ({why}); drafting from scratch"
                            ));
                            t.touch(now_ms());
                            app.save_task_logged(&t);
                            app.broadcast_task(&t);
                        }
                        return Ok(None);
                    }
                }
            }
            BriefAction::Cancelled => return Err(end_plan_stage(app, task_id, Some(idx)).await),
            BriefAction::Proceed => {}
        }
        // A redraft or a rewrite of the brief check starts the jobs again for
        // the commands it changed.
        if draft.subtasks.is_empty() {
            if let Ok(Some(mut rewritten)) = app.store.load_task(task_id) {
                start_base_checks(app, &mut rewritten, cancel);
            }
        }

        // The plan gate: one typed plan review, then the code checks. A
        // critical finding replans once.
        match plan_gate::run(app, task_id, attempt_n, &draft, cancel).await {
            plan_gate::GateAction::Replan => return Ok(None),
            plan_gate::GateAction::Cancelled => {
                return Err(end_plan_stage(app, task_id, Some(idx)).await);
            }
            plan_gate::GateAction::Proceed => {}
        }
        if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
            *task = reloaded;
        }

        Ok(Some(draft))
    }
}
