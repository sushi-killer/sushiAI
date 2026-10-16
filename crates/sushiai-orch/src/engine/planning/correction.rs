use crate::clock::now_ms;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::{run_harness, RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::decision::record_failure;
use crate::engine::planning::draft::PlanDraft;
use crate::engine::planning::publication::{publish, PlanProposal, Publication};
use crate::engine::planning::records::write_planner_run_files;
use crate::engine::review::native::review_request;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::harness;
use crate::harness::guard::helper_timeout_message;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::FailureKind;
use crate::task::Stage;
use crate::task::Task;
use std::path::PathBuf;
use std::sync::Arc;

/// Why a brief-check correction produced no draft.
pub(in crate::engine) enum Uncorrected {
    /// Draft from scratch instead; the reason is for the decision line.
    Fresh(String),
    /// The task was cancelled; the plan attempt at this index is open.
    Cancelled(usize),
    Failed,
    Superseded,
    Ended,
}

/// One admitted continuation of the planner session that wrote a contradictory draft.
pub(in crate::engine) struct PlannerCorrection<'a> {
    task: Task,
    index: usize,
    route: &'a Route,
    session: String,
    based_on: CheckSnapshot,
    limit: std::time::Duration,
}

impl<'a> PlannerCorrection<'a> {
    pub(in crate::engine) fn resume(
        app: &Arc<App>,
        task_id: &str,
        source: usize,
        route: &'a Route,
    ) -> Result<Self, Uncorrected> {
        let fresh = |why: &str| Uncorrected::Fresh(why.to_string());
        let lock = app.task_lock(task_id);
        let guard = lock.lock().unwrap();
        let Ok(Some(mut task)) = app.store.load_task(task_id) else {
            return Err(fresh("the task could not be read"));
        };
        if task.status != crate::task::TaskStatus::Drafting
            || task.archived
            || task.queue.park.is_some()
        {
            return Err(Uncorrected::Ended);
        }
        if task.pending_amendment.is_some() || task.question.is_some() {
            return Err(Uncorrected::Superseded);
        }
        let Some(session) = task.attempts.get(source).and_then(|a| a.session_id.clone()) else {
            return Err(fresh("the planner attempt has no session id"));
        };
        if task
            .cost_budget()
            .is_some_and(|budget| task.cost_usd >= budget)
        {
            return Err(fresh("the cost budget is reached"));
        }
        let index = crate::task::attempts::start(
            &mut task,
            Stage::Plan,
            route,
            "brief correction",
            now_ms(),
        );
        app.store.save_task(&task).map_err(|_| Uncorrected::Ended)?;
        drop(guard);
        app.broadcast_task(&task);
        Ok(Self {
            based_on: CheckSnapshot::capture(&task),
            task,
            index,
            route,
            session,
            limit: app.planner_timeout(),
        })
    }

    pub(in crate::engine) async fn run(
        mut self,
        app: &Arc<App>,
        conflict: &str,
        signals: &TaskSignals,
    ) -> Result<PlanDraft, Uncorrected> {
        let settings = app.settings.read().unwrap().clone();
        let result = self.launch(app, &settings, conflict, &signals.cancel).await;
        self.accept(app, &settings, result, signals).await
    }

    async fn launch(
        &self,
        app: &Arc<App>,
        settings: &Settings,
        conflict: &str,
        cancel: &crate::harness::cancel::CancelToken,
    ) -> Result<harness::RunOutcome, RunError> {
        let worktree = PathBuf::from(&self.task.worktree);
        let attempt_n = self.task.attempts[self.index].n;
        let run_dir = app.store.run_dir(&self.task.id, attempt_n).join("plan");
        let _ = std::fs::create_dir_all(&run_dir);
        let (mcp_path, settings_path, key_path) =
            write_planner_run_files(app, self.route, settings, &run_dir);
        let request = harness::RunRequest {
            resume: Some(&self.session),
            ..review_request(self.route, &worktree, &mcp_path, &settings_path, &[])
        };
        let brief = crate::engine::planning::prompt::build_plan_correction_brief(
            conflict,
            &self.task.variant(),
            self.task.parent.is_none(),
        );
        let _ = std::fs::write(run_dir.join("brief.md"), &brief);
        let result = run_harness(
            app,
            RunIdentity {
                id: &self.task.id,
                attempt_n,
                track: RunTrack::Attempt,
                tag: CostTag::task("plan", &self.route.id),
            },
            RunInvocation {
                request: &request,
                brief: &brief,
                events: &run_dir.join("events.jsonl"),
                schema: None,
            },
            cancel,
            Guard::helper("planner", self.limit),
        )
        .await;
        let _ = std::fs::remove_file(&key_path);
        result
    }

