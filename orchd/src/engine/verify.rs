use super::*;

const VERIFY_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Every path a verify command is allowed to write under when sandboxed:
/// the worktree and its own run dir, plus the usual OS/package-manager temp
/// and cache locations a build/test command routinely touches. Network is
/// deliberately left open (verify commands may need to hit a registry, run
/// a dev server, etc.) -- only the filesystem is restricted.
fn verify_allow_write_paths(worktree: &Path, run_dir: &Path) -> Vec<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let tmp_dir = std::env::var("TMPDIR").unwrap_or_default();
    let mut paths = vec![
        worktree.to_path_buf(),
        run_dir.to_path_buf(),
        PathBuf::from("/private/var/folders"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/dev"),
        PathBuf::from(format!("{home}/Library/Caches")),
        PathBuf::from(format!("{home}/.npm")),
        PathBuf::from(format!("{home}/.cache")),
        PathBuf::from(format!("{home}/.cargo/registry")),
    ];
    if !tmp_dir.is_empty() {
        paths.push(PathBuf::from(tmp_dir));
    }
    paths
}

#[cfg(target_os = "macos")]
fn build_verify_sandbox_profile(allow_write: &[PathBuf]) -> String {
    let mut profile =
        String::from("(version 1)\n(allow default)\n(deny file-write* (subpath \"/\"))\n");
    for p in allow_write {
        profile.push_str(&format!(
            "(allow file-write* (subpath \"{}\"))\n",
            p.to_string_lossy().replace('"', "")
        ));
    }
    profile
}

