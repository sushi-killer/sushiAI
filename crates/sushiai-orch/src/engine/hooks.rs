use crate::engine::state::App;
use crate::engine::verification::cache::run_verify_cached;
use crate::engine::verification::scoping::scoped_checks_for;
use crate::policy::stop as hook;
use crate::verify::command::hook_budget_secs;
use crate::verify::scoping::filter_scoped;
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

impl App {
    pub(super) async fn handle_hook_stop(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            token: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let ctx = { self.hook_tokens.read().unwrap().get(&p.token).cloned() };
        let Some(ctx) = ctx else {
            return Ok(json!({}));
        };

        let _hook_done = ctx.hook_running.start();
        let worktree = ctx.worktree.clone();
        let base_sha = ctx.base_sha.clone();
        // What the owner let the agent stage at a protected path is in place
        // before the diff is read and verified.
        crate::engine::permissions::staging::apply_staged(&ctx);
        let (wt, own) = (worktree.clone(), ctx.own.clone());
        let changed =
            tokio::task::spawn_blocking(move || own.changed_files(&wt).unwrap_or_default())
                .await
                .unwrap_or_default();
        let has_changed = !changed.is_empty();
        let verify_configured = !ctx.verify.is_empty();

        let scoped = self
            .store
            .load_task(&ctx.task_id)
            .ok()
            .flatten()
            .map(|task| scoped_checks_for(&self.settings.read().unwrap(), &task))
            .unwrap_or_default();
        let (hook_verify, _) = filter_scoped(&scoped, &ctx.repo, &ctx.verify, &changed);
        let verify_results = if !hook_verify.is_empty() && has_changed {
            let app = self.arc();
            let run_dir = self.store.run_dir(&ctx.task_id, ctx.attempt_n);

            // Derived from the verify timeout, capped well under Claude's
            // own 600s hook timeout; on timeout, fail open rather than block
            // the agent forever.
            let budget = hook_budget_secs(self.settings.read().unwrap().verify_timeout_secs);
            match tokio::time::timeout(
                Duration::from_secs(budget),
                run_verify_cached(
                    crate::engine::verification::runner::CheckRunner::from_context(
                        crate::engine::verification::context::CheckContext::new(
                            &app,
                            Some(&ctx.task_id),
                        ),
                        &worktree,
                        app.task_sandbox(&ctx.repo, &worktree, &run_dir).as_ref(),
                        &base_sha,
                        &ctx.cancel,
                    ),
                    &ctx.task_id,
                    &hook_verify,
                ),
            )
            .await
            {
                Ok((results, lines)) => {
                    for line in lines {
                        self.append_decision(&ctx.task_id, line);
                    }
                    results
                }
                Err(_) => return Ok(json!({})),
            }
        } else {
            vec![]
        };

        let facts = hook::StopFacts {
            blocks_so_far: ctx.blocks.load(Ordering::SeqCst),
            has_changed_files: has_changed,
            verify_configured,
            verify_results: &verify_results,
        };
        let decision = hook::decide_stop(&facts);
        match decision {
            hook::StopDecision::Allow => Ok(json!({})),
            hook::StopDecision::Block { reason } => {
                ctx.blocks.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"decision": "block", "reason": reason}))
            }
        }
    }
}