    async fn accept(
        &mut self,
        app: &Arc<App>,
        settings: &Settings,
        result: Result<harness::RunOutcome, RunError>,
        signals: &TaskSignals,
    ) -> Result<PlanDraft, Uncorrected> {
        let cancel = &signals.cancel;
        if let Ok(Some(reloaded)) = app.store.load_task(&self.task.id) {
            self.task = reloaded;
        }
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(RunError::Cancelled) => return Err(Uncorrected::Cancelled(self.index)),
            Err(RunError::Io(message) | RunError::NotFound(message)) => {
                if cancel.is_cancelled() || self.task.status != crate::task::TaskStatus::Drafting {
                    return Err(Uncorrected::Cancelled(self.index));
                }
                return correction_failed(
                    app,
                    &mut self.task,
                    self.index,
                    FailureKind::Error,
                    format!("the resumed run could not start: {message}"),
                );
            }
        };
        let observation = correction_observation(self.route.harness, &outcome, &settings.prices);
        self.task = app
            .update_task(&self.task.id, |task| {
                crate::task::attempts::record_plan_run(task, self.index, observation);
            })
            .map_err(|_| Uncorrected::Ended)?;
        if cancel.is_cancelled() || self.task.status != crate::task::TaskStatus::Drafting {
            return Err(Uncorrected::Cancelled(self.index));
        }
        if outcome.timed_out {
            return Err(settle_correction_timeout(
                app,
                &mut self.task,
                self.index,
                outcome.error.clone(),
                self.limit,
            ));
        }
        let draft = match correction_reply(outcome) {
            Ok(draft) => draft,
            Err((kind, why)) => {
                return correction_failed(app, &mut self.task, self.index, kind, why)
            }
        };
        let proposal = PlanProposal {
            based_on: self.based_on.clone(),
            draft,
            attempt: self.index,
        };
        match publish(app, &mut self.task, proposal, settings, signals).await {
            Ok(Publication::Installed(draft)) => Ok(*draft),
            Ok(Publication::Retry) => Err(Uncorrected::Superseded),
            Ok(Publication::Ended)
                if self.task.attempts[self.index].status == crate::task::AttemptStatus::Running =>
            {
                Err(Uncorrected::Cancelled(self.index))
            }
            Ok(Publication::Ended) | Err(_) => Err(Uncorrected::Ended),
        }
    }
}

fn settle_correction_timeout(
    app: &Arc<App>,
    task: &mut Task,
    at: usize,
    error: Option<String>,
    limit: std::time::Duration,
) -> Uncorrected {
    task.record_decision(helper_timeout_message("planner", limit));
    record_failure(task, at, FailureKind::Error, error.unwrap_or_default());
    task.mark_failed();
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    Uncorrected::Failed
}

fn correction_failed(
    app: &Arc<App>,
    task: &mut Task,
    at: usize,
    kind: FailureKind,
    why: String,
) -> Result<crate::engine::planning::draft::PlanDraft, Uncorrected> {
    record_failure(task, at, kind, why.clone());
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    Err(Uncorrected::Fresh(why))
}

fn correction_reply(
    outcome: harness::RunOutcome,
) -> Result<crate::engine::planning::draft::PlanDraft, (FailureKind, String)> {
    if let Some(error) = outcome.error {
        return Err((
            FailureKind::Error,
            format!("the resumed run failed ({error})"),
        ));
    }
    crate::engine::planning::draft::parse_plan(&outcome.final_text.unwrap_or_default()).ok_or_else(
        || {
            (
                FailureKind::NoDeliverable,
                "the resumed reply had no valid sushi-plan block".to_string(),
            )
        },
    )
}
fn correction_observation(
    harness: Harness,
    outcome: &harness::RunOutcome,
    prices: &std::collections::BTreeMap<String, crate::policy::pricing::Price>,
) -> crate::task::attempts::PlanRunObservation {
    // A resumed Claude session's own total also covers the run before it.
    let own = match harness {
        Harness::Claude => outcome.streamed_cost(prices),
        Harness::Codex => None,
    };
    crate::task::attempts::PlanRunObservation {
        cost: own.or(outcome.cost_usd),
        estimated: outcome.cost_estimated,
        fingerprint: outcome.fingerprint.clone(),
        usage: None,
    }
}

#[cfg(test)]
mod tests;
