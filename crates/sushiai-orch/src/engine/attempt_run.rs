use super::*;

fn wrap_task_harness(
    bin: String,
    argv: Vec<String>,
    req: &harness::RunRequest<'_>,
) -> (String, Vec<String>) {
    #[cfg(target_os = "macos")]
    if req.harness == Harness::Codex && harness::task_run_uses_outer_sandbox(req) {
        if let Some(policy) = req.task_sandbox {
            let mut wrapped = vec!["-p".into(), verify::to_sbpl(policy), bin];
            wrapped.extend(argv);
            return ("sandbox-exec".into(), wrapped);
        }
    }
    let _ = req;
    (bin, argv)
}

/// What a run is for, so its cost record names the stage. `repo` is set for
/// a run that is no task's (audit, evolution proposal, chat): its `task_id`
/// argument is then the audit or proposal id, kept as the record's `runId`.
pub(super) struct CostTag {
    pub stage: &'static str,
    pub route_id: String,
    pub repo: Option<String>,
    pub has_attempt: bool,
}

impl CostTag {
    pub fn task(stage: &'static str, route_id: &str) -> Self {
        CostTag {
            stage,
            route_id: route_id.to_string(),
            repo: None,
            has_attempt: true,
        }
    }

    pub fn task_without_attempt(stage: &'static str, route_id: &str) -> Self {
        Self {
            has_attempt: false,
            ..Self::task(stage, route_id)
        }
    }

    pub fn repo(stage: &'static str, route_id: &str, repo: &str) -> Self {
        CostTag {
            stage,
            route_id: route_id.to_string(),
            repo: Some(repo.to_string()),
            has_attempt: false,
        }
    }
}

/// The files a file-backed run (an implement or plan attempt) leaves next to
/// its `events.jsonl`: the child's stdin, its raw stdout and stderr, the exit
/// code its wrapper writes last, the argv it was started with, and the token
/// its Stop hook presents (mode 0600). They let a restarted daemon find the
/// run again and finish it (see `recovery.rs`).
pub(super) struct RunFiles {
    pub stdin: PathBuf,
    pub raw: PathBuf,
    pub stderr: PathBuf,
    pub exit: PathBuf,
    pub meta: PathBuf,
    pub token: PathBuf,
}

impl RunFiles {
    pub fn of(events_path: &Path) -> Self {
        let at = |ext: &str| events_path.with_extension(ext);
        RunFiles {
            stdin: at("stdin"),
            raw: at("raw"),
            stderr: at("stderr.log"),
            exit: at("exit"),
            meta: at("run.json"),
            token: at("token"),
        }
    }
}

/// `true` while some process of group `pgid` is alive, not only its leader
/// (`kill(-pgid, 0)` signals every member).
pub(super) fn group_alive(pgid: i32) -> bool {
    pgid > 0 && unsafe { libc::kill(-pgid, 0) == 0 }
}

/// `true` while group `pgid` is alive and is the run whose wrapper shell was
/// given `exit` as its `$0`. A pgid saved by an earlier daemon can have been
/// reused by a stranger's process since.
pub(super) fn run_group_alive(pgid: i32, exit: &Path) -> bool {
    group_alive(pgid) && store::group_started_with(pgid, exit)
}

/// Creates `path` empty, readable by the owner only.
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

/// Removes the unredacted output files (`*.raw`, `*.stderr.log`) of a run
/// directory. Only a settled attempt that nothing can resume loses them.
pub(super) fn remove_raw_output(run_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(run_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".raw") || name.ends_with(".stderr.log") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// How a run relates to its attempt.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RunTrack {
    /// A nested run (review, triage, audit, chat): pipes, no recovery.
    No,
    /// The attempt's own run, on files: its pgid and session are saved. A
    /// daemon shutdown kills it.
    Attempt,
    /// As `Attempt`, and a daemon shutdown leaves it running for the next
    /// daemon to adopt (the implement run).
    Resumable,
}

/// What ends a run early: silence, a loop, spend past the cap, or the
/// wall-clock budget. The same guard watches a run this daemon started and
/// one it adopted.
pub(super) struct Guard {
    stall: Option<Stall>,
    wall: Option<(tokio::time::Instant, WallLimit)>,
    detector: Option<LoopDetector>,
    /// Dollars one attempt may spend; `None` = no cap.
    cap: Option<f64>,
    last_output: tokio::time::Instant,
}

enum WallLimit {
    Attempt(f64),
    Helper(String, Duration),
}

pub(super) fn helper_timeout_message(helper: &str, limit: Duration) -> String {
    let seconds = if limit.is_zero() {
        "0.0".to_string()
    } else {
        limit.as_secs_f64().to_string()
    };
    format!("{helper} timed out after {seconds}s")
}

/// Why a guard stopped a run.
pub(super) enum Trip {
    Stalled(String),
    Looped(String),
    OverBudget(String),
    TimeBudget(String),
    TimedOut(String),
}

impl Trip {
    pub fn apply(self, outcome: &mut harness::RunOutcome) {
        match self {
            Trip::Stalled(error) => {
                outcome.stalled = true;
                outcome.error = Some(error);
            }
            Trip::Looped(note) => outcome.looped = Some(note),
            Trip::OverBudget(error) => {
                outcome.over_budget = true;
                outcome.error = Some(error);
            }
            Trip::TimeBudget(error) => {
                outcome.time_budget = true;
                outcome.error = Some(error);
            }
            Trip::TimedOut(error) => {
                outcome.timed_out = true;
                outcome.error = Some(error);
            }
        }
    }
}

impl Guard {
    /// No watch: an unguarded run.
    pub fn off() -> Self {
        Guard {
            stall: None,
            wall: None,
            detector: None,
            cap: None,
            last_output: tokio::time::Instant::now(),
        }
    }

