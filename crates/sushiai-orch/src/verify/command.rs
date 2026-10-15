use crate::harness::compiler_cache::CompilerCache;
use crate::harness::environment::augmented_path;
use crate::sandbox::to_sbpl;
use crate::sandbox::TaskSandbox;
use crate::task::attempt::VerifyOutcome;
use crate::verify::output::timed_out_tail;
use crate::verify::output::verify_tail;
use crate::verify::output::CANCELLED_TAIL;
use crate::verify::output::INTERNAL_ERROR_PREFIX;
use crate::verify::output::SPAWN_FAILED_PREFIX;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// PATH and compiler-cache inputs shared by a verify command and its stats.
pub struct VerifyEnvironment {
    pub path: String,
    pub compiler_cache: Option<CompilerCache>,
}

impl VerifyEnvironment {
    pub fn for_worktree(worktree: &Path) -> Self {
        let path = augmented_path();
        let compiler_cache = CompilerCache::for_worktree(worktree, &path);
        Self {
            path,
            compiler_cache,
        }
    }

    pub fn with_cache(
        worktree: &Path,
        path: String,
        dir: PathBuf,
        port: u16,
        owner_set_wrapper: bool,
    ) -> Self {
        let compiler_cache = CompilerCache::discover(
            &path,
            crate::worktree_env::compiler_cache_enabled(worktree).unwrap_or(false),
            owner_set_wrapper,
            dir,
            port,
        );
        Self {
            path,
            compiler_cache,
        }
    }

    pub fn build_command(
        &self,
        cwd: &Path,
        cmd: &str,
        sandbox: Option<&TaskSandbox>,
        base_sha: &str,
    ) -> tokio::process::Command {
        build_verify_command_in(cwd, cmd, sandbox, base_sha, self)
    }
}

/// Worktree setup uses this timeout; check runners read their App settings.
/// Updated at daemon start and on every `settings.set`.
static VERIFY_TIMEOUT_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1200);

/// The most the Stop hook waits for verify: under Claude's 600 s hook limit.
const HOOK_BUDGET_CAP_SECS: u64 = 540;

/// The Stop hook's verify budget: the verify timeout, capped.
pub(crate) fn hook_budget_secs(verify_timeout_secs: u64) -> u64 {
    verify_timeout_secs.clamp(1, HOOK_BUDGET_CAP_SECS)
}

pub(crate) fn set_verify_timeout_secs(secs: u64) {
    VERIFY_TIMEOUT_SECS.store(secs.max(1), Ordering::SeqCst);
}

pub(crate) fn verify_timeout() -> Duration {
    Duration::from_secs(VERIFY_TIMEOUT_SECS.load(Ordering::SeqCst))
}

/// `sandbox-exec`-wraps the command on macOS when `sandbox == Native`;
/// plain `/bin/sh -c` for `Host`, and on non-macOS (`sandbox-exec` doesn't
/// exist there -- a documented limitation, not a bug).
pub(crate) fn build_verify_command(
    cwd: &Path,
    cmd: &str,
    sandbox: Option<&TaskSandbox>,
    base_sha: &str,
) -> tokio::process::Command {
    VerifyEnvironment::for_worktree(cwd).build_command(cwd, cmd, sandbox, base_sha)
}