/// `sandbox-exec`-wraps the command on macOS when `sandbox == Native`;
/// plain `/bin/sh -c` for `Host`, and on non-macOS (`sandbox-exec` doesn't
/// exist there -- a documented limitation, not a bug).
fn build_verify_command(
    cwd: &Path,
    cmd: &str,
    sandbox: SandboxMode,
    allow_write: &[PathBuf],
) -> tokio::process::Command {
    let mut command;
    #[cfg(target_os = "macos")]
    {
        if sandbox == SandboxMode::Native {
            let profile = build_verify_sandbox_profile(allow_write);
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
        let _ = (sandbox, allow_write);
        command = tokio::process::Command::new("/bin/sh");
        command.arg("-c").arg(cmd);
    }
    command
        .current_dir(cwd)
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

pub(super) async fn run_verify_commands(
    cwd: &Path,
    run_dir: &Path,
    commands: &[String],
    sandbox: SandboxMode,
    cancel: &CancelToken,
) -> Vec<VerifyOutcome> {
    let allow_write = verify_allow_write_paths(cwd, run_dir);
    let mut results = Vec::new();
    for cmd in commands {
        results.push(
            run_one_verify_command(cwd, cmd, VERIFY_TIMEOUT, sandbox, &allow_write, cancel).await,
        );
    }
    results
}

/// Cached by a hash of `git diff <base>` + the untracked-file list: if
/// nothing has changed since the last run (by the hook or the previous
/// gate check), reuse its result instead of re-running the commands.
pub(super) async fn run_verify_cached(
    app: &Arc<App>,
    task_id: &str,
    worktree: &Path,
    run_dir: &Path,
    base: &str,
    commands: &[String],
    cancel: &CancelToken,
) -> Vec<VerifyOutcome> {
    let wt = worktree.to_path_buf();
    let b = base.to_string();
    let hash = tokio::task::spawn_blocking(move || diff_hash(&wt, &b))
        .await
        .unwrap_or_default();
    {
        let cache = app.verify_cache.lock().unwrap();
        if let Some((h, results)) = cache.get(task_id) {
            if *h == hash {
                return results.clone();
            }
        }
    }
    let sandbox = app.settings.read().unwrap().sandbox;
    let results = run_verify_commands(worktree, run_dir, commands, sandbox, cancel).await;
    app.verify_cache
        .lock()
        .unwrap()
        .insert(task_id.to_string(), (hash, results.clone()));
    results
}

/// Regular files only (a symlink could point at `/dev/zero` or outside the
/// worktree), and at most the first 1 MiB plus the length, so a huge
/// untracked artifact can't stall the daemon.
fn untracked_fingerprint(path: &Path) -> String {
    use std::io::Read;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => {
            let mut head = Vec::new();
            if let Ok(file) = std::fs::File::open(path) {
                let _ = file.take(1 << 20).read_to_end(&mut head);
            }
            format!("{}:{}", meta.len(), String::from_utf8_lossy(&head))
        }
        Ok(meta) => format!("{:?}", meta.file_type()),
        Err(_) => String::new(),
    }
}

pub(super) fn diff_hash(worktree: &Path, base: &str) -> String {
    let diff = git::diff_full(worktree, base, usize::MAX).unwrap_or_default();
    let untracked = git::status_porcelain(worktree).unwrap_or_default();
    // The base is part of the key: after the work is carried onto a moved
    // base, `git diff` of the task's own files can be byte-identical while
    // the tree that would land is not.
    let mut input = format!("{base}\u{0}{diff}");
    input.push_str("\u{0}untracked\u{0}");
    // Contents too: `git diff` never shows an untracked file, so hashing
    // only its name reused a stale failing result after the agent fixed a
    // file it had created.
    for u in &untracked {
        input.push_str(u);
        input.push('\u{0}');
        input.push_str(&untracked_fingerprint(&worktree.join(u)));
        input.push('\u{0}');
    }
    simple_hash(&input)
}

/// Shared by verify commands and the harness: SIGTERM the process group,
/// give it 5s, then SIGKILL, then reap it.
pub(super) async fn kill_group(pgid: Option<i32>, child: &mut tokio::process::Child) {
    if let Some(pgid) = pgid {
        unsafe {
            libc::killpg(pgid, libc::SIGTERM);
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    let _ = child.wait().await;
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

/// Planners sometimes put a check only a person or the reviewer can do into
/// `verify` ("screenshot the panel", "npm run x (only if ...)"); run as a
/// shell command it fails every attempt forever. An entry that doesn't
/// parse as shell, or whose program doesn't exist here, becomes a criterion
/// for the reviewer instead. Returns `(runnable, for_review)`.
pub(super) fn split_verify_commands(cwd: &Path, commands: &[String]) -> (Vec<String>, Vec<String>) {
    let runs = |args: &[&str]| {
        std::process::Command::new("/bin/sh")
            .args(args)
            .current_dir(cwd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    commands.iter().cloned().partition(|cmd| {
        if !runs(&["-n", "-c", cmd]) {
            return false;
        }
        // Only a plain leading word is checked: a path may be created by
        // the task itself, and grouping/quoting is left to `sh -n` above.
        match cmd
            .split_whitespace()
            .find(|w| !w.contains('=') && !matches!(*w, "(" | "{" | "!"))
        {
            Some(p)
                if p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) =>
            {
                runs(&["-c", "command -v \"$1\"", "sh", p])
            }
            _ => true,
        }
    })
}

/// Lines a test runner prints for a failure: cargo's `... FAILED` and
/// `panicked at`, node's `not ok` / `AssertionError`, and the assertion
/// detail lines that follow a panic.
fn is_failure_line(line: &str) -> bool {
    let t = line.trim();
    t.ends_with("FAILED")
        || t.starts_with("not ok")
        || t.starts_with("---- ")
        || t.contains("panicked at")
        || t.contains("AssertionError")
        || t.starts_with("assertion ")
        || t.starts_with("left:")
        || t.starts_with("right:")
        || t.starts_with("error:")
        || t.starts_with("Error:")
}

/// The failing tests' names and assertion lines from both streams, keeping
/// the last lines when there are more than fit.
fn failure_highlights(out: &str, err: &str, max: usize) -> String {
    let lines: Vec<&str> = out
        .lines()
        .chain(err.lines())
        .filter(|l| is_failure_line(l))
        .collect();
    tail_chars(&lines.join("\n"), max)
}

/// Each stream's own tail plus the failure lines found anywhere in either:
/// concatenated, a noisy stderr (cargo's compile log) pushed stdout's end --
/// where test runners list what failed -- out of the kept window, so a retry
/// never saw which test broke. The failure lines come last because the
/// consumers (brief, Stop hook) keep the end of the detail.
pub(super) fn verify_tail(stdout: &[u8], stderr: &[u8]) -> String {
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    let highlights = failure_highlights(&out, &err, 1000);
    let mut body = match (out.trim().is_empty(), err.trim().is_empty()) {
        (true, true) => String::new(),
        (true, _) => tail_chars(&err, 3000),
        (_, true) => tail_chars(&out, 3000),
        _ => format!(
            "--- stderr (tail) ---\n{}\n--- stdout (tail) ---\n{}",
            tail_chars(&err, 900),
            tail_chars(&out, 2100)
        ),
    };
    if !highlights.is_empty() {
        body.push_str("\n--- failures ---\n");
        body.push_str(&highlights);
    }
    body
}

/// Run one verify command in its own process group (`setsid`, like the
/// harness). On timeout *or* cancellation, SIGTERM the whole group, SIGKILL
/// 5s later -- a bare `tokio::time::timeout` around `Command::output()`
/// only stops *waiting*, it never touches the still-running process (or any
/// children it spawned), which is the bug this replaces.
pub(super) async fn run_one_verify_command(
    cwd: &Path,
    cmd: &str,
    timeout: Duration,
    sandbox: SandboxMode,
    allow_write: &[PathBuf],
    cancel: &CancelToken,
) -> VerifyOutcome {
    let start = std::time::Instant::now();
    let mut command = build_verify_command(cwd, cmd, sandbox, allow_write);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return VerifyOutcome {
                command: cmd.to_string(),
                code: None,
                tail: format!("failed to run: {e}"),
                ms: start.elapsed().as_millis() as u64,
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
        _ = cancel.cancelled() => Outcome::Cancelled,
    };
    match outcome {
        Outcome::Done(Ok((status, out_buf, err_buf))) => VerifyOutcome {
            command: cmd.to_string(),
            code: status.code(),
            tail: verify_tail(&out_buf, &err_buf),
            ms: start.elapsed().as_millis() as u64,
        },
        Outcome::Done(Err(e)) => VerifyOutcome {
            command: cmd.to_string(),
            code: None,
            tail: format!("internal error: {e}"),
            ms: start.elapsed().as_millis() as u64,
        },
        Outcome::TimedOut => {
            kill_group(pgid, &mut child).await;
            VerifyOutcome {
                command: cmd.to_string(),
                code: None,
                tail: "timed out after 20 minutes".to_string(),
                ms: start.elapsed().as_millis() as u64,
            }
        }
        Outcome::Cancelled => {
            kill_group(pgid, &mut child).await;
            VerifyOutcome {
                command: cmd.to_string(),
                code: None,
                tail: "cancelled".to_string(),
                ms: start.elapsed().as_millis() as u64,
            }
        }
    }
}

/// Suggested quick-reply options for the synthesized "no verification
/// command" question: each of the target repo's own `package.json` scripts,
/// as `npm run <script>`, `test`-named scripts ranked first (they're by far
/// the most likely answer). Empty (not an error) when there's no
/// `package.json` or no `scripts` -- the question still accepts free text.
pub(super) fn verify_options_from_package_json(worktree: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(worktree.join("package.json")) else {
        return vec![];
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![];
    };
    let Some(scripts) = v.get("scripts").and_then(|s| s.as_object()) else {
        return vec![];
    };
    let mut names: Vec<&String> = scripts.keys().collect();
    names.sort_by_key(|k| (!k.to_ascii_lowercase().contains("test"), k.as_str()));
    names
        .into_iter()
        .take(4)
        .map(|k| format!("npm run {k}"))
        .collect()
}
