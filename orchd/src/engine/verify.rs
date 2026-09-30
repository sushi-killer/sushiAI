use super::*;

/// `settings.verifyTimeoutSecs`, set when the daemon starts and on every
/// `settings.set`; the verify runners are free functions with no `App`.
static VERIFY_TIMEOUT_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1200);

/// The most the Stop hook waits for verify: under Claude's 600 s hook limit.
const HOOK_BUDGET_CAP_SECS: u64 = 540;

/// The Stop hook's verify budget: the verify timeout, capped.
pub(super) fn hook_budget_secs(verify_timeout_secs: u64) -> u64 {
    verify_timeout_secs.clamp(1, HOOK_BUDGET_CAP_SECS)
}

pub(super) fn set_verify_timeout_secs(secs: u64) {
    VERIFY_TIMEOUT_SECS.store(secs.max(1), Ordering::SeqCst);
}

fn verify_timeout() -> Duration {
    Duration::from_secs(VERIFY_TIMEOUT_SECS.load(Ordering::SeqCst))
}

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
        PathBuf::from("/dev"),
        PathBuf::from(format!("{home}/.npm")),
        PathBuf::from(format!("{home}/.cache")),
        PathBuf::from(format!("{home}/.cargo/registry")),
    ];
    #[cfg(target_os = "macos")]
    paths.extend([
        PathBuf::from("/private/var/folders"),
        PathBuf::from("/private/tmp"),
        PathBuf::from(format!("{home}/Library/Caches")),
    ]);
    #[cfg(not(target_os = "macos"))]
    paths.extend([PathBuf::from("/tmp"), PathBuf::from("/var/tmp")]);
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
    base_sha: &str,
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

/// The entries that scope `command` in `repo`.
fn scoping_entries<'a>(
    scoped: &'a [ScopedCheck],
    repo: &str,
    command: &str,
) -> Vec<&'a ScopedCheck> {
    scoped
        .iter()
        .filter(|e| {
            !e.paths.is_empty()
                && e.repo.as_deref().is_none_or(|r| r == repo)
                && glob_match(&e.command, command.trim())
        })
        .collect()
}

/// The commands in `commands` that stay after `scoped` entries are applied
/// to `changed` (repo-relative files), and one decision line per skipped
/// command. A command an entry matches, where no changed file matches the
/// entry's paths, is skipped; an entry without paths never scopes.
pub(super) fn filter_scoped(
    scoped: &[ScopedCheck],
    repo: &str,
    commands: &[String],
    changed: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut run = Vec::new();
    let mut lines = Vec::new();
    for command in commands {
        let entries = scoping_entries(scoped, repo, command);
        let touched = entries
            .iter()
            .any(|e| changed.iter().any(|f| matches_any_protected(f, &e.paths)));
        if entries.is_empty() || touched {
            run.push(command.clone());
            continue;
        }
        let mut paths: Vec<&str> = Vec::new();
        for e in &entries {
            for p in &e.paths {
                if !paths.contains(&p.as_str()) {
                    paths.push(p);
                }
            }
        }
        lines.push(format!(
            "Orchestrator: skipped {command}: no change under {}",
            paths.join(", ")
        ));
    }
    (run, lines)
}

/// Like [`filter_scoped`] for the task's own diff against `base`. When the
/// diff cannot be read every command runs.
pub(super) async fn scope_to_diff(
    app: &Arc<App>,
    repo: &str,
    worktree: &Path,
    base: &str,
    commands: &[String],
) -> (Vec<String>, Vec<String>) {
    let scoped = app.settings.read().unwrap().scoped_checks.clone();
    if !commands
        .iter()
        .any(|c| scoped.iter().any(|e| glob_match(&e.command, c.trim())))
    {
        return (commands.to_vec(), Vec::new());
    }
    let (wt, b) = (worktree.to_path_buf(), base.to_string());
    let changed = tokio::task::spawn_blocking(move || git::changed_files(&wt, &b).ok())
        .await
        .unwrap_or(None);
    match changed {
        Some(changed) => filter_scoped(&scoped, repo, commands, &changed),
        None => (commands.to_vec(), Vec::new()),
    }
}

