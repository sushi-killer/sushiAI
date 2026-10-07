use super::*;

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
        permissions::apply_staged(&ctx);
        let wt = worktree.clone();
        let base = base_sha.clone();
        let changed =
            tokio::task::spawn_blocking(move || git::changed_files(&wt, &base).unwrap_or_default())
                .await
                .unwrap_or_default();
        let has_changed = !changed.is_empty();
        let verify_configured = !ctx.verify.is_empty();

        let (hook_verify, _) = filter_scoped(
            &self.settings.read().unwrap().scoped_checks,
            &ctx.repo,
            &ctx.verify,
            &changed,
        );
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
                    &app,
                    &ctx.task_id,
                    &worktree,
                    &run_dir,
                    &base_sha,
                    &hook_verify,
                    &ctx.cancel,
                ),
            )
            .await
            {
                Ok(results) => results,
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