fn build_verify_command_in(
    cwd: &Path,
    cmd: &str,
    sandbox: Option<&TaskSandbox>,
    base_sha: &str,
    environment: &VerifyEnvironment,
) -> tokio::process::Command {
    let mut command;
    #[cfg(target_os = "macos")]
    {
        if let Some(sandbox) = sandbox {
            let profile = to_sbpl(sandbox);
            command = tokio::process::Command::new("sandbox-exec");
            command
                .arg("-p")
                .arg(profile)
                .arg("/bin/sh")
                .arg("-c")
                .arg(cmd);
        } else {
            command = tokio::process::Command::new("/bin/sh");
            command.arg("-c").arg(cmd);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = sandbox;
        command = tokio::process::Command::new("/bin/sh");
        command.arg("-c").arg(cmd);
    }
    if let Some(sandbox) = sandbox {
        // The sandbox policy owns the sandboxed-check variables.
        sandbox.apply_check_env(command.as_std_mut());
    } else if let Some(cache) = &environment.compiler_cache {
        cache.apply(&mut command);
    }
    command
        .current_dir(cwd)
        .env("PATH", &environment.path)
        .env("ORCHD_BASE_SHA", base_sha)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    command
}

/// Concurrently drain a child's stdout/stderr and wait for it to exit,
/// without moving `child` -- so the caller still owns it (and can kill its
/// process group) if this gets dropped by an outer timeout/cancellation.
async fn read_and_wait(
    child: &mut tokio::process::Child,
) -> std::io::Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)> {
    use tokio::io::AsyncReadExt;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();
    let (status, _, _) = tokio::try_join!(
        child.wait(),
        stdout.read_to_end(&mut out_buf),
        stderr.read_to_end(&mut err_buf),
    )?;
    Ok((status, out_buf, err_buf))
}

/// Runs an already-built check with the same timeout and process cleanup.
pub async fn run_verify_command(
    cwd: &Path,
    cmd: &str,
    mut command: tokio::process::Command,
    timeout: Duration,
    cancelled: impl std::future::Future<Output = ()>,
) -> VerifyOutcome {
    let start = std::time::Instant::now();
    let running = crate::procs::running_in(cwd);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return VerifyOutcome {
                command: cmd.to_string(),
                code: None,
                tail: format!("{SPAWN_FAILED_PREFIX}{e}"),
                ms: start.elapsed().as_millis() as u64,
                ..Default::default()
            };
        }
    };
    let pgid = child.id().map(|p| p as i32);

    enum Outcome {
        Done(std::io::Result<(std::process::ExitStatus, Vec<u8>, Vec<u8>)>),
        TimedOut,
        Cancelled,
    }
    let outcome = tokio::select! {
        r = read_and_wait(&mut child) => Outcome::Done(r),
        _ = tokio::time::sleep(timeout) => Outcome::TimedOut,
        _ = cancelled => Outcome::Cancelled,
    };
    // Whatever the outcome, nothing the command left in its group outlives it.
    crate::procs::kill_group(pgid, &mut child).await;
    // Processes that left the group (`setsid`) but run from this directory. After a
    // normal exit only orphans, and only when no sibling check still runs here.
    let dir = cwd.to_path_buf();
    if matches!(outcome, Outcome::Done(_)) {
        if running.alone() {
            let _ =
                tokio::task::spawn_blocking(move || crate::procs::stop_orphans_under(&dir)).await;
        }
    } else {
        let _ = tokio::task::spawn_blocking(move || crate::procs::stop_processes_under(&dir)).await;
    }
    let mut full = String::new();
    let (code, tail) = match outcome {
        Outcome::Done(Ok((status, out_buf, err_buf))) => {
            if status.code() != Some(0) {
                full = format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&out_buf),
                    String::from_utf8_lossy(&err_buf)
                );
            }
            (status.code(), verify_tail(&out_buf, &err_buf))
        }
        Outcome::Done(Err(e)) => (None, format!("{INTERNAL_ERROR_PREFIX}{e}")),
        Outcome::TimedOut => (None, timed_out_tail(timeout)),
        Outcome::Cancelled => (None, CANCELLED_TAIL.to_string()),
    };
    // The tail is truncated; the baseline diff reads the whole output.
    let report = if full.is_empty() {
        Default::default()
    } else {
        crate::test_failures::parse(&full, &cwd.to_string_lossy())
    };
    VerifyOutcome {
        command: cmd.to_string(),
        code,
        tail,
        failing: report.failing,
        passing: report.passing,
        crashed: report.crashed,
        stopped_early: report.stopped_early,
        output_key: report.rest_key,
        ms: start.elapsed().as_millis() as u64,
        ..Default::default()
    }
}

#[cfg(test)]
#[path = "command/tests.rs"]
mod tests;