/// Adds each skip line once.
pub(super) fn record_skips(task: &mut Task, lines: Vec<String>) {
    for line in lines {
        if !task.decisions.contains(&line) {
            task.decisions.push(line);
        }
    }
}

/// Whether a planned path (a file or a directory) can hold a file `glob`
/// matches.
fn planned_path_in_scope(glob: &str, planned: &str) -> bool {
    let planned = planned.trim_start_matches("./").trim_end_matches('/');
    if planned.is_empty() || glob_match(glob, planned) {
        return true;
    }
    // A glob with no literal prefix (`*.html`) falls under no directory.
    let literal: &str = glob.split(['*', '?']).next().unwrap_or("");
    !literal.is_empty() && literal.starts_with(&format!("{planned}/"))
}

/// The base preflight has no diff, so `planned` (the task's `paths`) stands
/// in for it; it runs every command when `planned` is empty.
pub(super) fn filter_scoped_planned(
    scoped: &[ScopedCheck],
    repo: &str,
    commands: &[String],
    planned: &[String],
) -> Vec<String> {
    if planned.is_empty() {
        return commands.to_vec();
    }
    commands
        .iter()
        .filter(|command| {
            let entries = scoping_entries(scoped, repo, command);
            entries.is_empty()
                || entries.iter().any(|e| {
                    e.paths
                        .iter()
                        .any(|g| planned.iter().any(|p| planned_path_in_scope(g, p)))
                })
        })
        .cloned()
        .collect()
}

pub(super) async fn run_verify_commands(
    cwd: &Path,
    run_dir: &Path,
    commands: &[String],
    sandbox: SandboxMode,
    base_sha: &str,
    cancel: &CancelToken,
) -> Vec<VerifyOutcome> {
    let allow_write = verify_allow_write_paths(cwd, run_dir);
    let mut results = Vec::new();
    for cmd in commands {
        results.push(
            run_one_verify_command(
                cwd,
                cmd,
                verify_timeout(),
                sandbox,
                &allow_write,
                base_sha,
                cancel,
            )
            .await,
        );
    }
    results
}

/// Grades an eval task: runs its set's `check` from the root of a throwaway
/// detached worktree at `sha` (never the task's own), with the verify timeout.
pub(super) async fn run_eval_check(
    repo: &Path,
    run_dir: &Path,
    sha: &str,
    cmd: &str,
    sandbox: SandboxMode,
    cancel: &CancelToken,
) -> EvalCheck {
    let path = run_dir.join("eval-check");
    let failed = |tail: String| EvalCheck { code: None, tail };
    let (r, p, s) = (repo.to_path_buf(), path.clone(), sha.to_string());
    let made = tokio::task::spawn_blocking(move || {
        git::remove_worktree(&r, &p);
        git::add_detached_worktree(&r, &p, &s)?;
        let _ = git::bootstrap_worktree(&r, &p);
        Ok::<(), git::GitError>(())
    })
    .await;
    match made {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return failed(format!("could not check out {sha}: {e}")),
        Err(e) => return failed(format!("check checkout panicked: {e}")),
    }
    let allow_write = verify_allow_write_paths(&path, run_dir);
    let out = run_one_verify_command(
        &path,
        cmd,
        verify_timeout(),
        sandbox,
        &allow_write,
        sha,
        cancel,
    )
    .await;
    let (r, p) = (repo.to_path_buf(), path);
    let _ = tokio::task::spawn_blocking(move || git::remove_worktree(&r, &p)).await;
    EvalCheck {
        code: out.code,
        tail: out.tail,
    }
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
    let results = run_verify_commands(worktree, run_dir, commands, sandbox, base, cancel).await;
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
        || t.starts_with("FAIL")
        || eslint_summary_has_errors(t)
}