    pub fn helper(name: &str, limit: Duration) -> Self {
        Self {
            wall: Some((
                tokio::time::Instant::now() + limit,
                WallLimit::Helper(name.to_string(), limit),
            )),
            ..Self::off()
        }
    }

    /// The watch a variant asks for; `hook_running` pauses the stall clock.
    pub fn for_variant(variant: &Variant, hook_running: Option<Arc<HookClock>>) -> Self {
        Guard {
            stall: (variant.stall_timeout_secs > 0).then(|| Stall {
                limit: Duration::from_secs(variant.stall_timeout_secs),
                paused: hook_running.unwrap_or_default(),
            }),
            wall: None,
            detector: variant.loop_detect.then(LoopDetector::new),
            cap: (variant.max_attempt_cost_usd > 0.0).then_some(variant.max_attempt_cost_usd),
            last_output: tokio::time::Instant::now(),
        }
    }

    /// Adds the wall-clock budget of an implement run: `mins` after the
    /// attempt started (`started_at_ms`), so a run a restarted daemon adopts
    /// does not get a fresh one. `0` (or less) = no budget.
    pub fn with_time_budget(mut self, mins: f64, started_at_ms: i64) -> Self {
        if mins > 0.0 {
            let left_ms = started_at_ms as f64 + mins * 60_000.0 - now_ms() as f64;
            let left = Duration::from_millis(left_ms.max(0.0) as u64);
            self.wall = Some((tokio::time::Instant::now() + left, WallLimit::Attempt(mins)));
        }
        self
    }

    pub(super) fn armed(&self) -> bool {
        self.stall.is_some() || self.wall.is_some()
    }

    /// The earlier of the silence limit and the wall deadline. An unset
    /// limit still needs a deadline that does not overflow `Instant`
    /// (select! builds a disabled branch's future too).
    pub(super) fn deadline(&self) -> tokio::time::Instant {
        let limit = self
            .stall
            .as_ref()
            .map_or(Duration::from_secs(365 * 24 * 3600), |s| s.limit);
        let silent = self.last_output + limit;
        self.wall
            .as_ref()
            .map_or(silent, |(wall, _)| (*wall).min(silent))
    }

    pub(super) fn heard(&mut self) {
        self.last_output = tokio::time::Instant::now();
    }

    /// The wall deadline passed (`TimeBudget`), or the silence limit ran
    /// out. Silence is the only stall signal; a long
    /// Bash call or the Stop hook's verify (up to 540s) is silent too, so the
    /// limit must sit above them. While a hook runs no stall fires, and the
    /// moment it ends counts as activity: the window starts then.
    pub(super) fn stall_expired(&mut self) -> Option<Trip> {
        if let Some((wall, limit)) = &self.wall {
            if tokio::time::Instant::now() >= *wall {
                return Some(match limit {
                    WallLimit::Attempt(mins) => Trip::TimeBudget(format!(
                        "The attempt ran past its {mins} minute time budget; the run was stopped."
                    )),
                    WallLimit::Helper(name, limit) => {
                        Trip::TimedOut(helper_timeout_message(name, *limit))
                    }
                });
            }
        }
        let stall = self.stall.as_ref()?;
        if stall.paused.running.load(Ordering::SeqCst) {
            self.heard();
            return None;
        }
        let ended = *stall.paused.ended.lock().unwrap();
        if let Some(ended) = ended.filter(|e| *e > self.last_output) {
            self.last_output = ended;
        }
        if tokio::time::Instant::now() < self.deadline() {
            return None;
        }
        Some(Trip::Stalled(format!(
            "No output for {}s; the run was stopped as stalled.",
            stall.limit.as_secs()
        )))
    }

    /// One more output line (`outcome` already has it): `Some` when the run
    /// must stop.
    pub(super) fn line(
        &mut self,
        app: &App,
        (harness, model): (Harness, Option<&str>),
        line: &str,
        outcome: &harness::RunOutcome,
    ) -> Option<Trip> {
        self.heard();
        if let Some(hit) = self.detector.as_mut().and_then(|d| d.feed(line)) {
            return Some(Trip::Looped(format!(
                "{} Do not repeat it; change approach.",
                hit.detail
            )));
        }
        let cap = self.cap?;
        let spent = streamed_spend(app, harness, model, outcome);
        (outcome.over_budget || spent.is_some_and(|c| c > cap)).then(|| {
            Trip::OverBudget(format!(
                "The attempt spent about ${:.4}, past its ${cap} cap; the run was stopped.",
                spent.unwrap_or(cap)
            ))
        })
    }
}

/// Reads the lines `file` gains, as they arrive, until the run is over: its
/// exit file exists, its process group is gone, or `wrapper_gone` is set (the
/// daemon reaped the wrapper shell, which can die before it writes the exit
/// file and then stays a zombie that still counts as alive). What was written
/// before that moment is still delivered.
fn tail_file(
    file: PathBuf,
    exit: PathBuf,
    pgid: i32,
    wrapper_gone: Arc<AtomicBool>,
) -> Box<dyn tokio::io::AsyncRead + Unpin + Send> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut tx, rx) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let mut offset = 0u64;
        loop {
            let over = exit.exists() || wrapper_gone.load(Ordering::SeqCst) || !group_alive(pgid);
            if let Ok(mut f) = tokio::fs::File::open(&file).await {
                let _ =
                    tokio::io::AsyncSeekExt::seek(&mut f, std::io::SeekFrom::Start(offset)).await;
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match f.read(&mut buf).await {
                        Ok(n) if n > 0 => {
                            offset += n as u64;
                            if tx.write_all(&buf[..n]).await.is_err() {
                                return; // the reader gave up (cancelled run)
                            }
                        }
                        _ => break,
                    }
                }
            }
            if over {
                let _ = tx.shutdown().await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    Box::new(rx)
}

