use super::{ReviewEnd, ReviewRound};
use crate::clock::now_ms;
use crate::engine::cost::wait_while_over_budget;
use crate::engine::lifecycle::interruption::interrupt_attempt;
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::review::routing::select_review_route_with;
use crate::engine::state::App;
use crate::engine::verification::healing::preflight::settle_base_preflight;
use crate::policy::routing::Route;
use crate::task::attempts::{end, AttemptEnd};
use crate::task::state::Task;
use std::sync::Arc;

impl ReviewRound {
    pub(super) async fn select_route(
        &self,
        app: &Arc<App>,
        task: &mut Task,
        slot: &mut QuestionSlot<'_>,
    ) -> Result<Option<Route>, ReviewEnd> {
        let settings = &self.run.settings;
        let review_id = task.variant().review_route_id(settings).to_string();
        if review_id.is_empty() {
            return Ok(None);
        }
        let selected = select_review_route_with(settings, &review_id, &self.run.route, task.tier)
            .and_then(|(route, why)| {
                app.usable_route(settings, route.clone())
                    .map(|(route, swap)| (route, why, swap))
            });
        let Some((route, why, swap)) = selected else {
            let note =
                format!("Orchestrator: review skipped, no route matches review \"{review_id}\"");
            if !task.decisions.contains(&note) {
                task.record_decision(note);
            }
            return Ok(None);
        };
        let note = format!("Orchestrator: review: {} ({why})", route.id);
        for line in swap.into_iter().chain([note]) {
            if !task.decisions.contains(&line) {
                task.record_decision(line);
            }
        }
        match wait_while_over_budget(app, task, "review", slot).await {
            Ok(()) => {}
            Err(WaitEnd::Changed) => return Err(ReviewEnd::Changed),
            Err(WaitEnd::Stopped) => {
                if let Ok(Some(mut fresh)) = app.store.load_task(&task.id) {
                    if app.leaves_run_going(&fresh, self.run.idx) {
                        return Err(ReviewEnd::Stopped);
                    }
                    if self.run.idx < fresh.attempts.len() {
                        end(&mut fresh, self.run.idx, AttemptEnd::Interrupted, now_ms());
                    }
                    fresh.touch(now_ms());
                    app.save_task_logged(&fresh);
                    app.broadcast_task(&fresh);
                }
                return Err(ReviewEnd::Stopped);
            }
        }
        // Final-command preflight must settle before background checks clone it.
        match settle_base_preflight(app, task, slot.cancel()).await {
            Ok(()) => {}
            Err(WaitEnd::Changed) => return Err(ReviewEnd::Changed),
            Err(WaitEnd::Stopped) => {
                interrupt_attempt(app, task, self.run.idx);
                return Err(ReviewEnd::Stopped);
            }
        }
        Ok(Some(route))
    }
}
