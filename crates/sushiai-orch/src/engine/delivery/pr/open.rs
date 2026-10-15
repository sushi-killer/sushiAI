use self::carry::carry_admission;
use super::{admission::Admission, prepare::prepare_pr, record::record_publication};
use crate::api::validation::validate_task_id;
use crate::delivery::pr::command::run_cmd;
use crate::delivery::pr::command::GIT_TIMEOUT;
use crate::delivery::pr::error::PrError;
use crate::delivery::pr::error::PrRefusal;
use crate::delivery::pr::in_flight::InFlight;
use crate::delivery::pr::request::PrParams;
use crate::engine::state::App;
use crate::task::delivery::PrPlan;

impl App {
    /// The RPC entry: a branch that needs the newest base is carried off the
    /// request path, so a slow carry never stalls the connection.
    pub(in crate::engine) async fn handle_task_pr(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let p: PrParams = serde_json::from_value(params).map_err(|e| e.to_string())?;
        self.open_pr(p, false).await.map_err(|e| e.to_string())
    }

    /// `inline`: only the in-process pull request flow sets it; it carries
    /// onto the newest base before the push, as part of its own background run.
    pub(in crate::engine) async fn open_pr(
        &self,
        p: PrParams,
        inline: bool,
    ) -> Result<serde_json::Value, PrError> {
        if p.source.as_deref().is_some_and(|s| !s.is_empty()) && p.branch.is_some() {
            return Err(PrRefusal::AgentOwnBranch.into());
        }
        validate_task_id(&self.store, &p.id).map_err(PrRefusal::Invalid)?;
        let _guard = InFlight::take(&p.id)?;
        let (mut task, publication) = prepare_pr(self, &p).await?;
        // The same carry as the automatic pull request: never push a stale base.
        let owner_plan = PrPlan {
            branch: publication.content.branch.clone(),
            title: p.title.clone().unwrap_or_default(),
            body: p.body.clone().unwrap_or_default(),
            branch_from: crate::task::delivery::BranchFrom::Owner,
        };
        run_cmd(
            "git",
            &["fetch", "origin", &publication.base],
            &publication.repo,
            GIT_TIMEOUT,
        )
        .await?;
        let up_to_date = run_cmd(
            "git",
            &[
                "merge-base",
                "--is-ancestor",
                &format!("origin/{}", publication.base),
                &publication.local,
            ],
            &publication.repo,
            GIT_TIMEOUT,
        )
        .await
        .is_ok();
        if !inline && !up_to_date {
            // Stored with the owner's choices; the flow carries, resolves,
            // pushes and opens, then the task shows the pull request.
            let queued = self.update_task(&p.id, move |t| {
                t.record_pr_pending(true);
                t.record_pr_plan(owner_plan);
                t.record_decision(
                    "PR: carrying onto the newest base; the pull request opens when done"
                        .to_string(),
                );
            })?;
            crate::engine::delivery::pr::pending::spawn_open_pr(self, &p.id);
            return Ok(serde_json::to_value(&queued).map_err(|e| e.to_string())?);
        }
        loop {
            let admission =
                carry_admission(&self.arc(), &task, owner_plan.clone(), &publication.base).await?;
            let (fresh, mut next) = prepare_pr(self, &p).await?;
            next.content.body = super::validation::body(self, &fresh, next.content.body).await?;
            let push = next.prepare_push(&fresh.branch).await?;
            let push = match admission.dispatch(self, &p.id, push)? {
                Admission::Ready(write) => write,
                Admission::Changed(current) => {
                    task = *current;
                    continue;
                }
                Admission::Deferred => return Err(PrRefusal::TaskChanged.into()),
            };
            let local_sha = push.finish().await?;
            let open = next.prepare_open(&local_sha).await?;
            let open = match admission.dispatch(self, &p.id, open)? {
                Admission::Ready(write) => write,
                Admission::Changed(current) => {
                    task = *current;
                    continue;
                }
                Admission::Deferred => return Err(PrRefusal::TaskChanged.into()),
            };
            let receipt = open.finish().await?;
            return record_publication(self, &p, &next, receipt);
        }
    }
}

mod carry;

#[cfg(test)]
mod tests;
