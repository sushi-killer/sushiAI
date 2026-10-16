use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::prepare::{ImplementRun, PreparedRun};
use crate::engine::attempts::resume::RunBrief;
use crate::engine::attempts::run::{run_harness, RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::Trip;
use crate::harness::native::error::RunError;
use crate::harness::run_files::RunFiles;
use crate::harness::RunOutcome;
use std::sync::Arc;

pub(super) struct ResumeFallback<'a> {
    pub(super) run: ImplementRun<'a>,
    pub(super) prepared: &'a PreparedRun,
    pub(super) brief: &'a RunBrief,
}

impl ResumeFallback<'_> {
    pub(super) async fn run(
        self,
        app: &Arc<App>,
        mut previous: Result<RunOutcome, RunError>,
        cancel: &CancelToken,
    ) -> Result<RunOutcome, RunError> {
        let spent = previous
            .as_ref()
            .ok()
            .and_then(|outcome| outcome.cost_usd)
            .unwrap_or(0.0);
        let mut request = self.run.request(self.prepared, None);
        request.max_budget_usd = request.max_budget_usd.map(|cap| (cap - spent).max(0.0));
        if previous.as_ref().is_ok_and(|outcome| outcome.over_budget) {
            return previous;
        }
        if request.max_budget_usd == Some(0.0) {
            if let Ok(outcome) = &mut previous {
                Trip::OverBudget(format!(
                    "The attempt spent about ${spent:.4}; no budget remains for a fresh run."
                ))
                .apply(outcome);
            }
            return previous;
        }
        if let Err(error) = self.preserve_rejected_run() {
            return match previous {
                Ok(mut outcome) => {
                    outcome.error = Some(error);
                    Ok(outcome)
                }
                Err(_) => Err(RunError::Io(error)),
            };
        }
        let _ = std::fs::write(self.run.run_dir.join("brief.md"), &self.brief.full);
        if let Some(account) = self
            .run
            .route
            .account_id
            .as_ref()
            .and_then(|id| app.secrets.read().unwrap().accounts.get(id).cloned())
        {
            if account.kind == "subscription" {
                let _ = crate::store::files::write_secret_file(
                    &self.run.run_dir.join("oauth-token"),
                    &account.value,
                );
            }
        }
        let events = self.run.run_dir.join("events.jsonl");
        run_harness(
            app,
            RunIdentity {
                id: &self.run.task.id,
                attempt_n: self.run.attempt_n,
                track: RunTrack::Resumable,
                tag: CostTag::task("implement", &self.run.route.id),
            },
            RunInvocation {
                request: &request,
                brief: &self.brief.full,
                events: &events,
                schema: None,
            },
            cancel,
            self.run
                .guard(self.prepared)
                .with_cap(request.max_budget_usd),
        )
        .await
    }

    fn preserve_rejected_run(&self) -> Result<(), String> {
        let metadata = RunFiles::of(&self.run.run_dir.join("events.jsonl")).meta;
        if metadata.exists() {
            std::fs::copy(
                &metadata,
                RunFiles::of(&self.run.run_dir.join("resume-rejected.events.jsonl")).meta,
            )
            .map_err(|error| format!("saving rejected resume metadata: {error}"))?;
        }
        let events = self.run.run_dir.join("events.jsonl");
        let raw = RunFiles::of(&events).raw;
        if raw.exists() {
            std::fs::rename(
                &raw,
                RunFiles::of(&self.run.run_dir.join("resume-rejected.events.jsonl")).raw,
            )
            .map_err(|error| format!("saving rejected resume raw output: {error}"))?;
        }
        if events.exists() {
            std::fs::rename(
                &events,
                self.run.run_dir.join("resume-rejected.events.jsonl"),
            )
            .map_err(|error| format!("saving rejected resume events: {error}"))?;
        }
        Ok(())
    }
}
