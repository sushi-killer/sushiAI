use super::*;
use compiler_cache::{CacheRun, CompilerCache};

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
pub(super) fn hook_budget_secs(verify_timeout_secs: u64) -> u64 {
    verify_timeout_secs.clamp(1, HOOK_BUDGET_CAP_SECS)
}

pub(super) fn set_verify_timeout_secs(secs: u64) {
    VERIFY_TIMEOUT_SECS.store(secs.max(1), Ordering::SeqCst);
}

pub(super) fn verify_timeout() -> Duration {
    Duration::from_secs(VERIFY_TIMEOUT_SECS.load(Ordering::SeqCst))
}

/// Seatbelt's later rules reopen this run and worktree under denied roots.
pub fn to_sbpl(sandbox: &TaskSandbox) -> String {
    let quoted = |path: &Path| serde_json::to_string(&path.to_string_lossy()).unwrap();
    let mut profile = String::from("(version 1)\n(allow default)\n");
    for path in &sandbox.denied_read_roots {
        profile.push_str(&format!("(deny file-read* (subpath {}))\n", quoted(path)));
    }
    for path in &sandbox.read_exceptions {
        profile.push_str(&format!("(allow file-read* (subpath {}))\n", quoted(path)));
    }
    for path in &sandbox.read_metadata_paths {
        profile.push_str(&format!(
            "(allow file-read-metadata (literal {}))\n",
            quoted(path)
        ));
    }
    for path in &sandbox.read_metadata_subpaths {
        profile.push_str(&format!(
            "(allow file-read-metadata (subpath {}))\n",
            quoted(path)
        ));
    }
    profile.push_str("(deny file-write* (subpath \"/\"))\n");
    for path in &sandbox.write_roots {
        profile.push_str(&format!("(allow file-write* (subpath {}))\n", quoted(path)));
    }
    profile.push_str(&format!(
        "(deny network-outbound (remote unix-socket (path-literal {})))\n",
        quoted(&sandbox.daemon_socket),
    ));
    profile
}

/// `sandbox-exec`-wraps the command on macOS when `sandbox == Native`;
/// plain `/bin/sh -c` for `Host`, and on non-macOS (`sandbox-exec` doesn't
/// exist there -- a documented limitation, not a bug).
pub(super) fn build_verify_command(
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

/// One merged policy for diff checks, base checks, briefs and shared checks.
pub(super) fn scoped_checks_for(settings: &Settings, task: &Task) -> Vec<ScopedCheck> {
    let mut scoped = settings.scoped_checks.clone();
    if let Ok(entries) =
        crate::worktree_env::scoped_checks_at(Path::new(&task.repo), &task.base_sha)
    {
        scoped.extend(entries);
    }
    scoped
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
    task: &Task,
    worktree: &Path,
    base: &str,
    commands: &[String],
) -> (Vec<String>, Vec<String>) {
    let scoped = scoped_checks_for(&app.settings.read().unwrap(), task);
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
        Some(changed) => filter_scoped(&scoped, &task.repo, commands, &changed),
        None => (commands.to_vec(), Vec::new()),
    }
}

/// Path policy first, then package/test checks for subtasks gated at their parent.
pub(super) async fn scope_task_checks(
    app: &Arc<App>,
    task: &Task,
    worktree: &Path,
    base: &str,
    commands: &[String],
) -> (Vec<String>, Vec<String>) {
    let (path_commands, mut lines) = scope_to_diff(app, task, worktree, base, commands).await;
    let enabled = task.parent.is_some() && app.settings.read().unwrap().gates_at_parent(&task.repo);
    if !enabled {
        return (path_commands, lines);
    }
    let (wt, b, remaining) = (
        worktree.to_path_buf(),
        base.to_string(),
        path_commands.clone(),
    );
    let narrowed = tokio::task::spawn_blocking(move || {
        git::changed_files(&wt, &b)
            .map(|changed| subtask_checks::scope(&wt, &changed, &remaining, true))
    })
    .await;
    let Ok(Ok(mut narrowed)) = narrowed else {
        return (commands.to_vec(), Vec::new());
    };
    // Path-scoped shared checks also remain the parent's responsibility.
    narrowed.deferred.extend(
        commands
            .iter()
            .filter(|c| !path_commands.contains(c))
            .cloned(),
    );
    if !narrowed.deferred.is_empty() && !add_parent_checks(app, task, &narrowed.deferred) {
        return (commands.to_vec(), Vec::new());
    }
    lines.extend(narrowed.decisions);
    (narrowed.commands, lines)
}