pub(super) enum RunError {
    Cancelled,
    Io(String),
    /// The harness binary does not exist: nothing ran, so it costs no attempt.
    NotFound(String),
}

fn agent_cache_block_reason(
    harness: Harness,
    network_allowed: bool,
    review: bool,
    sandbox: SandboxMode,
    cache_dir: &Path,
    worktree: &Path,
) -> Option<&'static str> {
    if harness == Harness::Codex && !network_allowed {
        return Some("Codex codexNetwork false blocks localhost access");
    }
    if harness == Harness::Codex && review {
        return Some("Codex read-only sandbox cannot write the cache directory");
    }
    // Codex always uses its own workspace sandbox, including in Host mode.
    // Native Claude runs grant the worktree, not the shared cache directory.
    if (harness == Harness::Codex || sandbox == SandboxMode::Native)
        && !cache_dir.starts_with(worktree)
    {
        return Some("agent sandbox cannot write the shared cache directory");
    }
    None
}

fn apply_agent_cache(
    cmd: &mut tokio::process::Command,
    cache: Option<&compiler_cache::CompilerCache>,
    req: &harness::RunRequest<'_>,
    sandbox: SandboxMode,
) -> Option<String> {
    let cache = cache?;
    if let Some(reason) = agent_cache_block_reason(
        req.harness,
        req.network_allowed,
        req.review,
        sandbox,
        &cache.dir,
        req.worktree,
    ) {
        return Some(format!(
            "Orchestrator: compiler cache disabled for agent: {reason}"
        ));
    }
    cache.apply(cmd);
    None
}

/// Stamps `first_edit_at` now when the streamed `line` is the first edit of
/// an implement attempt; `true` once it has.
pub(super) async fn note_first_edit(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    line: &str,
) -> bool {
    let is_edit = serde_json::from_str(line)
        .ok()
        .is_some_and(|v| crate::ab::is_edit_event(&v));
    if is_edit {
        let at = now_ms();
        persist_attempt_field(app, task_id, attempt_n, move |a| {
            if a.stage == Stage::Implement && a.first_edit_at.is_none() {
                a.first_edit_at = Some(at);
            }
        })
        .await;
    }
    is_edit
}