/// eslint's `✖ N problems (M errors, ...)` with M > 0.
fn eslint_summary_has_errors(t: &str) -> bool {
    let Some(rest) = t.strip_prefix('\u{2716}') else {
        return false;
    };
    let Some((_, after)) = rest.split_once("problem") else {
        return false;
    };
    let Some((_, counts)) = after.split_once('(') else {
        return false;
    };
    let digits: String = counts.chars().take_while(char::is_ascii_digit).collect();
    counts[digits.len()..].starts_with(" error") && digits.parse::<u32>().is_ok_and(|n| n > 0)
}

/// The failure lines of one stream. A `Repository conventions failed` line
/// starts a block whose `- ` bullets (after blank lines) are kept too.
fn stream_highlights(text: &str) -> Vec<&str> {
    let mut kept = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        if in_block {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if t.starts_with("- ") {
                kept.push(line);
                continue;
            }
            in_block = false;
        }
        if line.contains("Repository conventions failed") {
            in_block = true;
            kept.push(line);
        } else if is_failure_line(line) {
            kept.push(line);
        }
    }
    kept
}

/// The failing tests' names and assertion lines from both streams, keeping
/// the last lines when there are more than fit.
fn failure_highlights(out: &str, err: &str, max: usize) -> String {
    let mut lines = stream_highlights(out);
    lines.extend(stream_highlights(err));
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

pub(super) fn timed_out_tail(timeout: Duration) -> String {
    let secs = timeout.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("timed out after {} minutes", secs / 60)
    } else {
        format!("timed out after {secs} seconds")
    }
}
const CANCELLED_TAIL: &str = "cancelled";
const SPAWN_FAILED_PREFIX: &str = "failed to run: ";
const INTERNAL_ERROR_PREFIX: &str = "internal error: ";

/// How a check ran on the base checkout. `None` for a cancel, which aborts
/// the baseline instead of classifying anything.
pub(super) fn classify_baseline(outcome: &VerifyOutcome) -> Option<Baseline> {
    match outcome.code {
        Some(0) => Some(Baseline::Pass),
        Some(126 | 127) => Some(Baseline::Env),
        Some(_) => Some(Baseline::Fail),
        None if outcome.tail == CANCELLED_TAIL => None,
        None if outcome.tail.starts_with(SPAWN_FAILED_PREFIX)
            || outcome.tail.starts_with(INTERNAL_ERROR_PREFIX) =>
        {
            Some(Baseline::Env)
        }
        // A timeout is a check that does not pass.
        None => Some(Baseline::Fail),
    }
}

/// Runs the checks that have no baseline yet in `worktree` and stores what
/// each one did on the base: `pass` (proves nothing), `fail` (gates), `env`
/// (could not run). Whatever the commands changed in the worktree is undone.
/// `false` when cancelled, with nothing stored.
pub(super) async fn baseline_checks(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    worktree: &Path,
    cancel: &CancelToken,
) -> bool {
    let todo = baseline_todo(task);
    if todo.is_empty() {
        return true;
    }
    let wt = worktree.to_path_buf();
    let before = tokio::task::spawn_blocking(move || git::status_entries(&wt).unwrap_or_default())
        .await
        .unwrap_or_default();
    let commands: Vec<String> = todo.iter().map(|(_, _, run)| run.clone()).collect();
    let sandbox = app.settings.read().unwrap().sandbox;
    let results = run_verify_commands(
        worktree,
        &app.store.task_dir(task_id),
        &commands,
        sandbox,
        &task.base_sha,
        cancel,
    )
    .await;
    let wt = worktree.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || git::restore_status(&wt, &before)).await;
    if cancel.is_cancelled() {
        return false;
    }
    record_baselines(task, &todo, &results)
}

