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

/// Runs a command in `cwd` with a timeout; stdout on success, stderr in the error.
pub(crate) async fn run_cmd(
    program: &str,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
) -> Result<String, String> {
    let shown = format!("{program} {}", args.first().copied().unwrap_or(""));
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GH_PROMPT_DISABLED", "1")
        .kill_on_drop(true);
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| format!("{shown} timed out after {}s", timeout.as_secs()))?
        .map_err(|e| format!("{shown}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{shown} failed: {}", err.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
