use super::checkout::BaseCheckout;
use super::jobs::BaseCheckJob;
use crate::engine::state::App;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::run_verify_commands;
use crate::harness::cancel::CancelToken;
use crate::task::attempt::VerifyOutcome;
use crate::verify::base_cache::save_base_runs;
use crate::verify::passes::is_interrupted;
use std::collections::HashMap;
use std::sync::Arc;

/// Runs `commands` on the commit `base_sha` of `repo`, in a throwaway
/// detached worktree that is removed again whatever happens (done, failed or
/// cancelled). Results are cached on the app per (base sha, command), so the
/// same command on an unchanged base runs once. A checkout error gets one retry
/// in a fresh worktree.
/// `Err` when neither checkout could be made; nothing is known about the base.
pub(in crate::engine) async fn run_on_base(
    app: &Arc<App>,
    job: BaseCheckJob,
    commands: &[String],
    cancel: &CancelToken,
) -> Result<Vec<VerifyOutcome>, String> {
    let task_id = job.task_id.as_deref();
    let repo = job.repo.as_str();
    let base_sha = job.base_sha.as_str();
    let mut known: HashMap<String, VerifyOutcome> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    {
        let cache = app.base_runs.lock().unwrap();
        for c in commands {
            match cache.get(&(base_sha.to_string(), c.clone())) {
                Some(r) => {
                    known.insert(c.clone(), r.clone());
                }
                None if !missing.contains(c) => missing.push(c.clone()),
                None => {}
            }
        }
    }
    for attempt in 0..2 {
        if missing.is_empty() || cancel.is_cancelled() {
            break;
        }
        let (checkout, setup) = BaseCheckout::prepare(app, &job, cancel).await;
        let ran = match setup {
            Ok(()) => {
                let results = run_verify_commands(
                    crate::engine::verification::runner::CheckRunner::from_context(
                        CheckContext::new(app, task_id).with_gate("base checks"),
                        &checkout.worktree,
                        checkout.sandbox.as_ref(),
                        base_sha,
                        cancel,
                    ),
                    &missing,
                )
                .await;
                for result in &results {
                    app.base_run_decisions
                        .lock()
                        .unwrap()
                        .entry((base_sha.to_string(), result.command.clone()))
                        .or_default()
                        .extend(result.decisions.iter().cloned());
                }
                Ok(results)
            }
            Err(e) => Err(format!("could not check out {base_sha}: {e}")),
        };
        checkout.remove(repo).await;
        let cancelled = cancel.is_cancelled();
        let results = match ran {
            Ok(results) => results,
            Err(_) if attempt == 0 && !cancelled => continue,
            Err(e) if known.is_empty() => return Err(e),
            Err(_) => break,
        };
        let mut cache = app.base_runs.lock().unwrap();
        for (c, r) in missing.iter().zip(results) {
            // An interrupted run says nothing about the commit.
            if !cancelled && r.code.is_some() && !is_interrupted(&r) {
                cache.insert((base_sha.to_string(), c.clone()), r.clone());
            }
            known.insert(c.clone(), r);
        }
        save_base_runs(&app.data_dir, &cache);
        missing.clear();
    }
    Ok(commands
        .iter()
        .filter_map(|c| known.get(c).cloned())
        .collect())
}