/// (index into task.checks, or None for the held-out check, criterion,
/// command) for every check that has no baseline yet.
pub(super) fn baseline_todo(task: &Task) -> Vec<(Option<usize>, usize, String)> {
    let mut todo = Vec::new();
    for (i, c) in task.checks.iter().enumerate() {
        if c.baseline.is_none() {
            todo.push((Some(i), c.criterion, c.run.clone()));
        }
    }
    if let Some(h) = task.held_out.as_ref().filter(|h| h.baseline.is_none()) {
        todo.push((None, h.criterion, h.run.clone()));
    }
    todo
}

/// Stores what each `todo` check did on the base (`results` in the same
/// order) and notes the ones that prove nothing. `false` when a result is a
/// cancel: nothing is stored then.
pub(super) fn record_baselines(
    task: &mut Task,
    todo: &[(Option<usize>, usize, String)],
    results: &[VerifyOutcome],
) -> bool {
    let classified: Vec<Option<Baseline>> = results.iter().map(classify_baseline).collect();
    if classified.iter().any(Option::is_none) {
        return false;
    }
    for (((slot, criterion, run), result), baseline) in todo.iter().zip(results).zip(classified) {
        let baseline = baseline.expect("checked above");
        let shown = match slot {
            Some(_) => run.clone(),
            None => brief::held_out_label(*criterion),
        };
        match baseline {
            Baseline::Pass => task.decisions.push(format!(
                "Orchestrator: check for criterion {criterion} already passes on base, not grounded: {shown}"
            )),
            Baseline::Env => task.decisions.push(format!(
                "Orchestrator: check for criterion {criterion} could not run on base (exit {}), left out: {shown}",
                result
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "none".to_string())
            )),
            Baseline::Fail => {}
        }
        match slot {
            Some(i) => task.checks[*i].baseline = Some(baseline),
            None => {
                if let Some(h) = task.held_out.as_mut() {
                    h.baseline = Some(baseline);
                }
            }
        }
    }
    task.updated_at = now_ms();
    true
}

/// Some check (or the held-out one) has not run on the base yet.
pub(super) fn needs_baseline(task: &Task) -> bool {
    task.checks.iter().any(|c| c.baseline.is_none())
        || task.held_out.as_ref().is_some_and(|h| h.baseline.is_none())
}

/// The gated checks of `task`: visible ones that fail on the base, then the
/// held-out one.
pub(super) fn gated_checks(task: &Task) -> Vec<Check> {
    task.checks
        .iter()
        .filter(|c| c.baseline == Some(Baseline::Fail))
        .cloned()
        .collect()
}

pub(super) fn gated_held_out(task: &Task) -> Option<Check> {
    task.held_out
        .clone()
        .filter(|h| h.baseline == Some(Baseline::Fail))
}

/// Runs the held-out check and returns its outcome with the command scrubbed
/// out of the command field and the tail, plus the failure detail when it
/// did not exit 0.
pub(super) async fn run_held_out(
    task: &Task,
    check: &Check,
    worktree: &Path,
    run_dir: &Path,
    sandbox: SandboxMode,
    cancel: &CancelToken,
) -> (VerifyOutcome, Option<String>) {
    let mut result = run_verify_commands(
        worktree,
        run_dir,
        std::slice::from_ref(&check.run),
        sandbox,
        &task.base_sha,
        cancel,
    )
    .await
    .remove(0);
    result.command = brief::held_out_label(check.criterion);
    result.tail = result.tail.replace(&check.run, "<held-out check>");
    let detail = (result.code != Some(0)).then(|| {
        format!(
            "Held-out check for criterion {} failed: {}. exited {}.\n{}",
            check.criterion,
            task.criteria
                .get(check.criterion)
                .map(String::as_str)
                .unwrap_or(""),
            result
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "null".to_string()),
            result.tail
        )
        .replace(&check.run, "<held-out check>")
    });
    (result, detail)
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
    base_sha: &str,
    cancel: &CancelToken,
) -> VerifyOutcome {
    let start = std::time::Instant::now();
    let mut command = build_verify_command(cwd, cmd, sandbox, allow_write, base_sha);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return VerifyOutcome {
                command: cmd.to_string(),
                code: None,
                tail: format!("{SPAWN_FAILED_PREFIX}{e}"),
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
            tail: format!("{INTERNAL_ERROR_PREFIX}{e}"),
            ms: start.elapsed().as_millis() as u64,
        },
        Outcome::TimedOut => {
            kill_group(pgid, &mut child).await;
            VerifyOutcome {
                command: cmd.to_string(),
                code: None,
                tail: timed_out_tail(timeout),
                ms: start.elapsed().as_millis() as u64,
            }
        }
        Outcome::Cancelled => {
            kill_group(pgid, &mut child).await;
            VerifyOutcome {
                command: cmd.to_string(),
                code: None,
                tail: CANCELLED_TAIL.to_string(),
                ms: start.elapsed().as_millis() as u64,
            }
        }
    }
}

