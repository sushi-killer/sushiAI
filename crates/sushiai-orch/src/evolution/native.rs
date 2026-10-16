use super::job::{Origin, PreparedProposal, ProposalJob};
use crate::clock::now_ms;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::{run_harness, RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::state::App;
use crate::harness;
use crate::harness::guard::Guard;
use crate::policy::evolution::proposal::gate;
use crate::policy::routing::Route;
use crate::task::evolution::{Proposal, ProposalReply, ProposalStatus, Track};
use serde_json::json;
use std::sync::Arc;

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn eval_command(app: &App, repo: &str, origin: &Origin, arm: Option<&serde_json::Value>) -> String {
    let arm = arm.cloned().unwrap_or_else(|| json!({"<flag>": true}));
    let mut cmd = format!(
        "SUSHIAI_HOME={} {} orch eval run --repo {} --request {}",
        shell_quote(&app.home.to_string_lossy()),
        shell_quote(&app.exe_path),
        shell_quote(repo),
        shell_quote(&origin.request),
    );
    if !origin.base_sha.is_empty() {
        cmd.push_str(&format!(" --base {}", shell_quote(&origin.base_sha)));
    }
    cmd.push_str(&format!(
        " --arms {}",
        shell_quote(&json!([{}, arm]).to_string())
    ));
    cmd
}

impl PreparedProposal {
    pub(super) fn launch(self, app: &App, route: Route) {
        let dir = app.store.proposal_run_dir(&self.job.id);
        if std::fs::create_dir_all(&dir)
            .and_then(|()| std::fs::write(dir.join("brief.md"), &self.brief))
            .is_err()
        {
            self.release(app);
            return;
        }
        let app = app.arc();
        tokio::spawn(async move { self.run(&app, route).await });
    }

    async fn run(self, app: &Arc<App>, route: Route) {
        let Self {
            job,
            brief: brief_text,
            cancel,
        } = self;
        let dir = app.store.proposal_run_dir(&job.id);
        // A parallel slot, like every other harness run.
        let permit = tokio::select! {
            _ = cancel.cancelled() => None,
            p = app.slots.clone().acquire_owned() => p.ok(),
        };
        if let Some(_permit) = permit {
            let settings = app.settings.read().unwrap().clone();
            let (mcp_path, settings_path, key_path) =
                crate::engine::audit::run::prepare_run(app, &route, &dir, &settings);
            let req = crate::engine::audit::run::audit_request(
                &route,
                &job.toplevel,
                &mcp_path,
                &settings_path,
            );
            let result = run_harness(
                app,
                RunIdentity {
                    id: &job.id,
                    attempt_n: 0,
                    track: RunTrack::No,
                    tag: CostTag::repo("evolution", &route.id, &job.repo),
                },
                RunInvocation {
                    request: &req,
                    brief: &brief_text,
                    events: &dir.join("events.jsonl"),
                    schema: None,
                },
                &cancel,
                Guard::off(),
            )
            .await;
            let _ = std::fs::remove_file(&key_path);
            if let Ok(outcome) = result {
                let text = outcome.final_text.clone().unwrap_or_default();
                // No parsable block stores nothing, so the next run retries.
                if let Ok(reply) = crate::prompts::evolution::proposal::parse_proposal(&text) {
                    store_reply(app, &job, reply, &outcome).await;
                }
            }
        }
        app.proposals.lock().unwrap().remove(&job.key);
    }
}

async fn store_reply(
    app: &App,
    job: &ProposalJob,
    reply: ProposalReply,
    outcome: &harness::RunOutcome,
) {
    let reason = gate(&reply);
    let eval_command = (reply.track == Track::Harness)
        .then(|| eval_command(app, &job.repo, &job.origin, reply.arm.as_ref()));
    let proposal = Proposal {
        id: job.id.clone(),
        cluster_key: job.key.clone(),
        kind: job.kind.clone(),
        repo: job.repo.clone(),
        track: reply.track,
        form: reply.form,
        change: reply.change,
        evidence: reply.evidence,
        metric: reply.metric,
        test: reply.test,
        status: if reason.is_some() {
            ProposalStatus::Rejected
        } else {
            ProposalStatus::Proposed
        },
        created_at: now_ms(),
        cost_usd: outcome.cost_usd.unwrap_or(0.0),
        fingerprint: outcome.fingerprint.clone(),
        eval_command,
        task_id: None,
        adopted_at: None,
        before: None,
        after: None,
        reason,
    };
    let _guard = app.proposal_lock.lock().await;
    let _ = app.store_proposal(&proposal);
}
