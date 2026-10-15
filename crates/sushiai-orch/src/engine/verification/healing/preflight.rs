use crate::clock::now_ms;
use crate::engine::brief_check::run_judge;
use crate::engine::brief_check::JudgeRun;
use crate::engine::state::App;
use crate::engine::verification::base::answers::keep_planned_checks;
use crate::engine::verification::base::checks::start_final_base_jobs;
use crate::engine::verification::base::run::run_on_base;
use crate::engine::verification::context::record_check_outcomes;
use crate::engine::verification::healing::contract::apply_feasibility;
use crate::engine::verification::healing::contract::judge_route;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::task::attempt::VerifyOutcome;
use crate::task::checks::edits::heal_grep_dashes;
use crate::task::state::Task;
use crate::verify::healing::reply::feasibility_prompt;
use crate::verify::healing::reply::parse_feasibility;
use crate::verify::healing::reply::Feasibility;
use crate::verify::load::timed_out_under_load;
use std::sync::Arc;

/// What the background base preflight found; the loop applies it to its own
/// task (`apply_base_preflight`), the job itself writes no task state.
#[derive(Clone, Default)]
pub(in crate::engine) struct BasePreflight {
    /// The base could be checked out and the judge ran over every failure:
    /// feasibility is settled for good.
    settled: bool,
    rulings: Vec<(VerifyOutcome, Feasibility)>,
    /// Decision lines the plan-kept rule wrote (`keep_planned_checks`).
    decisions: Vec<String>,
    cost: f64,
}

/// One task's base preflight: its result arrives on `result`; the channel
/// closes without a value when the job was cancelled.
pub(in crate::engine) struct PreflightJob {
    result: tokio::sync::watch::Receiver<Option<Arc<BasePreflight>>>,
    applied: std::sync::atomic::AtomicBool,
    /// The commands the job runs on the base; a job for other commands is
    /// replaced, not reused.
    snapshot: crate::task::checks::snapshot::CheckSnapshot,
    cancel: CancelToken,
}

impl PreflightJob {
    /// The result, waiting for the job when it still runs; `None` when it
    /// was cancelled.
    async fn wait(&self) -> Option<Arc<BasePreflight>> {
        let mut rx = self.result.clone();
        let seen = rx.wait_for(|v| v.is_some()).await.ok()?;
        seen.clone()
    }

    fn running(&self) -> bool {
        self.result.borrow().is_none() && self.result.has_changed().is_ok()
    }

    fn finished(&self) -> bool {
        self.result.borrow().is_some()
    }
}

/// The commands the base preflight runs for `task`: every `verify` and
/// `checks` command, once each (`finalVerify` has its own base jobs).
fn preflight_commands(task: &Task) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    for c in task.verify.iter().chain(task.checks.iter().map(|c| &c.run)) {
        if !c.trim().is_empty() && !commands.contains(c) {
            commands.push(c.clone());
        }
    }
    commands
}

/// Starts the base preflight of `task` in the background, holding no slot:
/// every `verify` and `checks` command runs on the base once (`run_on_base`)
/// and each failure goes to the feasibility judge. The result only goes into
/// the app's map; nothing is saved here. A no-op while a job of this task
/// for the same commands is running or finished; a job for other commands
/// (the draft or the brief check rewrote them) is cancelled and started
/// again. `loop_cancel` stops the job.
pub(in crate::engine) fn spawn_base_preflight(
    app: &Arc<App>,
    task: &Task,
    loop_cancel: &CancelToken,
) {
    let snapshot = crate::task::checks::snapshot::CheckSnapshot::capture(task);
    let mut jobs = app.base_preflights.lock().unwrap();
    if let Some(job) = jobs.get(&task.id) {
        if job.snapshot.matches(task) && (job.running() || job.finished()) {
            return;
        }
        job.cancel.cancel();
    }
    let (tx, rx) = tokio::sync::watch::channel(None);
    // The loop's cancel reaches the job through the job's own token, which
    // the base runs watch, so their scratch checkout is cleaned up.
    let cancel = CancelToken::new();
    jobs.insert(
        task.id.clone(),
        Arc::new(PreflightJob {
            result: rx,
            applied: std::sync::atomic::AtomicBool::new(false),
            snapshot,
            cancel: cancel.clone(),
        }),
    );
    drop(jobs);
    let (app, task, loop_cancel) = (app.clone(), task.clone(), loop_cancel.clone());
    tokio::spawn(async move {
        let watcher = {
            let job_cancel = cancel.clone();
            tokio::spawn(async move {
                loop_cancel.cancelled().await;
                job_cancel.cancel();
            })
        };
        let result = run_base_preflight(&app, &task, &cancel).await;
        watcher.abort();
        if let Some(result) = result {
            let _ = tx.send(Some(Arc::new(result)));
        }
    });
}