/// The package runner the repo's lockfile names: pnpm, yarn, bun, else npm.
fn package_runner(worktree: &Path) -> &'static str {
    let has = |name: &str| worktree.join(name).exists();
    if has("pnpm-lock.yaml") {
        "pnpm"
    } else if has("yarn.lock") {
        "yarn"
    } else if has("bun.lockb") || has("bun.lock") {
        "bun"
    } else {
        "npm"
    }
}

/// Suggested quick-reply options for the synthesized "no verification
/// command" question: each of the target repo's own `package.json` scripts,
/// as `<runner> run <script>` (the runner follows the lockfile), `test`-named
/// scripts ranked first (they're by far the most likely answer), then the
/// test command of each other toolchain the repo shows: `cargo test`,
/// `go test ./...`, `pytest`, `make test`. Empty (not an error) when nothing
/// is recognised -- the question still accepts free text.
pub(super) fn verify_options(worktree: &Path) -> Vec<String> {
    let mut options: Vec<String> = Vec::new();
    let scripts = std::fs::read_to_string(worktree.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.get("scripts").and_then(|s| s.as_object()).cloned());
    if let Some(scripts) = scripts {
        let runner = package_runner(worktree);
        let mut names: Vec<&String> = scripts.keys().collect();
        names.sort_by_key(|k| (!k.to_ascii_lowercase().contains("test"), k.as_str()));
        options.extend(
            names
                .into_iter()
                .take(4)
                .map(|k| format!("{runner} run {k}")),
        );
    }
    let has = |name: &str| worktree.join(name).exists();
    if has("Cargo.toml") {
        options.push("cargo test".to_string());
    }
    if has("go.mod") {
        options.push("go test ./...".to_string());
    }
    if has("pyproject.toml") || has("pytest.ini") {
        options.push("pytest".to_string());
    }
    let make_test = ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .filter_map(|f| std::fs::read_to_string(worktree.join(f)).ok())
        .any(|text| {
            text.lines().any(|l| {
                l.strip_prefix("test")
                    .is_some_and(|r| r.trim_start().starts_with(':'))
            })
        });
    if make_test {
        options.push("make test".to_string());
    }
    options
}

#[cfg(test)]
mod grounded_checks_tests {
    use super::*;

    fn outcome(code: Option<i32>, tail: &str) -> VerifyOutcome {
        VerifyOutcome {
            command: "c".into(),
            code,
            tail: tail.into(),
            ms: 0,
        }
    }

