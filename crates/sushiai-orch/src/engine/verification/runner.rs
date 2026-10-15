use crate::engine::state::App;
use crate::engine::verification::context::CheckContext;
use crate::git::paths::short_sha;
use crate::harness::cancel::CancelToken;
use crate::harness::compiler_cache::CacheRun;
use crate::sandbox::TaskSandbox;
use crate::task::attempt::VerifyOutcome;
use crate::verify::command::build_verify_command;
use crate::verify::command::run_verify_command;
use crate::verify::load::timed_out_under_load;
use crate::verify::output::CANCELLED_TAIL;
use crate::verify::passes::check_tree;
use crate::verify::passes::is_interrupted;
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Copy)]
pub(in crate::engine) struct CheckRunner<'a> {
    pub(in crate::engine) ctx: CheckContext<'a>,
    pub(in crate::engine) cwd: &'a Path,
    pub(super) sandbox: Option<&'a TaskSandbox>,
    pub(super) timeout: Duration,
    pub(in crate::engine) base: &'a str,
    pub(in crate::engine) cancel: &'a CancelToken,
}

impl<'a> CheckRunner<'a> {
    pub(in crate::engine) fn new(
        app: &'a App,
        cwd: &'a Path,
        sandbox: Option<&'a TaskSandbox>,
        base: &'a str,
        cancel: &'a CancelToken,
    ) -> Self {
        Self {
            ctx: CheckContext::new(app, None),
            cwd,
            sandbox,
            timeout: Duration::from_secs(app.settings.read().unwrap().verify_timeout_secs),
            base,
            cancel,
        }
    }

    pub(in crate::engine) fn from_context(
        ctx: CheckContext<'a>,
        cwd: &'a Path,
        sandbox: Option<&'a TaskSandbox>,
        base: &'a str,
        cancel: &'a CancelToken,
    ) -> Self {
        Self {
            ctx,
            ..Self::new(ctx.app, cwd, sandbox, base, cancel)
        }
    }

    pub(in crate::engine) fn with_timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    #[cfg(test)]
    pub(in crate::engine) async fn run(
        &self,
        commands: &[String],
    ) -> (Vec<VerifyOutcome>, Vec<String>) {
        let mut results = Vec::new();
        let mut lines = Vec::new();
        for command in commands {
            let (result, decisions) = self.run_one(command, command, None).await;
            results.push(result);
            lines.extend(decisions);
        }
        (results, lines)
    }

    pub(in crate::engine) async fn run_one(
        &self,
        command: &str,
        label: &str,
        cached: Option<&VerifyOutcome>,
    ) -> (VerifyOutcome, Vec<String>) {
        let ctx = self.ctx.with_label(label);
        if !self.cancel.is_cancelled() {
            if let Some(outcome) = passed_on_tree(ctx, self.cwd, command, self.base).await {
                let lines = outcome.decisions.clone();
                return (outcome, lines);
            }
            if let Some(cached) =
                cached.filter(|r| r.code.is_some_and(|c| c != 0) && !is_interrupted(r))
            {
                return (cached.clone(), Vec::new());
            }
        }
        let runner = CheckRunner { ctx, ..*self };
        let outcome = run_attempts(&runner, command).await;
        let lines = outcome.decisions.clone();
        (outcome, lines)
    }

    pub(in crate::engine) fn for_task(mut self, task_id: &'a str) -> Self {
        self.ctx = CheckContext::new(self.ctx.app, Some(task_id));
        self
    }
}

pub(in crate::engine) async fn run_verify_commands(
    runner: CheckRunner<'_>,
    commands: &[String],
) -> Vec<VerifyOutcome> {
    let CheckRunner { ctx, cwd, .. } = runner;
    let cache_run = CacheRun::for_commands(cwd, commands).await;
    let mut results = Vec::new();
    for cmd in commands {
        let timeout = Duration::from_secs(ctx.app.settings.read().unwrap().verify_timeout_secs);
        results.push(run_one_verify_command(CheckRunner { timeout, ..runner }, cmd).await);
    }
    if let Some(line) = cache_run.finish(ctx.gate).await {
        if let Some(result) = results.last_mut() {
            ctx.decision(line, &mut result.decisions);
        }
    }
    results
}