/// Spawn `claude`/`codex` in its own process group, write `brief` to
/// stdin, stream stdout into `events.jsonl` and the log broadcast, and wait
/// for it to exit. `cancel` triggers SIGTERM to the group, then SIGKILL
/// after 5s (spec: "Each child in its own process group; stop = SIGTERM to
/// the group, SIGKILL after 5s").
///
/// `track` gates the mid-run `pgid`/`session_id` persistence below:
/// `Attempt`/`Resumable` for a session that *is* `task.attempts[_]` with
/// number `attempt_n` (the implement or plan attempt itself); `No` for a nested session
/// that belongs to the attempt but writes to its own helper folder (review,
/// orchestrator triage) -- otherwise its own pgid/session id would
/// overwrite the real attempt's, leaving it stuck `running` with a session
/// id `--resume` should never see and a pgid recovery would `killpg` on a
/// process that already exited (P1, triage review). A nested session's own
/// child is still reachable through `cancel` for as long as this call is
/// awaited, so skipping persistence here only affects recovery after a
/// daemon crash mid-review/-triage, not the normal stop path.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_harness(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    track: RunTrack,
    worktree: &Path,
    req: &harness::RunRequest<'_>,
    tag: CostTag,
    brief_text: &str,
    events_path: &Path,
    cancel: &CancelToken,
    mut guard: Guard,
) -> Result<harness::RunOutcome, RunError> {
    let RunSecrets {
        env: project_env,
        redaction: redaction_env,
    } = run_secrets(app, task_id).map_err(|error| {
        if let Ok(Some(mut task)) = app.store.load_task(task_id) {
            task.decisions.push(format!("Orchestrator: {error}"));
            let _ = app.store.save_task(&task);
        }
        RunError::Io(error)
    })?;
    let track_attempt = track != RunTrack::No;
    let started_at = now_ms();
    let mut argv = harness::build_argv(req);
    let mut bin = resolve_binary(req.harness);
    #[cfg(unix)]
    if req.harness == Harness::Claude {
        if let Some(settings_path) = req.settings_path {
            let token_path = settings_path.with_file_name("oauth-token");
            if token_path.is_file() {
                let claude = bin;
                let args = argv;
                bin = "/bin/sh".to_string();
                argv = vec![
                    "-c".into(),
                    "exec 3<\"$1\"; rm -f \"$1\"; export CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR=3; shift; exec \"$@\"".into(),
                    "sushiai".into(),
                    token_path.to_string_lossy().into_owned(),
                    claude,
                ];
                argv.extend(args);
            }
        }
    }
    // Asked before the run, not after, so a stand-in binary that logs its
    // argv per invocation ends with the run's own.
    let codex_version = match req.harness {
        Harness::Codex => match codex_version(&bin, cancel, &mut guard).await {
            Ok(version) => version,
            Err(trip) => {
                let mut outcome = harness::RunOutcome::default();
                trip.apply(&mut outcome);
                record_run(
                    app,
                    task_id,
                    attempt_n,
                    &tag,
                    (req.harness, req.model, false),
                    &outcome,
                    started_at,
                );
                return Ok(outcome);
            }
        },
        Harness::Claude => None,
    };
    if cancel.is_cancelled() {
        return Err(RunError::Cancelled);
    }
    let mut cmd;
    let files = RunFiles::of(events_path);
    // What the fingerprint hashes: the harness's own argv, not the wrapper's.
    let run_argv = argv.clone();
    (bin, argv) = wrap_task_harness(bin, argv, req);
    if track_attempt {
        // stdin, stdout and stderr are files and a wrapper writes the exit
        // code last, so the run does not depend on this process staying
        // alive (a restarted daemon re-adopts it by pgid).
        let wrapped = std::mem::take(&mut argv);
        argv = vec![
            "-c".into(),
            "\"$@\"; c=$?; echo $c > \"$0.tmp\" && mv \"$0.tmp\" \"$0\"; exit $c".into(),
            files.exit.to_string_lossy().into_owned(),
            bin,
        ];
        argv.extend(wrapped);
        bin = "/bin/sh".to_string();
        {
            use std::io::Write;
            let _ =
                create_private(&files.stdin).and_then(|mut f| f.write_all(brief_text.as_bytes()));
        }
        let _ = std::fs::remove_file(&files.exit);
        let _ = std::fs::write(
            &files.meta,
            json!({"argv": &run_argv, "codexVersion": codex_version}).to_string(),
        );
    }
    cmd = tokio::process::Command::new(&bin);
    let path = augmented_path();
    cmd.args(&argv)
        .current_dir(worktree)
        .env("PATH", &path)
        // Lets a repo's own hooks tell an orchd-run agent from a person's
        // session (sushiAI's lesson reminder stays quiet for it).
        .env("ORCHD_TASK", task_id)
        .env("SUSHIAI_HOME", &app.home)
        .envs(&project_env);
    let task_policy = req
        .task_sandbox
        .filter(|_| cfg!(target_os = "macos") && req.task_run && !req.review);
    if let Some(policy) = task_policy {
        // The task sandbox policy owns the variables of a sandboxed run.
        policy.apply_check_env(cmd.as_std_mut());
    } else if !project_env.contains_key("RUSTC_WRAPPER") {
        let cache = compiler_cache::CompilerCache::for_worktree(worktree, &path);
        let sandbox = if req.task_run && !req.review {
            SandboxMode::Host
        } else {
            app.settings.read().unwrap().sandbox
        };
        if let Some(line) = apply_agent_cache(&mut cmd, cache.as_ref(), req, sandbox) {
            // Best-of candidates can discover the same skip concurrently.
            static CACHE_DECISIONS: StdMutex<()> = StdMutex::new(());
            let _write = CACHE_DECISIONS.lock().unwrap();
            let recorded = app
                .store
                .load_task(task_id)
                .ok()
                .flatten()
                .is_some_and(|task| task.decisions.contains(&line));
            if !recorded {
                app.append_decision(task_id, line);
            }
        }
    }
    if let Some(token) = app.hook_token_of(task_id, attempt_n) {
        cmd.env("SUSHIAI_ORCH_TOKEN", token);
    }
    if track_attempt {
        match (
            std::fs::File::open(&files.stdin).map(std::process::Stdio::from),
            create_private(&files.raw).map(std::process::Stdio::from),
            create_private(&files.stderr).map(std::process::Stdio::from),
        ) {
            (Ok(i), Ok(o), Ok(e)) => cmd.stdin(i).stdout(o).stderr(e),
            _ => return Err(RunError::Io("could not open the run files".into())),
        };
    } else {
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    }
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let mut child = cmd.spawn().map_err(|e| {
        let text = format!("{bin}: {e}");
        if e.kind() == std::io::ErrorKind::NotFound {
            RunError::NotFound(text)
        } else {
            RunError::Io(text)
        }
    })?;
    let pgid = child.id().map(|p| p as i32);
    if track_attempt {
        if let Some(pgid) = pgid {
            persist_attempt_field(app, task_id, attempt_n, move |a| a.pgid = Some(pgid)).await;
        }
    }

    type Source = Box<dyn tokio::io::AsyncRead + Unpin + Send>;
    let wrapper_gone = Arc::new(AtomicBool::new(false));
    let stdin = child.stdin.take();
    let (stdout, stderr): (Source, Source) = if track_attempt {
        let pgid = pgid.unwrap_or(0);
        (
            tail_file(
                files.raw.clone(),
                files.exit.clone(),
                pgid,
                wrapper_gone.clone(),
            ),
            tail_file(
                files.stderr.clone(),
                files.exit.clone(),
                pgid,
                wrapper_gone.clone(),
            ),
        )
    } else {
        (
            Box::new(child.stdout.take().expect("piped stdout")),
            Box::new(child.stderr.take().expect("piped stderr")),
        )
    };
    let mut out_lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stdout));
    let mut err_lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stderr));
    let feed_stdin = async {
        if let Some(mut stdin) = stdin {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(brief_text.as_bytes()).await;
        }
    };
    tokio::pin!(feed_stdin);
    let mut stdin_done = track_attempt;

    let mut outcome = harness::RunOutcome::default();
    let harness_kind = req.harness;
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stderr_tail = String::new();
    let mut session_persisted = false;
    let mut edit_seen = false;
    let model = req.model;

    loop {
        tokio::select! {
            _ = &mut feed_stdin, if !stdin_done => {
                stdin_done = true;
            }
            _ = cancel.cancelled() => {
                if track == RunTrack::Resumable && app.runs_survive_shutdown() {
                    // A daemon shutdown leaves the run going: the next daemon
                    // finds it by its process group and files.
                    return Err(RunError::Cancelled);
                }
                crate::procs::kill_group(pgid, &mut child).await;
                // A stopped run still ran: keep what it reported so far.
                let fp = fingerprint_of(&outcome, req, &run_argv, codex_version.clone());
                let mut partial = outcome.clone();
                finalize_cost(app, req.harness, req.model, &mut partial);
                partial.fingerprint = Some(fp.clone());
                record_run(
                    app,
                    task_id,
                    attempt_n,
                    &tag,
                    (req.harness, req.model, false),
                    &partial,
                    started_at,
                );
                if track_attempt {
                    persist_attempt_field(app, task_id, attempt_n, move |a| a.fingerprint = Some(fp)).await;
                } else if let Ok(json) = serde_json::to_string(&fp) {
                    // Nested runs (review, advisor, audit) have no attempt to
                    // write to and RunError::Cancelled carries no payload:
                    // hand the fingerprint to the caller through a sidecar.
                    let _ = std::fs::write(cancelled_fingerprint_path(events_path), json);
                }
                return Err(RunError::Cancelled);
            }
            _ = tokio::time::sleep_until(guard.deadline()), if guard.armed() => {
                let Some(trip) = guard.stall_expired() else { continue };
                crate::procs::kill_group(pgid, &mut child).await;
                trip.apply(&mut outcome);
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(200)),
                if track_attempt && !wrapper_gone.load(Ordering::SeqCst) =>
            {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    wrapper_gone.store(true, Ordering::SeqCst);
                }
            }
            line = out_lines.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(l)) => {
                        let l = redact(&l, &redaction_env);
                        append_line(events_path, &l);
                        if let Some(note) = harness::feed_stream_line(harness_kind, &l, &mut outcome) {
                            app.broadcast_log(task_id, attempt_n, note);
                        }
                        if let Some(trip) = guard.line(app, (harness_kind, model), &l, &outcome) {
                            crate::procs::kill_group(pgid, &mut child).await;
                            trip.apply(&mut outcome);
                            break;
                        }
                        if track_attempt && !session_persisted {
                            if let Some(sid) = outcome.session_id.clone() {
                                session_persisted = true;
                                persist_attempt_field(app, task_id, attempt_n, move |a| a.session_id = Some(sid)).await;
                            }
                        }
                        if track_attempt && !edit_seen {
                            edit_seen = note_first_edit(app, task_id, attempt_n, &l).await;
                        }
                    }
                    _ => { stdout_done = true; }
                }
            }
            line = err_lines.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(l)) => {
                        guard.heard();
                        let l = redact(&l, &redaction_env);
                        append_line(events_path, &format!("[stderr] {l}"));
                        stderr_tail.push_str(&l);
                        stderr_tail.push('\n');
                        if stderr_tail.len() > 4000 {
                            let cut = stderr_tail.len() - 4000;
                            let cut = (cut..stderr_tail.len()).find(|i| stderr_tail.is_char_boundary(*i)).unwrap_or(0);
                            stderr_tail.drain(..cut);
                        }
                    }
                    _ => { stderr_done = true; }
                }
            }
            status = child.wait(), if stdout_done && stderr_done => {
                if let Ok(status) = status {
                    // The wrapper shell reports a harness it could not start
                    // (missing binary or interpreter) as 126 or 127: nothing ran.
                    if track_attempt && matches!(status.code(), Some(126 | 127)) {
                        let tail = stderr_tail.trim();
                        return Err(RunError::NotFound(if tail.is_empty() {
                            format!("{:?}: command not found", req.harness)
                        } else {
                            tail.to_string()
                        }));
                    }
                    if !status.success() && outcome.error.is_none() {
                        let tail = stderr_tail.trim();
                        outcome.error = Some(if tail.is_empty() {
                            crate::harness::exit_error(req.harness, status)
                        } else {
                            tail.to_string()
                        });
                    }
                }
                break;
            }
        }
    }
    finalize_cost(app, req.harness, req.model, &mut outcome);
    outcome.fingerprint = Some(fingerprint_of(&outcome, req, &run_argv, codex_version));
    record_run(
        app,
        task_id,
        attempt_n,
        &tag,
        (req.harness, req.model, req.resume.is_some()),
        &outcome,
        started_at,
    );
    Ok(outcome)
}

