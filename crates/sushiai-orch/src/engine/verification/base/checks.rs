use super::jobs::BaseCheckJob;
use crate::engine::state::App;
use crate::engine::verification::base::answers::base_check_answered;
use crate::engine::verification::base::answers::is_kept;
use crate::engine::verification::base::jobs::start_base_jobs;
use crate::engine::verification::base::jobs::wait_for_base;
use crate::engine::verification::base::jobs::wait_for_base_job;
use crate::engine::verification::base::run::run_on_base;
use crate::engine::verification::baseline::record_baselines;
use crate::engine::verification::context::record_check_decisions;
use crate::engine::verification::context::record_check_outcomes;
use crate::engine::verification::scoping::scoped_checks_for;
use crate::engine::verification::scoping::split_deferred;
use crate::git::paths::short_sha;
use crate::harness::cancel::CancelToken;
use crate::task::attempt::VerifyOutcome;
use crate::task::state::Task;
use crate::verify::passes::is_interrupted;
use crate::verify::scoping::filter_scoped_planned;
use std::path::Path;
use std::sync::Arc;

/// The result of `command` on `base_sha`: the cache, else the in-flight
/// background job of the same pair, else a job started now (its run folders
/// under `run_dir`). `None` after environment failures, when the base cannot
/// be checked out or the wait was cancelled: nothing is known then.
/// `background` picks the lane of a job started here (see `start_base_jobs`).
pub(in crate::engine) async fn base_result(
    app: &Arc<App>,
    job: BaseCheckJob,
    command: &str,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    let task_id = job.task_id.as_deref();
    let repo = job.repo.as_str();
    let base_sha = job.base_sha.as_str();
    let run_dir = job.run_dir.as_path();
    let background = job.background;
    start_base_jobs(
        app,
        crate::engine::verification::base::jobs::BaseCheckJob::new(
            task_id, repo, base_sha, run_dir, background,
        ),
        &[command.to_string()],
    );
    wait_for_base(app, base_sha, command, cancel).await
}

/// The base's result for `failed`, when the same command also exits non-zero
/// there: the failure is older than this task. `None` for a command that
/// passes on base or a base that cannot be run. A final check: a job started
/// here runs in the background lane.
pub(in crate::engine) async fn failing_on_base(
    app: &Arc<App>,
    task: &mut Task,
    failed: &VerifyOutcome,
    run_dir: &Path,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    if is_interrupted(failed) {
        return None;
    }
    // The same commit the baselines ran on: the base with the planner's tests.
    let rev = base_rev(task).ok()?;
    let key = (rev.clone(), failed.command.clone());
    let mut job = app
        .final_base_jobs
        .lock()
        .unwrap()
        .get(&task.id)
        .and_then(|jobs| jobs.get(&key).cloned());
    if job.is_none() {
        let jobs = start_base_jobs(
            app,
            crate::engine::verification::base::jobs::BaseCheckJob::new(
                Some(&task.id),
                &task.repo,
                &rev,
                run_dir,
                true,
            ),
            std::slice::from_ref(&failed.command),
        );
        job = jobs.get(&key).cloned();
        app.final_base_jobs
            .lock()
            .unwrap()
            .entry(task.id.clone())
            .or_default()
            .extend(jobs);
    }
    let base = wait_for_base_job(app, &rev, &failed.command, job, cancel).await;
    record_base_run_decisions(app, task, &rev, std::slice::from_ref(&failed.command));
    base.filter(|r| r.code.is_some_and(|c| c != 0) && !is_interrupted(r))
}