/// Grades an eval task: runs its set's `check` from the root of a throwaway
/// detached worktree at `sha` (never the task's own), with the verify timeout.
pub(in crate::engine) async fn run_eval_check(
    runner: CheckRunner<'_>,
    repo: &Path,
    cmd: &str,
) -> VerifyOutcome {
    let CheckRunner {
        ctx,
        cwd,
        sandbox,
        base: sha,
        cancel,
        ..
    } = runner;
    let path = cwd.to_path_buf();
    let failed = |tail: String| VerifyOutcome {
        command: cmd.into(),
        tail,
        ..Default::default()
    };
    let (r, p, s) = (repo.to_path_buf(), path.clone(), sha.to_string());
    let made = tokio::task::spawn_blocking(move || {
        crate::git::worktree::remove_worktree(&r, &p);
        crate::git::worktree::add_detached_worktree(&r, &p, &s)?;
        let _ = crate::git::bootstrap::bootstrap_worktree(&r, &p);
        Ok::<(), crate::git::GitError>(())
    })
    .await;
    match made {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return failed(format!("could not check out {sha}: {e}")),
        Err(e) => return failed(format!("check checkout panicked: {e}")),
    }
    if let Err(e) = crate::engine::worktrees::setup_worktree(repo, &path, sandbox, cancel).await {
        return VerifyOutcome {
            setup_failed: true,
            ..failed(e)
        };
    }
    let timeout = Duration::from_secs(ctx.app.settings.read().unwrap().verify_timeout_secs);
    let cache_run = CacheRun::for_commands(&path, &[cmd.to_string()]).await;
    let mut out = run_one_verify_command(CheckRunner { timeout, ..runner }, cmd).await;
    if let Some(line) = cache_run.finish("eval checks").await {
        ctx.decision(line, &mut out.decisions);
    }
    let (r, p) = (repo.to_path_buf(), path);
    let _ =
        tokio::task::spawn_blocking(move || crate::git::worktree::remove_worktree(&r, &p)).await;
    out
}

/// Run one verify command in its own process group (`setsid`, like the
/// harness). After every outcome the group is stopped (SIGTERM, SIGKILL 5s
/// later); on timeout *or* cancellation processes that left the group but
/// run from `cwd` are stopped too -- a bare `tokio::time::timeout` around `Command::output()`
/// only stops *waiting*, it never touches the still-running process (or any
/// children it spawned), which is the bug this replaces.
async fn run_attempts(runner: &CheckRunner<'_>, cmd: &str) -> VerifyOutcome {
    let CheckRunner {
        ctx,
        cwd,
        base: base_sha,
        cancel,
        ..
    } = *runner;
    let mut decisions = Vec::new();
    let mut load_detail = None;
    let mut interrupted: Option<VerifyOutcome> = None;
    for attempt in 0..2 {
        // The only check-slot acquire site: every retry queues afresh, and
        // waiting never spends the command's execution timeout.
        let permit = acquire_check_slot(runner, cmd, &mut decisions).await;
        let Some(permit) = permit.filter(|_| !cancel.is_cancelled()) else {
            return VerifyOutcome {
                command: cmd.into(),
                tail: CANCELLED_TAIL.into(),
                decisions,
                ..Default::default()
            };
        };
        let tree = check_tree(cwd).await;
        let mut result = execute_verify_command(runner, cmd).await;
        result.tree.clone_from(&tree);
        drop(permit);
        if attempt == 0 && is_interrupted(&result) && !cancel.is_cancelled() {
            if timed_out_under_load(&result) {
                let detail = format!(
                    "{}: {} ms, timed out under load {:.2} on {} cores",
                    ctx.label.unwrap_or(cmd),
                    result.ms,
                    result.load,
                    result.cores
                );
                ctx.decision(
                    format!("Orchestrator: {detail} (environment); rerun once on the same tree"),
                    &mut decisions,
                );
                load_detail = Some(detail);
            }
            interrupted = Some(result);
            continue;
        }
        if let Some(first) = interrupted {
            let reason = first
                .tail
                .lines()
                .next()
                .filter(|l| !l.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    first
                        .code
                        .map_or_else(|| "interrupted".into(), |code| format!("exit {code}"))
                });
            let reason = if ctx.label.is_some_and(|label| label != cmd) {
                reason.replace(cmd, "<held-out check>")
            } else {
                reason
            };
            let rerun = match result.code {
                Some(code) if !is_interrupted(&result) => format!("exit {code}"),
                _ => "interrupted".to_string(),
            };
            ctx.decision(format!(
                "Orchestrator: {} was interrupted after {:.1} s ({reason}); ran it again at once: {rerun} after {:.1} s",
                ctx.label.unwrap_or(cmd), first.ms as f64 / 1000.0, result.ms as f64 / 1000.0,
            ), &mut decisions);
        }
        if let Some(detail) = load_detail {
            result.load_timeout = true;
            if result.code != Some(0) && result.tail != CANCELLED_TAIL {
                result.tail = format!("{detail}\nRerun: {}", result.tail);
            }
        }
        if result.code == Some(0) && !cancel.is_cancelled() {
            if let Some(tree) = tree {
                // A mutating check neither validates the resulting tree nor
                // supplies its side effects when reused on the original tree.
                if check_tree(cwd).await.as_deref() == Some(tree.as_str()) {
                    ctx.app
                        .gate_passes
                        .lock()
                        .unwrap()
                        .insert(&tree, base_sha, &result);
                }
            }
        }
        result.decisions = decisions;
        return result;
    }
    unreachable!("one initial run and at most one retry")
}