/// The project's agent variables, and every value known for the project
/// (agent and MCP variables) that a run's output must not keep.
#[derive(Default)]
pub(super) struct RunSecrets {
    pub env: HashMap<String, String>,
    pub redaction: HashMap<String, String>,
}

pub(super) fn run_secrets(app: &App, task_id: &str) -> Result<RunSecrets, String> {
    let Some(task) = app.store.load_task(task_id).ok().flatten() else {
        return Ok(Default::default());
    };
    let secrets = app.secrets.read().unwrap();
    let id = secrets::project_for_repo(&secrets, &task.repo, task.project_id.as_deref())?;
    Ok(id.map_or_else(Default::default, |id| {
        let agent = secrets.projects.get(&id).cloned().unwrap_or_default();
        let mut known = agent.clone();
        known.extend(secrets.project_mcp.get(&id).cloned().unwrap_or_default());
        RunSecrets {
            env: agent,
            redaction: known,
        }
    }))
}

pub(super) fn redact(line: &str, env: &std::collections::HashMap<String, String>) -> String {
    let mut values: Vec<_> = env.values().filter(|value| !value.is_empty()).collect();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.into_iter().fold(line.to_string(), |text, value| {
        text.replace(value, "[REDACTED]")
    })
}

/// Prices a run the harness reported no cost for: Codex from its token
/// usage, Claude from its streamed messages (marked estimated).
pub(super) fn finalize_cost(
    app: &App,
    harness: Harness,
    model: Option<&str>,
    outcome: &mut harness::RunOutcome,
) {
    if outcome.cost_usd.is_none() && harness == Harness::Codex {
        let price = model.and_then(|m| app.settings.read().unwrap().prices.get(m).copied());
        outcome.cost_usd = price.map(|p| {
            p.codex_cost(
                outcome.usage_input,
                outcome.usage_cached,
                outcome.usage_output,
            )
        });
    }
    if harness == Harness::Claude {
        outcome.estimate_cost(&app.settings.read().unwrap().prices);
    }
}