/// Starts the base jobs of `task` that its brief check does not need to
/// wait for: the preflight of its `verify` and `checks` and the base runs of
/// its `finalVerify`. Called as soon as the commands exist and again after a
/// rewrite; a job for commands that did not change is not started twice.
pub(in crate::engine) fn start_base_checks(app: &Arc<App>, task: &mut Task, cancel: &CancelToken) {
    if task.brief_check.feasibility_done {
        return;
    }
    if heal_grep_dashes(task) {
        app.verify_cache.lock().unwrap().remove(&task.id);
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
    spawn_base_preflight(app, task, cancel);
    start_final_base_jobs(app, task);
}

/// The job body. `None`: cancelled.
async fn run_base_preflight(
    app: &Arc<App>,
    task: &Task,
    cancel: &CancelToken,
) -> Option<BasePreflight> {
    let mut out = BasePreflight::default();
    // No judge: nothing is ruled and nothing is settled.
    let Some(route) = judge_route(app, &task.variant()) else {
        return Some(out);
    };
    // `finalVerify` is not judged here: its base runs are the background
    // jobs of `start_final_base_jobs` and the finals gate after review. A
    // command that is in `verify` as well is still judged.
    let commands = preflight_commands(task);
    let run_dir = match app
        .store
        .claim_helper_run_dir(&task.id, "feasibility-base", None)
    {
        Ok(dir) => dir,
        Err(_) => return Some(out),
    };
    let ran = run_on_base(
        app,
        crate::engine::verification::base::jobs::BaseCheckJob::new(
            Some(&task.id),
            &task.repo,
            &task.base_sha,
            &run_dir,
            false,
        ),
        &commands,
        cancel,
    )
    .await;
    if cancel.is_cancelled() {
        return None;
    }
    // A base that could not be checked out says nothing: not settled.
    let Ok(results) = ran else {
        return Some(out);
    };
    // A check that names files the plan creates is feasible by rule: no
    // judge run, only the kept decision.
    let failing: Vec<String> = results
        .iter()
        .filter(|r| r.code != Some(0) && !timed_out_under_load(r))
        .map(|r| r.command.clone())
        .collect();
    let mut scratch = task.clone();
    record_check_outcomes(app, &mut scratch, &results);
    let kept = keep_planned_checks(app, &mut scratch, &failing);
    out.decisions = scratch.decisions[task.decisions.len()..].to_vec();
    for failed in results
        .iter()
        .filter(|r| r.code != Some(0) && !timed_out_under_load(r) && !kept.contains(&r.command))
    {
        let prompt = feasibility_prompt(task, failed);
        let reply = run_judge(
            app,
            task,
            &route,
            None,
            "brief_check",
            "feasibility",
            &prompt,
            cancel,
        )
        .await;
        match reply {
            Err(RunError::Cancelled) => return None,
            Ok(JudgeRun {
                reply,
                cost,
                timeout_note,
            }) => {
                out.decisions.extend(timeout_note);
                out.cost += cost;
                if let Some(r) = reply
                    .ok()
                    .and_then(|text| parse_feasibility(&text, task.criteria.len()))
                {
                    out.rulings.push((failed.clone(), r));
                }
            }
            Err(RunError::Io(_) | RunError::NotFound(_)) => {}
        }
    }
    out.settled = true;
    Some(out)
}

/// Waits for the task's base preflight (a running one is awaited and the wait
/// is a decision line) and applies it to the loop's own `task`: the judge's
/// cost, the rulings (`apply_feasibility`'s exceptions hold: exit 127 keeps
/// gating, a shared check is never switched off) and, when the judge settled,
/// `feasibility_done`. Saves and broadcasts. A no-op without a job or after
/// it was applied. `false`: cancelled while waiting.
pub(in crate::engine) async fn settle_base_preflight(
    app: &Arc<App>,
    task: &mut Task,
    cancel: &CancelToken,
) -> bool {
    let Some(job) = app.base_preflights.lock().unwrap().get(&task.id).cloned() else {
        return true;
    };
    if job.applied.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    let waited = job.running();
    let result = tokio::select! {
        r = job.wait() => r,
        _ = cancel.cancelled() => return false,
    };
    if job.applied.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    let Some(result) = result else {
        // A cancelled job: forget it, a later start may spawn a new one.
        app.base_preflights.lock().unwrap().remove(&task.id);
        return true;
    };
    if !job.snapshot.matches(task) {
        task.charge(result.cost);
        task.record_decision("Orchestrator: discarded a stale base preflight".to_string());
        app.base_preflights.lock().unwrap().remove(&task.id);
        app.save_task_logged(task);
        app.broadcast_task(task);
        return true;
    }
    if waited {
        task.record_decision(
            "Orchestrator: waited for the base preflight before the final checks".to_string(),
        );
    }
    apply_base_preflight(app, task, &result);
    app.save_task_logged(task);
    app.broadcast_task(task);
    true
}

fn apply_base_preflight(app: &Arc<App>, task: &mut Task, result: &BasePreflight) {
    task.charge(result.cost);
    for line in &result.decisions {
        if !task.decisions.contains(line) {
            task.record_decision(line.clone());
        }
    }
    let mut changed = false;
    for (failed, ruling) in &result.rulings {
        // 127: the shell could not find a program. A rewrite to a check the
        // repo can run is fine, but "unrelated" would be this host's PATH or
        // toolchain quietly switching the check off: it keeps gating.
        if failed.code == Some(127) && matches!(ruling, Feasibility::Unrelated { .. }) {
            task.record_decision(format!(
                "Orchestrator: check {} cannot find a program on this host (exit 127); it keeps gating - fix the host's PATH or toolchain",
                failed.command
            ));
            changed = true;
            continue;
        }
        let settings = app.settings.read().unwrap().clone();
        changed |= apply_feasibility(&settings, task, &failed.command, ruling.clone());
    }
    if changed {
        app.verify_cache.lock().unwrap().remove(&task.id);
    }
    if result.settled {
        task.mark_feasibility_done();
    }
    task.touch(now_ms());
}

#[cfg(test)]
mod tests;