fn add_parent_checks(app: &App, task: &Task, commands: &[String]) -> bool {
    let Some(parent_id) = task.parent.as_deref() else {
        return false;
    };
    let Ok(Some(mut parent)) = app.store.load_task(parent_id) else {
        return false;
    };
    let mut changed = false;
    for command in commands {
        if parent
            .verify
            .iter()
            .chain(&parent.final_verify)
            .any(|c| c.trim() == command.trim())
        {
            continue;
        }
        parent.final_verify.push(command.trim().to_string());
        parent.decisions.push(format!(
            "Orchestrator: added {} to finalVerify for subtask {}",
            command.trim(),
            short_sha(&task.id)
        ));
        changed = true;
    }
    if changed {
        parent.updated_at = now_ms();
        if app.store.save_task(&parent).is_err() {
            return false;
        }
        app.broadcast_task(&parent);
    }
    true
}

/// Splits a subtask's `finalVerify` into the commands it runs and those its
/// parent runs anyway (exact match after trim, against the parent's `verify`
/// and `finalVerify`). Nothing is deferred for a top-level task, a parent that
/// cannot be loaded, or a repo with `gateAt` "every". The stored list is
/// never changed.
pub(super) fn split_deferred(
    app: &App,
    settings: &Settings,
    task: &Task,
) -> (Vec<String>, Vec<String>) {
    let all = || (task.final_verify.clone(), Vec::new());
    let Some(parent_id) = task.parent.as_deref() else {
        return all();
    };
    if !settings.gates_at_parent(&task.repo) {
        return all();
    }
    let Ok(Some(parent)) = app.store.load_task(parent_id) else {
        return all();
    };
    let at_parent: Vec<&str> = parent
        .verify
        .iter()
        .chain(&parent.final_verify)
        .map(|c| c.trim())
        .collect();
    let (deferred, run) = task
        .final_verify
        .iter()
        .cloned()
        .partition(|c| at_parent.contains(&c.trim()));
    (run, deferred)
}

/// The decision line for a command deferred to the parent.
pub(super) fn deferred_line(command: &str) -> String {
    format!("Orchestrator: Deferred to parent: {command}")
}

/// Adds each check decision line once.
pub(super) fn record_check_decisions(task: &mut Task, lines: Vec<String>) {
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

#[derive(Clone, Copy)]
pub(super) struct CheckContext<'a> {
    pub(super) app: &'a App,
    task_id: Option<&'a str>,
    pub(super) label: Option<&'a str>,
    gate: &'a str,
}

impl<'a> CheckContext<'a> {
    pub(super) fn new(app: &'a App, task_id: Option<&'a str>) -> Self {
        Self {
            app,
            task_id,
            label: None,
            gate: "verify",
        }
    }

    pub(super) fn with_gate(self, gate: &'a str) -> Self {
        Self { gate, ..self }
    }

    pub(super) fn with_label(self, label: &'a str) -> Self {
        Self {
            label: Some(label),
            ..self
        }
    }

    pub(super) fn decision(&self, mut line: String, lines: &mut Vec<String>) {
        let mut stored = self
            .task_id
            .and_then(|id| self.app.store.load_task(id).ok().flatten());
        if let Some(held) = stored
            .as_ref()
            .and_then(|task| task.held_out.as_ref())
            .filter(|held| !held.run.is_empty())
        {
            line = line.replace(&held.run, &brief::held_out_label(held.criterion));
        }
        lines.push(line.clone());
        if let Some(id) = self.task_id {
            {
                let controls = self.app.controls.lock().unwrap();
                if controls.contains_key(id) {
                    self.app
                        .check_decisions
                        .lock()
                        .unwrap()
                        .entry(id.to_string())
                        .or_default()
                        .push(line.clone());
                }
            }
            if let Some(task) = stored.as_mut() {
                record_check_decisions(task, vec![line]);
                task.updated_at = now_ms();
                self.app.save_task_logged(task);
                self.app.broadcast_task(task);
            }
        }
    }
}