/// Appends the run's cost record. A resumed Claude session's own total
/// covers its earlier runs too, so its record is priced from this run's
/// streamed messages when it has any.
pub(super) fn record_run(
    app: &App,
    task_id: &str,
    attempt_n: u32,
    tag: &CostTag,
    run: (Harness, Option<&str>, bool),
    outcome: &harness::RunOutcome,
    started_at: i64,
) {
    let (harness, req_model, resumed) = run;
    let (mut cost, mut estimated) = (outcome.cost_usd.unwrap_or(0.0), outcome.cost_estimated);
    if harness == Harness::Claude && resumed {
        if let Some(own) = outcome.streamed_cost(&app.settings.read().unwrap().prices) {
            cost = own;
            estimated = true;
        }
    }
    let is_task = tag.repo.is_none();
    let repo = tag.repo.clone().unwrap_or_else(|| {
        app.store
            .load_task(task_id)
            .ok()
            .flatten()
            .map(|t| t.repo)
            .unwrap_or_default()
    });
    let model = outcome
        .fingerprint
        .as_ref()
        .and_then(|f| f.models.first().cloned())
        .or_else(|| req_model.map(str::to_string))
        .unwrap_or_else(|| "unknown".into());
    let ended = now_ms();
    crate::costs::append(
        &app.data_dir,
        &crate::costs::CostRecord {
            ts: ended,
            repo,
            task_id: is_task.then(|| task_id.to_string()),
            run_id: (!is_task && !task_id.is_empty()).then(|| task_id.to_string()),
            stage: tag.stage.to_string(),
            attempt: (is_task && tag.has_attempt).then_some(attempt_n),
            route_id: tag.route_id.clone(),
            harness,
            model,
            cost_usd: cost,
            estimated,
            input_tokens: outcome.usage_input,
            cached_tokens: outcome.usage_cached,
            output_tokens: outcome.usage_output,
            ms: (ended - started_at).max(0) as u64,
            backfilled: false,
        },
    );
}

/// What this run has cost so far, priced from its streamed message usage.
/// The CLI's own `total_cost_usd` is not used: on a resumed session it also
/// covers earlier attempts, which the cap must not count.
fn streamed_spend(
    app: &App,
    harness: Harness,
    model: Option<&str>,
    outcome: &harness::RunOutcome,
) -> Option<f64> {
    let settings = app.settings.read().unwrap();
    match harness {
        Harness::Claude => outcome.streamed_cost(&settings.prices),
        Harness::Codex => model.and_then(|m| settings.prices.get(m)).map(|p| {
            p.codex_cost(
                outcome.usage_input,
                outcome.usage_cached,
                outcome.usage_output,
            )
        }),
    }
}

fn fingerprint_of(
    outcome: &harness::RunOutcome,
    req: &harness::RunRequest<'_>,
    argv: &[String],
    codex_version: Option<String>,
) -> Fingerprint {
    let settings_json = req
        .settings_path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    outcome.build_fingerprint(
        req.harness,
        codex_version,
        harness::prompt_hash(argv, settings_json.as_ref()),
    )
}

/// `codex --version`, asked once per binary per daemon start.
async fn codex_version(
    bin: &str,
    cancel: &CancelToken,
    guard: &mut Guard,
) -> Result<Option<String>, Trip> {
    use tokio::io::AsyncReadExt;

    static CACHE: OnceLock<StdMutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(known) = cache.lock().unwrap().get(bin) {
        return Ok(known.clone());
    }
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("--version")
        .env("PATH", augmented_path())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(_) => return Ok(None),
    };
    let pgid = child.id().map(|pid| pid as i32);
    let mut stdout = child.stdout.take().expect("piped stdout");
    let result = {
        let output = async {
            let mut bytes = Vec::new();
            let (status, read) = tokio::join!(child.wait(), stdout.read_to_end(&mut bytes));
            (status, read, bytes)
        };
        tokio::pin!(output);
        loop {
            tokio::select! {
                output = &mut output => break Ok(Some(output)),
                _ = cancel.cancelled() => break Ok(None),
                _ = tokio::time::sleep_until(guard.deadline()), if guard.armed() => {
                    if let Some(trip) = guard.stall_expired() {
                        break Err(trip);
                    }
                }
            }
        }
    };
    let (status, read, bytes) = match result {
        Ok(Some(output)) => output,
        stopped => {
            crate::procs::kill_group(pgid, &mut child).await;
            return stopped.map(|_| None);
        }
    };
    let version = status
        .ok()
        .filter(|status| status.success() && read.is_ok())
        .map(|_| String::from_utf8_lossy(&bytes).trim().to_string())
        .filter(|v| !v.is_empty());
    cache
        .lock()
        .unwrap()
        .insert(bin.to_string(), version.clone());
    Ok(version)
}

fn cancelled_fingerprint_path(events_path: &Path) -> PathBuf {
    events_path.with_extension("fingerprint.json")
}

