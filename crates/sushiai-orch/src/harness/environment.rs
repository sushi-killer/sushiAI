use crate::policy::routing::Harness;
pub(crate) fn resolve_binary(harness: Harness) -> String {
    match harness {
        Harness::Claude => {
            std::env::var("ORCHD_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_string())
        }
        Harness::Codex => std::env::var("ORCHD_CODEX_BIN").unwrap_or_else(|_| "codex".to_string()),
    }
}

/// The PATH every command orchd runs gets: its own, then the owner's login
/// shell's (a GUI app or an ssh-started daemon never sees `~/.zshrc`, where
/// toolchains like cargo put themselves), then the usual install dirs.
/// Worked out once per process.
pub(crate) fn augmented_path() -> String {
    static PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let home = std::env::var("HOME").unwrap_or_default();
        let shell = std::env::var("SHELL").unwrap_or_default();
        let login = login_shell_path(&shell, std::time::Duration::from_secs(5));
        merge_paths(
            &std::env::var("PATH").unwrap_or_default(),
            login.as_deref().unwrap_or(""),
            &home,
        )
    })
    .clone()
}

pub(crate) fn merge_paths(own: &str, login: &str, home: &str) -> String {
    let mut extra = vec![format!("{home}/.local/bin"), format!("{home}/.cargo/bin")];
    #[cfg(target_os = "macos")]
    extra.push("/opt/homebrew/bin".to_string());
    extra.push("/usr/local/bin".to_string());
    let mut parts: Vec<String> = Vec::new();
    for dir in own
        .split(':')
        .chain(login.split(':'))
        .map(str::to_string)
        .chain(extra)
    {
        if !dir.is_empty() && !parts.contains(&dir) {
            parts.push(dir);
        }
    }
    parts.join(":")
}

/// `$SHELL -ilc` printing its PATH after a marker (rc files may print
/// banners). `None` when there is no shell, it fails, or it takes longer
/// than `limit` (a prompt waiting for input). ponytail: stdout is read after
/// the shell exits, so an rc file printing more than a pipe buffer would
/// hit the time limit instead.
pub(crate) fn login_shell_path(shell: &str, limit: std::time::Duration) -> Option<String> {
    use std::io::Read;
    if shell.is_empty() {
        return None;
    }
    let mut child = std::process::Command::new(shell)
        .args(["-ilc", "printf '\\n__ORCHD_PATH__%s\\n' \"$PATH\""])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(_) => break,
            None if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    out.lines()
        .find_map(|l| l.strip_prefix("__ORCHD_PATH__"))
        .map(str::to_string)
        .filter(|p| !p.is_empty())
}
