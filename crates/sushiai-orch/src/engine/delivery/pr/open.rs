use super::{prepare::prepare_pr, record::record_publication};
use crate::api::validation::validate_task_id;
use crate::delivery::pr::command::run_cmd;
use crate::delivery::pr::command::GIT_TIMEOUT;
use crate::delivery::pr::error::PrError;
use crate::delivery::pr::error::PrRefusal;
use crate::delivery::pr::in_flight::InFlight;
use crate::delivery::pr::publication::{PrContent, Publication};
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
        let (task, publication) = prepare_pr(self, &p).await?;
        let Publication {
            content:
                PrContent {
                    branch,
                    title: _,
                    body: _,
                },
            repo,
            base,
            local,
        } = &publication;
        // The same carry as the automatic pull request: never push a stale base.
        let owner_plan = PrPlan {
            branch: branch.clone(),
            title: p.title.clone().unwrap_or_default(),
            body: p.body.clone().unwrap_or_default(),
            branch_from: crate::task::delivery::BranchFrom::Owner,
        };
        run_cmd("git", &["fetch", "origin", base], repo, GIT_TIMEOUT).await?;
        let up_to_date = run_cmd(
            "git",
            &[
                "merge-base",
                "--is-ancestor",
                &format!("origin/{base}"),
                local,
            ],
            repo,
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
        match crate::engine::delivery::pr::carry::carry_for_pr(&self.arc(), &task, Some(owner_plan))
            .await?
        {
            crate::engine::delivery::pr::carry::PrCarry::Ready => {}
            crate::engine::delivery::pr::carry::PrCarry::Contained => {
                return Err(PrRefusal::Contained(base.clone()).into());
            }
            crate::engine::delivery::pr::carry::PrCarry::Resolving(files) => {
                return Err(PrRefusal::Resolving {
                    base: base.clone(),
                    files,
                }
                .into());
            }
            crate::engine::delivery::pr::carry::PrCarry::Failed(reason) => {
                return Err(PrRefusal::CarryFailed(reason).into());
            }
            crate::engine::delivery::pr::carry::PrCarry::Deferred => {
                return Err(PrRefusal::TaskChanged.into());
            }
            crate::engine::delivery::pr::carry::PrCarry::Environment(reason) => {
                return Err(PrRefusal::CarryFailed(format!(
                    "the setup of the carry worktree failed (environment): {reason}"
                ))
                .into());
            }
        }
        let local_sha = publication.push(&task.branch).await?;
        let receipt = publication.open(&local_sha).await?;
        record_publication(self, &p, &publication, receipt)
    }
}