/// A `verify` command that failed after the work was carried onto a moved
/// base: `Some` with the base's own result when the command fails on the new
/// base alone yet passed on the base the attempt started from, so the base
/// broke it and the attempt did not. A command that already failed on the
/// old base is a check the work is meant to turn green, never blamed on the
/// base. Cached per base commit like every base run.
pub(in crate::engine) async fn broken_by_the_new_base(
    app: &Arc<App>,
    job: BaseCheckJob,
    failed: &VerifyOutcome,
    old_base: &str,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    if is_interrupted(failed) {
        return None;
    }
    // A `verify` command: its lookups use the blocking lane, so a slow
    // background final of the same repo never holds them back.
    let on_new = base_result(
        app,
        BaseCheckJob {
            run_dir: job.run_dir.join("new"),
            ..job.clone()
        },
        &failed.command,
        cancel,
    )
    .await
    .filter(|b| b.code.is_some_and(|c| c != 0) && !is_interrupted(b))?;
    let on_old = base_result(
        app,
        BaseCheckJob {
            base_sha: old_base.to_string(),
            run_dir: job.run_dir.join("old"),
            ..job
        },
        &failed.command,
        cancel,
    )
    .await?;
    (on_old.code == Some(0)).then_some(on_new)
}

/// Records on `task` the commands whose first base run failed and whose
/// rerun passed: `base check <cmd> flaky on <sha>: passed on rerun`.
pub(in crate::engine) fn note_flaky_base_checks(app: &App, task: &mut Task, commands: &[String]) {
    let flaky = app.base_flaky.lock().unwrap();
    for c in commands {
        if !flaky.contains(&(task.base_sha.clone(), c.clone())) {
            continue;
        }
        let line = flaky_base_check_line(c, &task.base_sha);
        if !task.decisions.contains(&line) {
            task.record_decision(line);
        }
    }
}

pub(in crate::engine) fn flaky_base_check_line(command: &str, base_sha: &str) -> String {
    format!(
        "Orchestrator: base check {command} flaky on {}: passed on rerun",
        short_sha(base_sha)
    )
}

/// Runs `checks` on the task's base, waiting for the result: every command
/// whose pair is not cached starts a background job (or joins the one in
/// flight). A command that exits non-zero is run once more by the job (the
/// task records `base check <cmd> flaky on <sha>: passed on rerun`); every
/// one that fails both times is returned. Empty when every command passes,
/// the base cannot be checked out, or the run was cancelled.
///
/// A parent passes its unkept `verify` commands here and asks first: a
/// `verify` command is usually the very test the work is meant to turn
/// green, so failing on the base is what it is for.
pub(in crate::engine) async fn failing_twice_on_base(
    app: &Arc<App>,
    task: &mut Task,
    checks: &[String],
    cancel: &CancelToken,
) -> Vec<VerifyOutcome> {
    let commands = planned_base_commands(app, task, checks);
    if commands.is_empty() {
        return Vec::new();
    }
    let dir = app.store.task_dir(&task.id);
    start_base_jobs(
        app,
        crate::engine::verification::base::jobs::BaseCheckJob::new(
            Some(&task.id),
            &task.repo,
            &task.base_sha,
            &dir,
            false,
        ),
        &commands,
    );
    let mut failing = Vec::new();
    for c in &commands {
        if let Some(r) = wait_for_base(app, &task.base_sha, c, cancel).await {
            record_check_outcomes(app, task, std::slice::from_ref(&r));
            if r.code.is_some_and(|code| code != 0) && !is_interrupted(&r) {
                failing.push(r);
            }
        }
    }
    if cancel.is_cancelled() {
        return Vec::new();
    }
    record_base_run_decisions(app, task, &task.base_sha.clone(), &commands);
    note_flaky_base_checks(app, task, &commands);
    failing
}

fn record_base_run_decisions(app: &App, task: &mut Task, rev: &str, commands: &[String]) {
    let decisions = app.base_run_decisions.lock().unwrap();
    let mut lines = Vec::new();
    for command in commands {
        if let Some(recorded) = decisions.get(&(rev.to_string(), command.clone())) {
            for line in recorded {
                let line = if let Some(held) = task.held_out.as_ref().filter(|h| &h.run == command)
                {
                    line.replace(
                        command,
                        &crate::task::checks::commands::held_out_label(held.criterion),
                    )
                } else {
                    line.clone()
                };
                lines.push(line);
            }
        }
    }
    record_check_decisions(task, lines);
}