pub(super) fn record_check_outcomes(app: &App, task: &mut Task, results: &[VerifyOutcome]) {
    let mut lines = app
        .check_decisions
        .lock()
        .unwrap()
        .remove(&task.id)
        .unwrap_or_default();
    lines.extend(results.iter().flat_map(|r| r.decisions.iter().cloned()));
    record_check_decisions(task, lines);
}

pub(super) fn machine_load() -> (f64, u32) {
    let mut load = 0.0;
    // getloadavg writes at most one double into this buffer.
    unsafe {
        libc::getloadavg(&mut load, 1);
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    (load, cores)
}

pub(super) fn timed_out_under_load(outcome: &VerifyOutcome) -> bool {
    outcome.load_timeout
        || (outcome.code.is_none()
            && outcome.tail.starts_with("timed out after ")
            && outcome.cores > 0
            && outcome.load >= f64::from(outcome.cores))
}

pub(super) async fn run_verify_commands(
    ctx: CheckContext<'_>,
    cwd: &Path,
    _run_dir: &Path,
    commands: &[String],
    sandbox: Option<&TaskSandbox>,
    base_sha: &str,
    cancel: &CancelToken,
) -> Vec<VerifyOutcome> {
    let cache_run = CacheRun::for_commands(cwd, commands).await;
    let mut results = Vec::new();
    for cmd in commands {
        let timeout = Duration::from_secs(ctx.app.settings.read().unwrap().verify_timeout_secs);
        results
            .push(run_one_verify_command(ctx, cwd, cmd, timeout, sandbox, base_sha, cancel).await);
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
pub(super) async fn run_eval_check(
    ctx: CheckContext<'_>,
    repo: &Path,
    run_dir: &Path,
    sha: &str,
    cmd: &str,
    sandbox: Option<&TaskSandbox>,
    cancel: &CancelToken,
) -> VerifyOutcome {
    let path = run_dir.join("eval-check");
    let failed = |tail: String| VerifyOutcome {
        command: cmd.into(),
        tail,
        ..Default::default()
    };
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
    if let Err(e) = super::worktrees::setup_worktree(repo, &path, sandbox, cancel).await {
        return failed(e);
    }
    let timeout = Duration::from_secs(ctx.app.settings.read().unwrap().verify_timeout_secs);
    let cache_run = CacheRun::for_commands(&path, &[cmd.to_string()]).await;
    let mut out = run_one_verify_command(ctx, &path, cmd, timeout, sandbox, sha, cancel).await;
    if let Some(line) = cache_run.finish("eval checks").await {
        ctx.decision(line, &mut out.decisions);
    }
    let (r, p) = (repo.to_path_buf(), path);
    let _ = tokio::task::spawn_blocking(move || git::remove_worktree(&r, &p)).await;
    out
}

/// Cached by a hash of `git diff <base>` + the untracked-file list: if
/// nothing has changed since the last run, reuse completed failures. Passes
/// come from the bounded tree cache; interrupted result sets are not stored.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_verify_cached(
    app: &Arc<App>,
    task_id: &str,
    repo: &str,
    worktree: &Path,
    run_dir: &Path,
    base: &str,
    commands: &[String],
    cancel: &CancelToken,
) -> (Vec<VerifyOutcome>, Vec<String>) {
    let wt = worktree.to_path_buf();
    let b = base.to_string();
    let hash = tokio::task::spawn_blocking(move || diff_hash(&wt, &b))
        .await
        .unwrap_or_default();
    let cached = app
        .verify_cache
        .lock()
        .unwrap()
        .get(task_id)
        .filter(|(h, results)| *h == hash && results.iter().map(|r| &r.command).eq(commands.iter()))
        .map(|(_, results)| results.clone());
    let sandbox = app.task_sandbox(repo, worktree, run_dir);
    let cache_run = CacheRun::for_commands(worktree, commands).await;
    let runner = CheckRunner::new(app, worktree, sandbox.as_ref(), base, cancel).for_task(task_id);
    let mut results = Vec::new();
    let mut lines = Vec::new();
    for (i, command) in commands.iter().enumerate() {
        let previous = cached.as_ref().and_then(|r| r.get(i));
        let (result, decisions) = runner.run_one(command, command, previous).await;
        results.push(result);
        lines.extend(decisions);
    }
    lines.extend(cache_run.finish("verify").await);
    let mut cache = app.verify_cache.lock().unwrap();
    if !cancel.is_cancelled()
        && results
            .iter()
            .all(|r| r.code.is_some() && !is_interrupted(r))
    {
        cache.insert(task_id.to_string(), (hash, results.clone()));
    } else {
        cache.remove(task_id);
    }
    (results, lines)
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
            .env("PATH", augmented_path())
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

/// Whether `command` runs a whole repository's suite: `npm run ci`, `cargo
/// test --workspace`, `make test`, a bare `pytest`, `go test ./...`, or an
/// `npm|pnpm|yarn run <script>` whose `<cwd>/package.json` script body is one
/// of these (one level, no further script expansion). Such a check is
/// finalVerify's job, not a per-attempt verify command. The command is split
/// on `&&`, `||` and `;`; one such segment is enough.
pub(super) fn whole_repo_suite(cwd: &Path, command: &str) -> bool {
    let segments = |text: &str| -> Vec<String> {
        text.replace("&&", ";")
            .replace("||", ";")
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let words_of = |segment: &str| -> Vec<String> {
        segment
            .split_whitespace()
            .skip_while(|w| w.contains('=') && !w.starts_with('-'))
            .map(String::from)
            .collect()
    };
    let direct = |words: &[String]| -> bool {
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        let is_suite_word = |x: &str| matches!(x, "ci" | "test" | "check");
        match w.as_slice() {
            ["npm", "test"] => true,
            ["npm", "run", x] => is_suite_word(x),
            ["pnpm" | "yarn", x] => is_suite_word(x),
            ["pnpm" | "yarn", "run", x] => is_suite_word(x),
            ["make", x] => is_suite_word(x),
            ["pytest"] => true,
            ["go", "test", rest @ ..] => {
                rest.contains(&"./...")
                    && rest.iter().all(|a| *a == "./..." || a.starts_with('-'))
                    && !rest.contains(&"-run")
            }
            ["cargo", "test", rest @ ..] => {
                if rest.contains(&"--workspace") {
                    return true;
                }
                let before_args = rest.iter().take_while(|a| **a != "--");
                let mut scoped_or_filtered = false;
                for a in before_args {
                    let flag = a.split('=').next().unwrap_or(a);
                    if matches!(flag, "-p" | "--package" | "--test" | "--lib" | "--bin")
                        || !a.starts_with('-')
                    {
                        scoped_or_filtered = true;
                    }
                }
                !scoped_or_filtered
            }
            _ => false,
        }
    };
    segments(command).iter().any(|segment| {
        let words = words_of(segment);
        if direct(&words) {
            return true;
        }
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        let script = match w.as_slice() {
            ["npm" | "pnpm" | "yarn", "run", name] => *name,
            _ => return false,
        };
        let Ok(text) = std::fs::read_to_string(cwd.join("package.json")) else {
            return false;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            return false;
        };
        json.get("scripts")
            .and_then(|s| s.get(script))
            .and_then(|b| b.as_str())
            .is_some_and(|body| segments(body).iter().any(|s| direct(&words_of(s))))
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
pub(super) const CANCELLED_TAIL: &str = "cancelled";
pub(super) const SPAWN_FAILED_PREFIX: &str = "failed to run: ";
pub(super) const INTERNAL_ERROR_PREFIX: &str = "internal error: ";

/// How a check ran on the base checkout. `None` for a cancel, which aborts
/// the baseline instead of classifying anything.
pub(super) fn classify_baseline(outcome: &VerifyOutcome) -> Option<Baseline> {
    if outcome.tail == CANCELLED_TAIL {
        return None;
    }
    if outcome.code != Some(0) && timed_out_under_load(outcome) {
        return Some(Baseline::Env);
    }
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
    let run_dir = app.store.task_dir(task_id).join("runs/baseline");
    let sandbox = app.task_sandbox(&task.repo, worktree, &run_dir);
    let cache_run = CacheRun::for_commands(worktree, &commands).await;
    let runner =
        CheckRunner::new(app, worktree, sandbox.as_ref(), &task.base_sha, cancel).for_task(task_id);
    let mut results = Vec::new();
    let mut decisions = Vec::new();
    for ((slot, criterion, _), command) in todo.iter().zip(&commands) {
        let label = slot.map_or_else(|| brief::held_out_label(*criterion), |_| command.clone());
        let (result, lines) = runner.run_one(command, &label, None).await;
        results.push(result);
        decisions.extend(lines);
    }
    decisions.extend(cache_run.finish("baseline checks").await);
    record_check_decisions(task, decisions);
    let wt = worktree.to_path_buf();
    let _ = tokio::task::spawn_blocking(move || git::restore_status(&wt, &before)).await;
    if cancel.is_cancelled() {
        return false;
    }
    record_check_outcomes(app, task, &results);
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
    if let Some(h) = task
        .held_out
        .as_ref()
        .filter(|h| task.variant().held_out && h.baseline.is_none())
    {
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
        || (task.variant().held_out && task.held_out.as_ref().is_some_and(|h| h.baseline.is_none()))
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
        .filter(|h| task.variant().held_out && h.baseline == Some(Baseline::Fail))
}

/// Runs the held-out check and returns its outcome with the command scrubbed
/// out of the command field and the tail, plus the failure detail when it
/// did not exit 0.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_held_out(
    ctx: CheckContext<'_>,
    task: &Task,
    check: &Check,
    worktree: &Path,
    run_dir: &Path,
    sandbox: Option<&TaskSandbox>,
    cancel: &CancelToken,
) -> (VerifyOutcome, Option<String>) {
    let label = brief::held_out_label(check.criterion);
    let ctx = CheckContext {
        label: Some(&label),
        ..ctx
    };
    let result = run_verify_commands(
        ctx,
        worktree,
        run_dir,
        std::slice::from_ref(&check.run),
        sandbox,
        &task.base_sha,
        cancel,
    )
    .await
    .remove(0);
    held_out_result(task, check, result)
}

/// Runs a held-out gate using its real command as the key and its label in decisions.
#[cfg(test)]
pub(super) async fn run_held_out_cached(
    runner: &CheckRunner<'_>,
    task: &Task,
    check: &Check,
) -> (VerifyOutcome, Option<String>, Vec<String>) {
    let (result, lines) = runner
        .run_one(&check.run, &brief::held_out_label(check.criterion), None)
        .await;
    let (result, detail) = held_out_result(task, check, result);
    (result, detail, lines)
}

fn held_out_result(
    task: &Task,
    check: &Check,
    mut result: VerifyOutcome,
) -> (VerifyOutcome, Option<String>) {
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
/// harness). After every outcome the group is stopped (SIGTERM, SIGKILL 5s
/// later); on timeout *or* cancellation processes that left the group but
/// run from `cwd` are stopped too -- a bare `tokio::time::timeout` around `Command::output()`
/// only stops *waiting*, it never touches the still-running process (or any
/// children it spawned), which is the bug this replaces.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_one_verify_command(
    ctx: CheckContext<'_>,
    cwd: &Path,
    cmd: &str,
    timeout: Duration,
    sandbox: Option<&TaskSandbox>,
    base_sha: &str,
    cancel: &CancelToken,
) -> VerifyOutcome {
    if !cancel.is_cancelled() {
        if let Some(result) = passed_on_tree(ctx, cwd, cmd, base_sha).await {
            return result;
        }
    }
    let mut decisions = Vec::new();
    let mut load_detail = None;
    let mut interrupted: Option<VerifyOutcome> = None;
    for attempt in 0..2 {
        // The only check-slot acquire site: every retry queues afresh, and
        // waiting never spends the command's execution timeout.
        let permit = match ctx.app.check_slots.clone().try_acquire_owned() {
            Ok(p) => Some(p),
            Err(_) => {
                let (running, limit) = ctx.app.check_slots.usage();
                ctx.decision(
                    format!(
                        "Orchestrator: waiting for a check slot ({running} of {limit}): {}",
                        ctx.label.unwrap_or(cmd)
                    ),
                    &mut decisions,
                );
                tokio::select! {
                    p = ctx.app.check_slots.clone().acquire_owned() => p.ok(),
                    _ = cancel.cancelled() => None,
                }
            }
        };
        let Some(permit) = permit else {
            return VerifyOutcome {
                command: cmd.into(),
                tail: CANCELLED_TAIL.into(),
                decisions,
                ..Default::default()
            };
        };
        if cancel.is_cancelled() {
            return VerifyOutcome {
                command: cmd.into(),
                tail: CANCELLED_TAIL.into(),
                decisions,
                ..Default::default()
            };
        }
        let tree = check_tree(cwd).await;
        let mut result =
            execute_verify_command(ctx, cwd, cmd, timeout, sandbox, base_sha, cancel).await;
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

#[allow(clippy::too_many_arguments)]
async fn execute_verify_command(
    ctx: CheckContext<'_>,
    cwd: &Path,
    cmd: &str,
    timeout: Duration,
    sandbox: Option<&TaskSandbox>,
    base_sha: &str,
    cancel: &CancelToken,
) -> VerifyOutcome {
    let (load, cores) = (ctx.app.check_load.read().unwrap())();
    let command = build_verify_command(cwd, cmd, sandbox, base_sha);
    let mut result = run_verify_command(cwd, cmd, command, timeout, cancel.cancelled()).await;
    result.load = load;
    result.cores = cores;
    result
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
        truncated: report.truncated,
        crashed: report.crashed,
        stopped_early: report.stopped_early,
        output_key: report.rest_key,
        ms: start.elapsed().as_millis() as u64,
        ..Default::default()
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

    #[test]
    fn disabling_held_out_keeps_regular_baselines_and_gated_checks() {
        let mut task = task_with_status(TaskStatus::Running);
        let check = |run: &str| Check {
            criterion: 0,
            run: run.into(),
            baseline: None,
        };
        task.checks = vec![check("regular")];
        task.held_out = Some(check("hidden"));
        assert!(task.variant().held_out);
        assert_eq!(
            baseline_todo(&task),
            vec![(Some(0), 0, "regular".into()), (None, 0, "hidden".into())]
        );
        task.variant = Some(Variant {
            held_out: false,
            ..Variant::default()
        });
        assert!(needs_baseline(&task));
        assert_eq!(baseline_todo(&task), vec![(Some(0), 0, "regular".into())]);
        task.checks[0].baseline = Some(Baseline::Fail);
        assert!(!needs_baseline(&task));
        assert_eq!(gated_checks(&task), task.checks);
        task.held_out.as_mut().unwrap().baseline = Some(Baseline::Fail);
        assert!(gated_held_out(&task).is_none());
        task.variant.as_mut().unwrap().held_out = true;
        assert_eq!(gated_held_out(&task), task.held_out);
    }

    fn outcome(code: Option<i32>, tail: &str) -> VerifyOutcome {
        VerifyOutcome {
            command: "c".into(),
            code,
            tail: tail.into(),
            ms: 0,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn an_interrupted_verify_result_is_not_cached() {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let base = git_in(&repo, &["rev-parse", "HEAD"]);
        let counter = dir.path().join("counter");
        let commands = vec![format!(
            "echo run >> '{}'; n=$(wc -l < '{}'); if [ \"$n\" -le 2 ]; then exit 143; fi",
            counter.display(),
            counter.display()
        )];
        let cancel = CancelToken::new();
        let repo_name = repo.to_string_lossy().into_owned();
        let run = || {
            run_verify_cached(
                &app,
                "task",
                &repo_name,
                &repo,
                dir.path(),
                &base,
                &commands,
                &cancel,
            )
        };
        let (first, lines) = run().await;
        assert_eq!(first[0].code, Some(143));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );
        assert!(app.verify_cache.lock().unwrap().is_empty());
        assert!(lines
            .iter()
            .any(|l| l.contains("was interrupted") && l.contains("interrupted after")));
        let (second, _) = run().await;
        assert_eq!(second[0].code, Some(0));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            3
        );
        let (third, lines) = run().await;
        assert_eq!(third[0].code, Some(0));
        assert_eq!(third[0].ms, 0);
        assert!(lines.iter().any(|l| l.contains("already passed on tree")));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            3
        );
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
        for code in [124, 137, 143] {
            assert_eq!(
                classify_baseline(&outcome(Some(code), "")),
                Some(Baseline::Fail)
            );
        }
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
        let (app, _app_dir) = test_app();
        let (result, detail) = run_held_out(
            CheckContext::new(&app, None),
            &task,
            &check,
            dir.path(),
            dir.path(),
            None,
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
        let (app, _dir) = test_app_fitting_sandbox();
        let mut task = task_with_status(TaskStatus::Running);
        task.criteria = vec!["a".into(), "b".into(), "c".into()];
        let check = |criterion, run: &str| Check {
            criterion,
            run: run.into(),
            baseline: None,
        };
        task.checks = vec![
            check(
                0,
                "echo x > made.txt; mkdir -p d/e; echo y > d/e/f.txt; echo more >> tracked.txt; false",
            ),
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

#[cfg(test)]
mod scoped_task_tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".sushiai")).unwrap();
        std::fs::write(
            dir.path().join(".sushiai/setup.json"),
            r#"{"scopedChecks":[{"command":"repo-check","paths":["src/**"]}]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("a/src")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = ['a']\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("a/Cargo.toml"),
            "[package]\nname = 'package-a'\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("a/src/lib.rs"), "// base\n").unwrap();
        init_git_repo(dir.path());
        let base = git_in(dir.path(), &["rev-parse", "HEAD"]);
        (dir, base)
    }

    #[test]
    fn merged_base_policy_is_shared_and_uses_the_existing_filters() {
        let (dir, base) = repo();
        let task = Task {
            repo: dir.path().to_string_lossy().into_owned(),
            base_sha: base,
            ..task_with_status(TaskStatus::Running)
        };
        let mut settings = Settings::default();
        settings.scoped_checks.push(ScopedCheck {
            repo: None,
            command: "repo-check".into(),
            paths: vec!["electron/**".into()],
        });
        std::fs::write(
            dir.path().join(".sushiai/setup.json"),
            r#"{"scopedChecks":[{"command":"new-check","paths":["never/**"]}]}"#,
        )
        .unwrap();
        let scoped = scoped_checks_for(&settings, &task);
        assert_eq!(scoped.len(), 2);
        assert_eq!(scoped[1].repo.as_deref(), Some(task.repo.as_str()));
        assert_eq!(scoped[1].paths, ["src/**"]);
        assert!(is_shared_check(&Settings::default(), &task, "repo-check"));
        let commands = vec!["repo-check".into()];
        for file in ["src/a.ts", "electron/main.cjs"] {
            assert_eq!(
                filter_scoped(&scoped, &task.repo, &commands, &[file.into()]).0,
                commands
            );
            assert_eq!(
                filter_scoped_planned(&scoped, &task.repo, &commands, &[file.into()]),
                commands
            );
        }
        assert!(
            filter_scoped(&scoped, &task.repo, &commands, &["other/file".into()])
                .0
                .is_empty()
        );
        assert!(
            filter_scoped_planned(&scoped, &task.repo, &commands, &["other".into()]).is_empty()
        );
    }

    #[tokio::test]
    async fn scoped_checks_are_persisted_to_the_parent_once_and_never_lost() {
        let (app, _app_dir) = test_app();
        let (dir, base) = repo();
        std::fs::write(dir.path().join("a/src/lib.rs"), "// changed\n").unwrap();
        let parent = task_with_status(TaskStatus::Running);
        app.store.save_task(&parent).unwrap();
        let mut child = Task {
            repo: dir.path().to_string_lossy().into_owned(),
            base_sha: base.clone(),
            parent: Some(parent.id.clone()),
            ..task_with_status(TaskStatus::Running)
        };
        let commands = vec!["cargo test --workspace".into(), "repo-check".into()];
        let (run, lines) = scope_task_checks(&app, &child, dir.path(), &base, &commands).await;
        assert_eq!(run, ["cargo test -p package-a"]);
        assert!(lines.contains(&"Orchestrator: subtask ran cargo test -p package-a instead of cargo test --workspace; the parent runs cargo test --workspace on the combined tree".into()));
        let saved = app.store.load_task(&parent.id).unwrap().unwrap();
        assert_eq!(saved.final_verify, commands);
        assert_eq!(saved.decisions.len(), 2);
        assert!(saved.decisions.contains(&format!(
            "Orchestrator: added cargo test --workspace to finalVerify for subtask {}",
            short_sha(&child.id)
        )));
        scope_task_checks(&app, &child, dir.path(), &base, &commands).await;
        let saved_again = app.store.load_task(&parent.id).unwrap().unwrap();
        assert_eq!(saved_again.final_verify, saved.final_verify);
        assert_eq!(saved_again.decisions, saved.decisions);
        child.parent = Some("missing-parent".into());
        assert_eq!(
            scope_task_checks(&app, &child, dir.path(), &base, &commands).await,
            (commands.clone(), Vec::new())
        );
        child.parent = None;
        assert_eq!(
            scope_task_checks(&app, &child, dir.path(), &base, &commands)
                .await
                .0,
            ["cargo test --workspace"]
        );
    }

    #[test]
    fn a_native_sandbox_check_overrides_a_configured_rustc_wrapper() {
        let root = Path::new("/");
        let policy = TaskSandbox::build(SandboxInputs {
            repo_root: root,
            worktree: root,
            run_dir: &root.join("run"),
            data_dir: &root.join("data"),
            sushiai_home: &root.join("sushiai"),
            worktree_root: "",
            home: &root.join("home"),
            daemon_socket: &root.join("daemon.sock"),
            cargo_home: None,
            rustup_home: None,
            tmp_dir: None,
            npm_cache: None,
            known_project_folders: &[],
            known_task_worktrees: &[],
        });
        let wrapper = |sandbox: Option<&TaskSandbox>| {
            let environment = VerifyEnvironment {
                path: "/usr/bin:/bin".into(),
                compiler_cache: None,
            };
            let command = build_verify_command_in(root, "true", sandbox, "base", &environment);
            command
                .as_std()
                .get_envs()
                .find(|(key, _)| *key == "RUSTC_WRAPPER")
                .map(|(_, value)| value.map(|v| v.to_os_string()))
        };
        assert_eq!(wrapper(Some(&policy)), Some(Some("".into())));
        assert_eq!(wrapper(None), None);
    }

    #[tokio::test]
    async fn every_gate_disables_subtask_narrowing() {
        let (app, _app_dir) = test_app();
        let (dir, base) = repo();
        std::fs::write(dir.path().join("a/src/lib.rs"), "// changed\n").unwrap();
        let repo = dir.path().to_string_lossy().into_owned();
        app.settings
            .write()
            .unwrap()
            .gate_at
            .insert(repo.clone(), GateAt::Every);
        let child = Task {
            repo,
            base_sha: base.clone(),
            parent: Some("parent".into()),
            ..task_with_status(TaskStatus::Running)
        };
        let commands = vec!["cargo test --workspace".into()];
        assert_eq!(
            scope_task_checks(&app, &child, dir.path(), &base, &commands).await,
            (commands, Vec::new())
        );
    }
}