    #[test]
    fn a_baseline_is_classified_from_the_exit_code_and_the_tail() {
        assert_eq!(
            classify_baseline(&outcome(Some(0), "")),
            Some(Baseline::Pass)
        );
        assert_eq!(
            classify_baseline(&outcome(Some(1), "")),
            Some(Baseline::Fail)
        );
        assert_eq!(
            classify_baseline(&outcome(Some(126), "")),
            Some(Baseline::Env)
        );
        assert_eq!(
            classify_baseline(&outcome(Some(127), "")),
            Some(Baseline::Env)
        );
        assert_eq!(
            classify_baseline(&outcome(
                None,
                &format!("{SPAWN_FAILED_PREFIX}no such file")
            )),
            Some(Baseline::Env)
        );
        assert_eq!(
            classify_baseline(&outcome(None, &format!("{INTERNAL_ERROR_PREFIX}pipe"))),
            Some(Baseline::Env)
        );
        assert_eq!(
            classify_baseline(&outcome(None, &timed_out_tail(Duration::from_secs(1200)))),
            Some(Baseline::Fail)
        );
        assert_eq!(classify_baseline(&outcome(None, CANCELLED_TAIL)), None);
    }

    #[tokio::test]
    async fn the_held_out_command_is_scrubbed_from_the_outcome_and_the_detail() {
        let dir = tempfile::tempdir().unwrap();
        // The command prints itself, so its output would leak it.
        let command = "cat held.sh; false";
        std::fs::write(dir.path().join("held.sh"), command).unwrap();
        let task = Task {
            criteria: vec!["the first".into(), "the second".into()],
            ..task_with_status(TaskStatus::Running)
        };
        let check = Check {
            criterion: 1,
            run: command.into(),
            baseline: Some(Baseline::Fail),
        };
        let (result, detail) = run_held_out(
            &task,
            &check,
            dir.path(),
            dir.path(),
            SandboxMode::Host,
            &CancelToken::new(),
        )
        .await;
        assert_eq!(result.command, "held-out check (criterion 1)");
        assert_eq!(result.code, Some(1));
        let detail = detail.unwrap();
        assert!(
            detail.starts_with("Held-out check for criterion 1 failed: the second. exited 1.\n"),
            "{detail}"
        );
        assert!(detail.contains("<held-out check>"), "{detail}");
        assert!(!detail.contains(command) && !result.tail.contains(command));
    }

    #[tokio::test]
    async fn a_baseline_undoes_what_the_checks_did_to_the_worktree() {
        let repo = tempfile::tempdir().unwrap();
        let sh = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .status()
                .unwrap()
                .success());
        };
        sh(&["init", "-q"]);
        sh(&["config", "user.email", "t@example.com"]);
        sh(&["config", "user.name", "t"]);
        std::fs::write(repo.path().join("tracked.txt"), "one\n").unwrap();
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "init"]);
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.criteria = vec!["a".into(), "b".into(), "c".into()];
        let check = |criterion, run: &str| Check {
            criterion,
            run: run.into(),
            baseline: None,
        };
        task.checks = vec![
            check(0, "echo x > made.txt; mkdir -p d/e; echo y > d/e/f.txt; echo more >> tracked.txt; false"),
            check(1, "true"),
            check(2, "no-such-command-anywhere"),
        ];
        let before = git::status_entries(repo.path()).unwrap();
        assert!(
            baseline_checks(
                &app,
                &task.id.clone(),
                &mut task,
                repo.path(),
                &CancelToken::new()
            )
            .await
        );
        assert_eq!(git::status_entries(repo.path()).unwrap(), before);
        assert!(!repo.path().join("made.txt").exists() && !repo.path().join("d").exists());
        assert_eq!(
            std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
            "one\n"
        );
        let baselines: Vec<_> = task.checks.iter().map(|c| c.baseline).collect();
        assert_eq!(
            baselines,
            vec![
                Some(Baseline::Fail),
                Some(Baseline::Pass),
                Some(Baseline::Env)
            ]
        );
        assert!(task.decisions.contains(
            &"Orchestrator: check for criterion 1 already passes on base, not grounded: true"
                .to_string()
        ));
        assert!(task.decisions.iter().any(|d| d.starts_with(
            "Orchestrator: check for criterion 2 could not run on base (exit 127), left out: no-such"
        )));
        assert!(!needs_baseline(&task));
    }
}
