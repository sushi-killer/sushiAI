use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

pub(crate) const GIT_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) const PUSH_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// The `gh` binary; `ORCHD_GH_BIN` lets a test point it at a fake script.
pub(crate) fn gh_bin() -> String {
    std::env::var("ORCHD_GH_BIN")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "gh".to_string())
}

/// A command dispatched synchronously, whose bounded result is awaited separately.
pub(crate) struct CommandWrite {
    child: tokio::process::Child,
    shown: String,
    timeout: Duration,
    deadline: tokio::time::Instant,
}

/// Dispatches immediately so an engine admission lock can cover the actual write.
pub(crate) fn start(
    program: &str,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> Result<CommandWrite, String> {
    let shown = format!("{program} {}", args.first().copied().unwrap_or(""));
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .kill_on_drop(true);
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let deadline = tokio::time::Instant::now() + timeout;
    let child = cmd.spawn().map_err(|error| format!("{shown}: {error}"))?;
    Ok(CommandWrite {
        child,
        shown,
        timeout,
        deadline,
    })
}

impl CommandWrite {
    pub(crate) async fn finish(self) -> Result<String, String> {
        let Self {
            child,
            shown,
            timeout,
            deadline,
        } = self;
        let out = tokio::time::timeout_at(deadline, child.wait_with_output())
            .await
            .map_err(|_| format!("{shown} timed out after {}s", timeout.as_secs()))?
            .map_err(|error| format!("{shown}: {error}"))?;
        if !out.status.success() {
            let error = String::from_utf8_lossy(&out.stderr);
            return Err(format!("{shown} failed: {}", error.trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

/// Runs a command in `cwd` with a timeout; stdout on success, stderr in the error.
pub(crate) async fn run_cmd(
    program: &str,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> Result<String, String> {
    start(program, args, cwd, timeout)?.finish().await
}
