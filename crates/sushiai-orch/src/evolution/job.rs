use super::clustering::Cluster;
use super::evidence::excerpts;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::prompts::evolution::proposal::ProposalBriefInput;
use serde_json::json;
use std::path::PathBuf;

/// What the cluster's latest task supplies to the eval command.
pub(super) struct Origin {
    pub(super) request: String,
    pub(super) base_sha: String,
}

/// A proposer run's job: what it was started for.
pub(super) struct ProposalJob {
    pub(super) id: String,
    pub(super) key: String,
    pub(super) kind: String,
    pub(super) repo: String,
    pub(super) toplevel: PathBuf,
    pub(super) origin: Origin,
}

pub(super) struct PreparedProposal {
    pub(super) job: ProposalJob,
    pub(super) brief: String,
    pub(super) cancel: CancelToken,
}

impl PreparedProposal {
    pub(super) async fn prepare(app: &App, cluster: &Cluster<'_>) -> Option<Self> {
        let repo = cluster.main_repo().to_string();
        let path = PathBuf::from(&repo);
        let Ok(Ok(toplevel)) =
            tokio::task::spawn_blocking(move || crate::git::history::repo_toplevel(&path)).await
        else {
            return None;
        };
        let latest = cluster.latest();
        let task = app.store.load_task(&latest.task_id).ok().flatten();
        let origin = Origin {
            request: task
                .as_ref()
                .map(|t| {
                    t.request
                        .clone()
                        .filter(|r| !r.trim().is_empty())
                        .unwrap_or_else(|| t.goal.clone())
                })
                .unwrap_or_else(|| latest.detail.clone()),
            base_sha: task.map(|t| t.base_sha.clone()).unwrap_or_default(),
        };
        let tasks = cluster.tasks();
        let (calls, usd) = (cluster.wasted_calls(), cluster.wasted_usd());
        let brief_text =
            crate::prompts::evolution::proposal::build_proposal_brief(&ProposalBriefInput {
                kind: &cluster.kind,
                detail: &latest.detail,
                repo: &repo,
                occurrences: cluster.occurrences.len(),
                tasks,
                wasted_calls: calls,
                wasted_usd: usd,
                excerpts: &excerpts(&app.data_dir, cluster),
            });
        let job = ProposalJob {
            id: uuid::Uuid::new_v4().to_string(),
            key: cluster.key.clone(),
            kind: cluster.kind.clone(),
            repo: repo.clone(),
            toplevel,
            origin,
        };
        let cancel = CancelToken::new();
        {
            let mut in_flight = app.proposals.lock().unwrap();
            if in_flight.contains_key(&job.key) {
                return None;
            }
            in_flight.insert(job.key.clone(), cancel.clone());
        }
        Some(Self {
            job,
            brief: brief_text,
            cancel,
        })
    }

    pub(super) fn summary(&self, cluster: &Cluster<'_>) -> serde_json::Value {
        json!({
            "id": self.job.id, "clusterKey": self.job.key, "kind": self.job.kind, "repo": self.job.repo,
            "tasks": cluster.tasks(), "wastedCalls": cluster.wasted_calls(), "wastedUsd": cluster.wasted_usd(),
        })
    }

    pub(super) fn release(&self, app: &App) {
        app.proposals.lock().unwrap().remove(&self.job.key);
    }
}