async fn execute_verify_command(runner: &CheckRunner<'_>, cmd: &str) -> VerifyOutcome {
    let CheckRunner {
        ctx,
        cwd,
        timeout,
        sandbox,
        base: base_sha,
        cancel,
    } = *runner;
    let (load, cores) = (ctx.app.check_load.read().unwrap())();
    let directory = cwd.to_path_buf();
    let commit =
        tokio::task::spawn_blocking(move || crate::git::history::head_sha(&directory).ok())
            .await
            .ok()
            .flatten();
    let command = build_verify_command(cwd, cmd, sandbox, base_sha);
    let mut result = run_verify_command(cwd, cmd, command, timeout, cancel.cancelled()).await;
    result.commit = commit;
    result.load = load;
    result.cores = cores;
    crate::verify::test_lists::stash_lists(&ctx.app.data_dir, &mut result);
    result
}

#[cfg(test)]
pub(in crate::engine) async fn run_checks(
    runner: CheckRunner<'_>,
    commands: &[String],
) -> (Vec<VerifyOutcome>, Vec<String>) {
    runner.run(commands).await
}

#[cfg(test)]
#[path = "runner/tests.rs"]
mod tests;

pub(in crate::engine) async fn run_one_verify_command(
    runner: CheckRunner<'_>,
    cmd: &str,
) -> VerifyOutcome {
    runner
        .run_one(cmd, runner.ctx.label.unwrap_or(cmd), None)
        .await
        .0
}

async fn acquire_check_slot(
    runner: &CheckRunner<'_>,
    cmd: &str,
    decisions: &mut Vec<String>,
) -> Option<crate::engine::slots::SlotPermit> {
    let CheckRunner { ctx, cancel, .. } = *runner;
    match ctx.app.check_slots.clone().try_acquire_owned() {
        Ok(p) => Some(p),
        Err(_) => {
            let (running, limit) = ctx.app.check_slots.usage();
            ctx.decision(
                format!(
                    "Orchestrator: waiting for a check slot ({running} of {limit}): {}",
                    ctx.label.unwrap_or(cmd)
                ),
                decisions,
            );
            tokio::select! {
                p = ctx.app.check_slots.clone().acquire_owned() => p.ok(),
                _ = cancel.cancelled() => None,
            }
        }
    }
}

pub(in crate::engine) async fn passed_on_tree(
    ctx: CheckContext<'_>,
    cwd: &Path,
    command: &str,
    base: &str,
) -> Option<VerifyOutcome> {
    let tree = check_tree(cwd).await?;
    let mut passed = ctx
        .app
        .gate_passes
        .lock()
        .unwrap()
        .get(&tree, command, base)
        .cloned()?;
    let mut decisions = Vec::new();
    ctx.decision(
        format!(
            "Orchestrator: {} already passed on tree {}; not run again",
            ctx.label.unwrap_or(command),
            short_sha(&tree)
        ),
        &mut decisions,
    );
    passed.decisions = decisions;
    passed.ms = 0;
    Some(passed)
}

#[cfg(test)]
#[path = "runner/provenance_tests.rs"]
mod provenance_tests;