/// The fingerprint a cancelled nested run left next to its events file.
pub(super) fn read_cancelled_fingerprint(events_path: &Path) -> Option<Fingerprint> {
    let path = cancelled_fingerprint_path(events_path);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn codex_without_network_gets_no_wrapper_and_a_reason() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let tool = tmp.path().join("sccache");
        std::fs::write(&tool, "#!/bin/sh\nexit 99\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cache = compiler_cache::CompilerCache::discover(
            &tmp.path().display().to_string(),
            true,
            false,
            tmp.path().join("cache"),
            4227,
        )
        .unwrap();
        let req = harness::RunRequest {
            harness: Harness::Codex,
            worktree: tmp.path(),
            model: None,
            effort: None,
            max_budget_usd: None,
            review: false,
            task_run: false,
            task_sandbox: None,
            mcp_config: None,
            settings_path: None,
            network_allowed: false,
            codex_mcp: &[],
            images: &[],
            repo_settings: false,
            resume: None,
        };
        let mut command = tokio::process::Command::new("codex");
        let line = apply_agent_cache(&mut command, Some(&cache), &req, SandboxMode::Host).unwrap();
        assert!(line.contains("codexNetwork false") && line.contains("localhost"));
        assert!(!command
            .as_std()
            .get_envs()
            .any(|(k, _)| k == "RUSTC_WRAPPER" || k.to_string_lossy().starts_with("SCCACHE_")));
        assert_eq!(
            apply_agent_cache(&mut command, None, &req, SandboxMode::Host),
            None
        );
    }

    #[test]
    fn cache_write_access_is_required_even_with_agent_network() {
        let cache = Path::new("/shared/cache");
        let wt = Path::new("/worktree");
        assert!(agent_cache_block_reason(
            Harness::Codex,
            true,
            false,
            SandboxMode::Host,
            cache,
            wt
        )
        .is_some());
        assert!(agent_cache_block_reason(
            Harness::Claude,
            true,
            false,
            SandboxMode::Native,
            cache,
            wt
        )
        .is_some());
        assert_eq!(
            agent_cache_block_reason(Harness::Claude, true, false, SandboxMode::Host, cache, wt),
            None
        );
    }

    #[test]
    fn codex_task_wrapper_preserves_host_and_non_task_runs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let policy = TaskSandbox::build(crate::sandbox::Inputs {
            repo_root: root,
            worktree: root,
            run_dir: root,
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
        let mut req = harness::RunRequest {
            harness: Harness::Codex,
            worktree: root,
            model: None,
            effort: None,
            max_budget_usd: None,
            review: false,
            task_run: true,
            task_sandbox: Some(&policy),
            mcp_config: None,
            settings_path: None,
            network_allowed: false,
            codex_mcp: &[],
            images: &[],
            repo_settings: true,
            resume: None,
        };
        let argv = harness::codex_argv(&req);
        let (bin, wrapped) = wrap_task_harness("codex".into(), argv.clone(), &req);
        #[cfg(target_os = "macos")]
        {
            assert_eq!(bin, "sandbox-exec");
            assert_eq!(
                &wrapped[..3],
                &["-p", verify::to_sbpl(&policy).as_str(), "codex"]
            );
            assert_eq!(&wrapped[3..], argv);
        }
        #[cfg(not(target_os = "macos"))]
        assert_eq!((bin, wrapped), ("codex".into(), argv.clone()));
        req.task_sandbox = None;
        assert_eq!(
            wrap_task_harness("codex".into(), argv.clone(), &req),
            ("codex".into(), argv.clone())
        );
        req.task_sandbox = Some(&policy);
        req.review = true;
        assert_eq!(
            wrap_task_harness("codex".into(), argv.clone(), &req),
            ("codex".into(), argv.clone())
        );
        req.review = false;
        req.task_run = false;
        assert_eq!(
            wrap_task_harness("codex".into(), argv.clone(), &req),
            ("codex".into(), argv)
        );
    }

    #[test]
    fn redacts_known_values_longest_first() {
        let env = HashMap::from([
            ("TOKEN".to_string(), "invented-secret-value".to_string()),
            ("SHORT".to_string(), "xyz".to_string()),
        ]);
        assert_eq!(
            redact("invented-secret-value xyz", &env),
            "[REDACTED] [REDACTED]"
        );
    }
}

#[cfg(test)]
mod stall_tests {
    use super::*;

    fn guarded(limit: u64) -> (Guard, Arc<HookClock>) {
        let clock = Arc::new(HookClock::default());
        let variant = Variant {
            stall_timeout_secs: limit,
            ..Default::default()
        };
        (Guard::for_variant(&variant, Some(clock.clone())), clock)
    }

    fn ago(secs: u64) -> tokio::time::Instant {
        tokio::time::Instant::now() - Duration::from_secs(secs)
    }

    #[tokio::test]
    async fn codex_version_timed_out_uses_the_helper_deadline_and_is_not_cached() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("codex");
        std::fs::write(&bin, "#!/bin/sh\necho $$ > \"$0.pid\"\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        // App startup normally resolves the login PATH before a task runs.
        let _ = augmented_path();
        let cancel = CancelToken::new();
        let limit = Duration::from_millis(250);
        let mut guard = Guard::helper("planner", limit);
        // Far below the fake's 30 s sleep, far above a loaded machine's
        // spawn and kill latency: it proves the helper deadline, not speed.
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            codex_version(bin.to_str().unwrap(), &cancel, &mut guard),
        )
        .await
        .expect("the version probe must honor the helper deadline");
        let trip = match result {
            Err(trip) => trip,
            Ok(version) => panic!("the version probe should time out, got {version:?}"),
        };
        let mut outcome = harness::RunOutcome::default();
        trip.apply(&mut outcome);
        assert!(outcome.timed_out);
        assert_eq!(
            outcome.error,
            Some(helper_timeout_message("planner", limit))
        );
        // Under load the deadline can stop the fake before it writes its pid.
        if let Ok(pid) = std::fs::read_to_string(bin.with_extension("pid")) {
            let pgid: i32 = pid.trim().parse().unwrap();
            assert!(!group_alive(pgid), "the version probe was not stopped");
        }