/// `checks` without duplicates, minus what the scoped-checks policy skips
/// for the task's planned paths (no diff yet: they stand in for it).
fn planned_base_commands(app: &App, task: &Task, checks: &[String]) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    for c in checks {
        if !commands.contains(c) {
            commands.push(c.clone());
        }
    }
    filter_scoped_planned(
        &scoped_checks_for(&app.settings.read().unwrap(), task),
        &task.repo,
        &commands,
        &task.paths,
    )
}

/// The base commit with the planner's tests written over it, or the base
/// itself without any: a commit no branch holds, the same sha for the same
/// input. Baselines and final base checks both run on it.
pub(in crate::engine) fn base_rev(task: &Task) -> Result<String, crate::git::GitError> {
    if task.planner_tests.files.is_empty() {
        return Ok(task.base_sha.clone());
    }
    let files: Vec<(String, Vec<u8>)> = task
        .planner_tests
        .files
        .iter()
        .map(|t| (t.path.clone(), t.content.clone().into_bytes()))
        .collect();
    crate::delivery::git::commit::commit_over(Path::new(&task.repo), &task.base_sha, &files)
}

/// Starts the background base jobs of the task's `finalVerify` that are not
/// answered yet; the first attempt does not wait for them.
pub(in crate::engine) fn start_final_base_jobs(app: &Arc<App>, task: &Task) {
    // A command deferred to the parent does not run here, so it has no base
    // run here either.
    let (finals, _) = {
        let settings = app.settings.read().unwrap().clone();
        split_deferred(app, &settings, task)
    };
    let unanswered: Vec<String> = finals
        .into_iter()
        .filter(|c| !is_kept(task, c) && !base_check_answered(task, c, &task.base_sha))
        .collect();
    let commands = planned_base_commands(app, task, &unanswered);
    if commands.is_empty() {
        return;
    }
    // The key `failing_on_base` reads: the base with the planner's tests.
    let Ok(rev) = base_rev(task) else {
        return;
    };
    let dir = app.store.task_dir(&task.id);
    let jobs = start_base_jobs(
        app,
        crate::engine::verification::base::jobs::BaseCheckJob::new(
            Some(&task.id),
            &task.repo,
            &rev,
            &dir,
            true,
        ),
        &commands,
    );
    app.final_base_jobs
        .lock()
        .unwrap()
        .entry(task.id.clone())
        .or_default()
        .extend(jobs);
}

/// Baselines the checks that have none yet on the base commit with the
/// planner's tests written over it (not in the task's worktree, which may hold
/// the implementer's changes). `false` when
/// cancelled or the base could not be checked out; nothing is stored then.
pub(in crate::engine) async fn baseline_on_base(
    app: &Arc<App>,
    task: &mut Task,
    cancel: &CancelToken,
) -> bool {
    let ticket = crate::task::checks::baseline::BaselineTicket::capture(task);
    let todo = &ticket.todo;
    if todo.is_empty() {
        return true;
    }
    let commands: Vec<String> = todo.iter().map(|(_, _, run)| run.clone()).collect();
    let run_dir = app.store.task_dir(&task.id).join("baseline-base");
    // The base with the planner's tests: a commit no branch holds.
    let Ok(rev) = base_rev(task) else {
        return false;
    };
    match run_on_base(
        app,
        crate::engine::verification::base::jobs::BaseCheckJob::new(
            Some(&task.id),
            &task.repo,
            &rev,
            &run_dir,
            false,
        ),
        &commands,
        cancel,
    )
    .await
    {
        Ok(results) if results.len() == todo.len() && !cancel.is_cancelled() => {
            record_check_outcomes(app, task, &results);
            record_base_run_decisions(app, task, &rev, &commands);
            record_baselines(task, &ticket, &results)
        }
        _ => false,
    }
}