        std::fs::write(&bin, "#!/bin/sh\necho 'codex fixture'\n").unwrap();
        let mut guard = Guard::helper("planner", Duration::from_secs(10));
        let version = codex_version(bin.to_str().unwrap(), &cancel, &mut guard)
            .await
            .ok()
            .expect("the later version probe must succeed");
        assert_eq!(version.as_deref(), Some("codex fixture"));
    }

    #[tokio::test]
    async fn cancelling_the_codex_version_probe_stops_its_process() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("codex");
        let pidfile = bin.with_extension("pid");
        std::fs::write(&bin, "#!/bin/sh\necho $$ > \"$0.pid\"\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _ = augmented_path();
        let cancel = CancelToken::new();
        let mut guard = Guard::off();
        let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                codex_version(bin.to_str().unwrap(), &cancel, &mut guard),
                async {
                    // The shell creates the file before it writes the pid.
                    while std::fs::read_to_string(&pidfile).map_or(true, |p| p.trim().is_empty()) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    cancel.cancel();
                }
            )
        })
        .await
        .expect("the probe must stop when the task is cancelled");
        assert_eq!(result.ok().expect("cancellation is not a timeout"), None);
        let pgid: i32 = std::fs::read_to_string(pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(!group_alive(pgid), "the cancelled probe was not stopped");
    }

    #[tokio::test]
    async fn helper_timed_out_while_stdin_is_full() {
        let (app, _dir, task, route) = slow_helper_task();
        let worktree = PathBuf::from(&task.worktree);
        std::fs::write(worktree.join(".overseer-no-stdin"), "").unwrap();
        let run_dir = app
            .store
            .claim_helper_run_dir(&task.id, "test-helper", None)
            .unwrap();
        let mcp = run_dir.join("mcp.json");
        let settings = run_dir.join("settings.json");
        let req = review_request(&route, &worktree, &mcp, &settings, &[]);
        let prompt = "x".repeat(1024 * 1024);
        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            run_harness(
                &app,
                &task.id,
                0,
                RunTrack::No,
                &worktree,
                &req,
                CostTag::task_without_attempt("brief_check", &route.id),
                &prompt,
                &run_dir.join("events.jsonl"),
                &CancelToken::new(),
                Guard::helper("test-helper", app.helper_timeout()),
            ),
        )
        .await
        .expect("the helper deadline must cover blocked stdin")
        .ok()
        .expect("the helper must start");
        assert!(outcome.timed_out);
        assert_eq!(
            outcome.error.as_deref(),
            Some("test-helper timed out after 0.25s")
        );
    }

    #[tokio::test]
    async fn helper_timed_out_uses_the_wall_deadline_even_after_output() {
        let limit = Duration::from_millis(50);
        let mut guard = Guard::helper("judge", limit);
        assert!(guard.armed());
        let deadline = guard.deadline();
        guard.heard();
        assert_eq!(guard.deadline(), deadline);
        assert!(guard.stall_expired().is_none());
        tokio::time::sleep_until(deadline).await;
        let trip = guard.stall_expired().expect("wall deadline expired");
        assert!(matches!(trip, Trip::TimedOut(_)));
        let mut outcome = harness::RunOutcome::default();
        trip.apply(&mut outcome);
        assert!(outcome.timed_out);
        assert!(!outcome.stalled && !outcome.time_budget && !outcome.over_budget);
        assert_eq!(
            outcome.error.as_deref(),
            Some("judge timed out after 0.05s")
        );

        let mut both = Guard::helper("judge", Duration::from_secs(10));
        both.stall = guarded(1).0.stall;
        assert!(both.deadline() < tokio::time::Instant::now() + Duration::from_secs(2));
    }

    #[test]
    fn the_wall_deadline_trips_and_no_budget_is_not_armed() {
        let off = Guard::off().with_time_budget(0.0, now_ms());
        assert!(!off.armed());
        let mut fresh = Guard::off().with_time_budget(10.0, now_ms());
        assert!(fresh.armed());
        assert!(fresh.stall_expired().is_none());
        // Started 2 minutes ago with a 1 minute budget: already past.
        let mut late = Guard::off().with_time_budget(1.0, now_ms() - 120_000);
        assert!(matches!(late.stall_expired(), Some(Trip::TimeBudget(_))));
        let (stall, _) = guarded(10);
        let both = stall.with_time_budget(1.0, now_ms());
        assert!(both.deadline() <= tokio::time::Instant::now() + Duration::from_secs(10));
    }

    #[test]
    fn a_hook_that_ends_on_the_limit_does_not_trip_the_stall() {
        let (mut guard, clock) = guarded(10);
        // Quiet since 20s ago; the hook ran the whole time and ends now,
        // exactly when the second window runs out.
        guard.last_output = ago(20);
        let hook = clock.start();
        assert!(
            guard.stall_expired().is_none(),
            "paused while the hook runs"
        );
        guard.last_output = ago(20);
        drop(hook);
        assert!(guard.stall_expired().is_none());
        assert!(guard.deadline() > tokio::time::Instant::now());
    }

    #[test]
    fn silence_past_the_limit_after_the_hook_ends_trips() {
        let (mut guard, clock) = guarded(10);
        guard.last_output = ago(30);
        drop(clock.start());
        *clock.ended.lock().unwrap() = Some(ago(12));
        assert!(matches!(guard.stall_expired(), Some(Trip::Stalled(_))));
        *clock.ended.lock().unwrap() = Some(ago(5));
        guard.last_output = ago(30);
        assert!(
            guard.stall_expired().is_none(),
            "only 5s since the hook ended"
        );
    }
}
