//! The attempt loop, gates, review, commit and failure rules (spec
//! "Engine"), plus `App`, which wires the `orch.*` methods to the store, git,
//! harness and agents. Pure
//! decision helpers live at the top with their own unit tests; `App` and
//! the async run loop are below.

use crate::brief;
use crate::events::Event;
use crate::git;
use crate::harness;
use crate::hook;
use crate::loop_detect::LoopDetector;
use crate::model::*;
use crate::overseer;
use crate::sandbox::{Inputs as SandboxInputs, TaskSandbox};
use crate::store::{self, Store};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, RwLock};
use std::time::Duration;
use tokio::sync::{broadcast, oneshot, watch, Notify, Semaphore};

#[path = "../audit.rs"]
mod audit;
#[path = "../chat.rs"]
pub(crate) mod chat;
#[path = "../evolution/mod.rs"]
mod evolution;
#[path = "../messages.rs"]
mod messages;

mod advisor;
mod answer_policy;
mod attempt_run;
mod availability;
mod backlog;
mod base_check;
mod baseline;
mod best_of;
mod brief_check;
mod chat_tools;
mod check_reuse;
pub mod compiler_cache;
mod cost;
mod criteria;
mod decision;
mod decision_memory;
mod deliverables;
mod digest;
mod finding_judge;
mod fresh_base;
mod graph;
mod healing;
mod hooks;
mod ladder;
mod land;
mod leases;
mod lines;
mod metrics;
mod owner_notice;
mod park;
mod past_work;
mod permissions;
mod pipeline;
mod plan;
mod plan_gate;
mod pr;
mod pr_draft;
mod protected;
mod questions;
mod rate_limit;
mod recovery;
mod repo_notes;
mod reports;
mod review;
mod review_findings;
mod routing;
mod rpc_misc;
mod rpc_tasks;
mod secrets;
mod slots;
mod stuck;
mod subtask_checks;
mod sync;
mod task_loop;
mod test_claim;
#[cfg(test)]
pub(crate) mod test_support;
mod time_budget;
pub mod verify;
pub use verify::to_sbpl as task_sandbox_profile;
mod wait_sla;
mod worktrees;

use advisor::*;
pub(crate) use answer_policy::owner_only;
use answer_policy::*;
use attempt_run::*;
use availability::*;
use backlog::*;
pub(crate) use base_check::is_environment_result;
use base_check::*;
use baseline::*;
use best_of::*;
use brief_check::*;
use check_reuse::*;
use cost::*;
use criteria::*;
use decision::*;
use decision_memory::*;
use finding_judge::*;
use graph::*;
use healing::*;
use land::*;
use leases::*;
use lines::*;
pub(crate) use owner_notice::{kind_name, option_part};
use plan::*;
use protected::*;
use questions::*;
use rate_limit::*;
use reports::*;
use review::*;
use review_findings::*;
use routing::*;
use rpc_tasks::*;
use slots::{take_slot, ResizableSlots, Slot, SlotPermit};
use stuck::*;
use task_loop::*;
#[cfg(test)]
use test_support::*;
use verify::*;

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

fn tail_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    s.chars().skip(count - max).collect()
}

/// P0: every task id that arrives as a request param must be a valid UUID
/// (the only shape `task.create` ever generates) *and* resolve to a path
/// that actually stays under `<data>/tasks` -- defense in depth, since a
/// non-UUID id can never produce a path outside that directory anyway, but
/// this makes the invariant an assertion rather than an assumption.
fn validate_task_id(store: &Store, id: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(id).map_err(|_| "invalid task id".to_string())?;
    let dir = store.task_dir(id);
    let tasks_root = store.data_dir().join("tasks");
    if !dir.starts_with(&tasks_root) {
        return Err("invalid task id".to_string());
    }
    Ok(())
}

/// FNV-1a 64-bit -- fast, dependency-free, plenty for a cache key (not a
/// security boundary).
pub(crate) fn simple_hash(s: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// A cancellation flag that (unlike a bare `Notify`) keeps its state: any
/// number of callers can synchronously ask `is_cancelled()` at any point --
/// "check it between every step" -- without racing a lost wakeup, and
/// `cancelled()` still resolves immediately for anyone who starts waiting
/// after `cancel()` already ran.
#[derive(Clone)]
struct CancelToken(Arc<CancelInner>);

struct CancelInner {
    flag: AtomicBool,
    notify: Notify,
    parent: Option<CancelToken>,
}

impl CancelToken {
    fn new() -> Self {
        CancelToken(Arc::new(CancelInner {
            flag: AtomicBool::new(false),
            notify: Notify::new(),
            parent: None,
        }))
    }

    fn child(&self) -> Self {
        CancelToken(Arc::new(CancelInner {
            flag: AtomicBool::new(false),
            notify: Notify::new(),
            parent: Some(self.clone()),
        }))
    }

    fn cancel(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    fn is_cancelled(&self) -> bool {
        let mut token = Some(self);
        while let Some(current) = token {
            if current.0.flag.load(Ordering::SeqCst) {
                return true;
            }
            token = current.0.parent.as_ref();
        }
        false
    }

    async fn cancelled(&self) {
        // Register interest *before* checking the flag, so a cancel() that
        // lands in between can't be missed (the documented tokio::Notify
        // pattern for exactly this race).
        let mut tokens = vec![self];
        while let Some(parent) = tokens.last().and_then(|t| t.0.parent.as_ref()) {
            tokens.push(parent);
        }
        let mut notified: Vec<_> = tokens
            .iter()
            .map(|t| Box::pin(t.0.notify.notified()))
            .collect();
        for wait in &mut notified {
            wait.as_mut().enable();
        }
        if self.is_cancelled() {
            return;
        }
        std::future::poll_fn(|cx| {
            for wait in &mut notified {
                if std::future::Future::poll(wait.as_mut(), cx).is_ready() {
                    return std::task::Poll::Ready(());
                }
            }
            std::task::Poll::Pending
        })
        .await;
    }
}

/// A Claude model profile's secrets (spec `secrets.set`): env vars merged
/// into the run's `settings.json`, and an API key written to a per-run key
/// file the daemon points `apiKeyHelper` at (never inlined into JSON on
/// disk).
#[derive(Debug, Clone, Deserialize, Default)]
struct ProfileSecret {
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct AccountSecret {
    kind: String,
    value: String,
}

/// In-memory only (spec: "travel from Electron to the daemon in memory
/// only"); `secrets.set` is a full replace, never a merge.
#[derive(Default)]
struct Secrets {
    profiles: HashMap<String, ProfileSecret>,
    accounts: HashMap<String, AccountSecret>,
    projects: HashMap<String, HashMap<String, String>>,
    project_mcp: HashMap<String, HashMap<String, String>>,
    /// Repo path -> project id, for a task made without one (the agent's own
    /// `task.create`): its project's values still reach it.
    repo_projects: HashMap<String, String>,
    /// The app has sent the values since this daemon started; until then
    /// every map above is empty, not "no values".
    received: bool,
    /// Repo path -> its project's default branch: what a task made with no
    /// `base` branches from.
    project_branches: HashMap<String, String>,
}

struct TaskControl {
    cancel: CancelToken,
    /// One-shot per question: created fresh by `wait_for_answer` each time
    /// the loop actually waits, taken (and consumed) by `task.answer`. No
    /// long-lived queue -- an answer that arrives when nobody is waiting is
    /// simply not delivered (`task.answer` rejects it before it gets here).
    pending_answer: Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    /// An owner's `task.amend` for a task whose loop holds its own copy of
    /// the task: the loop applies it at its next attempt boundary, so no
    /// save of the loop's copy can overwrite it.
    pending_amend: Arc<StdMutex<Option<Amendment>>>,
    handle: tokio::task::JoinHandle<()>,
}

struct HookContext {
    task_id: String,
    attempt_n: u32,
    repo: String,
    worktree: PathBuf,
    base_sha: String,
    verify: Vec<String>,
    blocks: AtomicU32,
    /// Cancelled when the attempt's run ends, so a Stop-hook verify still
    /// running for a killed (stalled) agent stops before the next attempt.
    cancel: CancelToken,
    /// Set while `hook.stop` runs verify for this attempt: the harness is
    /// silent then by design, so the stall clock waits.
    hook_running: Arc<HookClock>,
    /// Rules the owner allowed for this run only (`permissions.rs`).
    run_grants: StdMutex<std::collections::HashSet<String>>,
    /// Rules the owner denied during this run.
    run_denied: StdMutex<std::collections::HashSet<String>>,
    /// Tools and paths the hook decided with the owner, so a refusal of the
    /// same one after the run is not asked about again.
    handled: StdMutex<std::collections::BTreeSet<String>>,
    /// Protected paths the owner allowed: what the agent staged there is
    /// copied to the real path.
    staged: StdMutex<std::collections::BTreeSet<String>>,
    /// One permission question at a time per run.
    ask_lock: tokio::sync::Mutex<()>,
}

impl HookContext {
    fn new(task: &Task, attempt_n: u32, worktree: &Path, base_sha: &str) -> Arc<Self> {
        Arc::new(HookContext {
            task_id: task.id.clone(),
            attempt_n,
            repo: task.repo.clone(),
            worktree: worktree.to_path_buf(),
            base_sha: base_sha.to_string(),
            verify: task.verify.clone(),
            blocks: AtomicU32::new(0),
            cancel: CancelToken::new(),
            hook_running: Arc::default(),
            run_grants: Default::default(),
            run_denied: Default::default(),
            handled: Default::default(),
            staged: Default::default(),
            ask_lock: tokio::sync::Mutex::new(()),
        })
    }
}

/// An implement attempt's stall watchdog: the silence limit, paused while
/// the hook clock runs.
struct Stall {
    limit: Duration,
    paused: Arc<HookClock>,
}

/// Whether a hook runs now, and when the last one ended. The end counts as
/// activity: the stall window starts then.
#[derive(Default)]
struct HookClock {
    running: AtomicBool,
    ended: StdMutex<Option<tokio::time::Instant>>,
}

impl HookClock {
    fn start(&self) -> ClearOnDrop<'_> {
        self.running.store(true, Ordering::SeqCst);
        ClearOnDrop(self)
    }
}

struct ClearOnDrop<'a>(&'a HookClock);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        *self.0.ended.lock().unwrap() = Some(tokio::time::Instant::now());
        self.0.running.store(false, Ordering::SeqCst);
    }
}

pub struct App {
    pub store: Store,
    pub data_dir: PathBuf,
    /// The sushiAI home (`SUSHIAI_HOME`): where agents' `sushiai mcp` and
    /// `sushiai orch hook` find the daemon.
    pub home: PathBuf,
    /// The `sushiai` executable that agents run as `mcp` and `orch hook`.
    pub exe_path: String,
    settings: RwLock<Settings>,
    secrets: RwLock<Secrets>,
    events_tx: broadcast::Sender<Event>,
    controls: std::sync::Mutex<HashMap<String, TaskControl>>,
    /// Keyed by repo: the orchestrator chat turn running for it, if any.
    chat_turns: std::sync::Mutex<HashMap<String, CancelToken>>,
    /// How long an overseer turn may run before it is cancelled (ms).
    overseer_turn_timeout_ms: std::sync::atomic::AtomicU64,
    /// How long any task helper may run (ms).
    helper_timeout_ms: std::sync::atomic::AtomicU64,
    /// How long fetching a task's base branch may take (ms).
    base_fetch_timeout_ms: std::sync::atomic::AtomicU64,
    /// Keyed by audit id: the `repo.audit` runs still in flight.
    audits: std::sync::Mutex<HashMap<String, CancelToken>>,
    /// Every agent-to-agent message, mirrored to `<data>/messages.json`.
    messages: StdMutex<Vec<Message>>,
    hook_tokens: RwLock<HashMap<String, Arc<HookContext>>>,
    /// Keyed by task id: the held tool call a `task.answer` resolves.
    permission_waits: StdMutex<HashMap<String, oneshot::Sender<Reply>>>,
    /// Callers who just answered a task's parked question; the loop consumes
    /// the provenance after recording the answer.
    agent_answers: StdMutex<HashMap<String, AnsweredBy>>,
    cancellations: StdMutex<HashMap<String, park::Cancellation>>,
    /// Keyed by repo: overseer wakes skipped while its chat was busy; each
    /// fires when that repo's turn ends.
    deferred_wakes: StdMutex<HashMap<String, Vec<chat::DeferredWake>>>,
    /// Keyed by task id: the files each live task holds (`leases.rs`).
    leases: StdMutex<HashMap<String, Lease>>,
    /// Held while a graph start counts a parent's running children and
    /// spawns one more, so `childParallel` is never exceeded by a race.
    child_admission: StdMutex<()>,
    /// Held while the autopilot counts live loops and starts backlog tasks,
    /// so `parallel` is never exceeded by a race.
    autopilot_admission: StdMutex<()>,
    /// Keyed by task id: the diff+untracked-list hash a verify run was last
    /// computed for, and its completed results, shared by `hook.stop` and
    /// the post-session gate. Interrupted result sets are never stored.
    verify_cache: std::sync::Mutex<HashMap<String, (String, Vec<VerifyOutcome>)>>,
    /// Keyed by (base sha, command): what a command did on that commit, so an
    /// unchanged base is never checked twice for the same command. Kept on
    /// disk (`base-results.json`), so a restart does not run them again; an
    /// interrupted, cancelled or environment-failed run is never stored.
    base_runs: std::sync::Mutex<HashMap<(String, String), VerifyOutcome>>,
    /// Exit-0 checks on exact worktree trees, in insertion order, memory only.
    gate_passes: StdMutex<PassedChecks>,
    /// Retry decisions from background base runs, keyed by (base sha, command).
    base_run_decisions: StdMutex<HashMap<(String, String), Vec<String>>>,
    /// Keyed by (base sha, command): background base runs in flight. The
    /// receiver flips to `true` when the job is over; its result, if any, is
    /// then in `base_runs`. Concurrent lookups share the same job.
    base_jobs: StdMutex<HashMap<(String, String), watch::Receiver<bool>>>,
    /// Final-check job receipts for each live task, including jobs that
    /// finished without a base result. A final lookup waits for its original
    /// job instead of starting another one just because nothing was cached.
    final_base_jobs: StdMutex<HashMap<String, base_check::BaseJobReceivers>>,
    /// Pairs whose first base run failed and whose rerun passed (load-flaky).
    base_flaky: StdMutex<std::collections::HashSet<(String, String)>>,
    /// Cancelled by `shutdown_for` only: a task stop must not end a job
    /// another task may be waiting on.
    base_cancel: CancelToken,
    /// Keyed by task id: the task's base preflight, a background job that
    /// holds no slot (`healing.rs`). The loop applies its result.
    base_preflights: StdMutex<HashMap<String, Arc<PreflightJob>>>,
    /// Which harness CLIs the last probe found (`availability.rs`).
    harness_avail: StdMutex<HashMap<Harness, bool>>,
    /// Tasks the timer already resumed from a missing harness; a spawn that
    /// fails again asks the owner instead of resuming every tick.
    harness_resumed: StdMutex<std::collections::HashSet<String>>,
    /// Live task-loop limit. Shrinking retires free permits first, then
    /// held permits as their owners finish.
    slots: Arc<ResizableSlots>,
    /// Shared by all check runners on this daemon, independent of agent slots.
    check_slots: Arc<ResizableSlots>,
    check_load: RwLock<fn() -> (f64, u32)>,
    /// Hook and background check lines awaiting the loop's own Task copy.
    check_decisions: StdMutex<HashMap<String, Vec<String>>>,
    /// Keyed by parent task id: held while one child lands on the parent's
    /// branch, so landings onto one branch happen one at a time.
    landing_locks: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    task_locks: StdMutex<HashMap<String, Arc<StdMutex<()>>>>,
    recovery_timers_started: AtomicBool,
    /// Keyed by repo: one base-commit check runs at a time per repo, so a
    /// rerun of a failed base check never competes with another one.
    base_check_locks: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Set by `shutdown()`: the loops it cancels end `stopped`, and that
    /// must not read as a dependency ending.
    shutting_down: AtomicBool,
    /// Set with `shutting_down` when the module is switched off: no agent run outlives it.
    disabling: AtomicBool,
    /// Held across the check-and-append of evolution signal detection so one
    /// task is never recorded twice.
    evolution_lock: tokio::sync::Mutex<()>,
    /// Keyed by cluster key: the `evolution.run` proposer runs still in
    /// flight.
    proposals: std::sync::Mutex<HashMap<String, CancelToken>>,
    /// Held across a read-modify-write of a stored proposal.
    proposal_lock: tokio::sync::Mutex<()>,
    /// Held across a read-modify-write of `repo-notes.json`.
    notes_lock: tokio::sync::Mutex<()>,
    self_ref: OnceLock<std::sync::Weak<App>>,
}

/// The start of the error message for a method this engine does not have.
pub const UNKNOWN_METHOD: &str = "unknown method: ";

fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

fn resolve_binary(harness: Harness) -> String {
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
fn augmented_path() -> String {
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

fn merge_paths(own: &str, login: &str, home: &str) -> String {
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
fn login_shell_path(shell: &str, limit: std::time::Duration) -> Option<String> {
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

fn append_line(path: &Path, line: &str) {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Loads the task, applies `f` to the named attempt if it's still there,
/// and saves + broadcasts -- used to persist `session_id`/`pgid` as soon as
/// they're known, mid-run, rather than only after the harness exits (so a
/// killed daemon can still recover/resume from them).
async fn persist_attempt_field(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    f: impl FnOnce(&mut Attempt),
) {
    if let Err(e) = app.update_task(task_id, |task| {
        if let Some(a) = task.attempts.iter_mut().find(|a| a.n == attempt_n) {
            f(a);
        }
    }) {
        eprintln!("orch: updating task {task_id} attempt {attempt_n} failed: {e}");
    }
}

impl App {
    fn helper_timeout(&self) -> Duration {
        Duration::from_millis(self.helper_timeout_ms.load(Ordering::SeqCst))
    }

    pub(super) fn task_sandbox(
        &self,
        repo: &str,
        worktree: &Path,
        run_dir: &Path,
    ) -> Option<TaskSandbox> {
        let worktree_root = {
            let settings = self.settings.read().unwrap();
            if settings.sandbox_for(repo) == SandboxMode::Host {
                return None;
            }
            settings.worktree_root.clone()
        };
        let mut projects: Vec<PathBuf> = self
            .secrets
            .read()
            .unwrap()
            .repo_projects
            .keys()
            .map(PathBuf::from)
            .collect();
        let tasks = self.store.list_tasks().unwrap_or_default();
        projects.extend(tasks.iter().map(|task| PathBuf::from(&task.repo)));
        let worktrees: Vec<PathBuf> = tasks
            .iter()
            .filter(|task| !task.worktree.is_empty())
            .map(|task| PathBuf::from(&task.worktree))
            .collect();
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME for task sandbox"));
        let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from);
        let rustup_home = std::env::var_os("RUSTUP_HOME").map(PathBuf::from);
        let tmp_dir = std::env::var_os("TMPDIR").map(PathBuf::from);
        let npm_cache = std::env::var_os("npm_config_cache")
            .or_else(|| std::env::var_os("NPM_CONFIG_CACHE"))
            .map(PathBuf::from);
        Some(TaskSandbox::build(SandboxInputs {
            repo_root: Path::new(repo),
            worktree,
            run_dir,
            data_dir: &self.data_dir,
            sushiai_home: &self.home,
            worktree_root: &worktree_root,
            home: &home,
            daemon_socket: &self.home.join("daemon.sock"),
            cargo_home: cargo_home.as_deref(),
            rustup_home: rustup_home.as_deref(),
            tmp_dir: tmp_dir.as_deref(),
            npm_cache: npm_cache.as_deref(),
            known_project_folders: &projects,
            known_task_worktrees: &worktrees,
        }))
    }

    pub fn new(data_dir: PathBuf, home: PathBuf, exe_path: String) -> std::io::Result<Arc<App>> {
        crate::prompts::set_data_dir(&data_dir);
        let store = Store::new(&data_dir)?;
        let settings = store.load_settings()?;
        let (events_tx, _) = broadcast::channel(1024);
        let slots = ResizableSlots::new(settings.parallel.max(1) as usize);
        let check_slots = ResizableSlots::new(settings.heavy_check_limit() as usize);

        let messages = messages::load(&data_dir);
        let base_runs_dir = data_dir.clone();
        let app = Arc::new(App {
            store,
            data_dir,
            home,
            exe_path,
            settings: RwLock::new(settings),
            secrets: RwLock::new(Secrets::default()),
            events_tx,
            controls: std::sync::Mutex::new(HashMap::new()),
            chat_turns: std::sync::Mutex::new(HashMap::new()),
            overseer_turn_timeout_ms: std::sync::atomic::AtomicU64::new(15 * 60 * 1000),
            helper_timeout_ms: std::sync::atomic::AtomicU64::new(15 * 60 * 1000),
            base_fetch_timeout_ms: std::sync::atomic::AtomicU64::new(
                fresh_base::FETCH_TIMEOUT.as_millis() as u64,
            ),
            audits: std::sync::Mutex::new(HashMap::new()),
            messages: StdMutex::new(messages),
            hook_tokens: RwLock::new(HashMap::new()),
            permission_waits: StdMutex::new(HashMap::new()),
            agent_answers: StdMutex::new(Default::default()),
            cancellations: StdMutex::new(Default::default()),
            deferred_wakes: StdMutex::new(HashMap::new()),
            leases: StdMutex::new(HashMap::new()),
            child_admission: StdMutex::new(()),
            autopilot_admission: StdMutex::new(()),
            verify_cache: std::sync::Mutex::new(HashMap::new()),
            base_runs: std::sync::Mutex::new(base_check::load_base_runs(&base_runs_dir)),
            gate_passes: StdMutex::new(Default::default()),
            base_run_decisions: StdMutex::new(HashMap::new()),
            base_jobs: StdMutex::new(HashMap::new()),
            final_base_jobs: StdMutex::new(HashMap::new()),
            base_flaky: StdMutex::new(Default::default()),
            base_cancel: CancelToken::new(),
            base_preflights: StdMutex::new(HashMap::new()),
            harness_avail: StdMutex::new(HashMap::new()),
            harness_resumed: StdMutex::new(std::collections::HashSet::new()),
            slots,
            check_slots,
            check_load: RwLock::new(verify::machine_load),
            check_decisions: StdMutex::new(HashMap::new()),
            landing_locks: StdMutex::new(HashMap::new()),
            task_locks: StdMutex::new(HashMap::new()),
            recovery_timers_started: AtomicBool::new(false),
            base_check_locks: StdMutex::new(HashMap::new()),
            shutting_down: AtomicBool::new(false),
            disabling: AtomicBool::new(false),
            evolution_lock: tokio::sync::Mutex::new(()),
            proposals: std::sync::Mutex::new(HashMap::new()),
            proposal_lock: tokio::sync::Mutex::new(()),
            notes_lock: tokio::sync::Mutex::new(()),
            self_ref: OnceLock::new(),
        });
        let _ = app.self_ref.set(Arc::downgrade(&app));
        app.refresh_availability();
        set_verify_timeout_secs(app.settings.read().unwrap().verify_timeout_secs);
        Ok(app)
    }

    fn arc(&self) -> Arc<App> {
        self.self_ref
            .get()
            .and_then(|w| w.upgrade())
            .expect("App is always constructed behind an Arc")
    }

    fn task_lock(&self, id: &str) -> Arc<StdMutex<()>> {
        self.task_locks
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    pub(crate) fn save_task_logged(&self, task: &Task) {
        let lock = self.task_lock(&task.id);
        let _guard = lock.lock().unwrap();
        let kept;
        let mut task = task;
        if let Ok(Some(stored)) = self.store.load_task(&task.id) {
            // A loop saves its own in-memory copy; one that was cancelled a
            // moment ago must not put a stop or a park back to running.
            let suspended = stored.queue.stopped_explicitly || stored.queue.park.is_some();
            let live = matches!(
                task.status,
                TaskStatus::Running
                    | TaskStatus::Drafting
                    | TaskStatus::Waiting
                    | TaskStatus::Landing
            );
            if suspended
                && live
                && matches!(stored.status, TaskStatus::Stopped | TaskStatus::Queued)
            {
                let mut copy = task.clone();
                copy.status = stored.status;
                copy.question = None;
                copy.queue.park = stored.queue.park;
                copy.queue.stopped_explicitly = stored.queue.stopped_explicitly;
                for line in stored.decisions {
                    if !copy.decisions.contains(&line) {
                        copy.decisions.push(line);
                    }
                }
                for answer in stored.question_history {
                    if !copy.question_history.contains(&answer) {
                        copy.question_history.push(answer);
                    }
                }
                kept = copy;
                task = &kept;
            }
        }
        if let Err(e) = self.store.save_task(task) {
            eprintln!("orch: saving task {} failed: {e}", task.id);
        }
    }

    pub(crate) fn update_task(&self, id: &str, f: impl FnOnce(&mut Task)) -> Result<Task, String> {
        let lock = self.task_lock(id);
        let _guard = lock.lock().unwrap();
        let mut task = self
            .store
            .load_task(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        f(&mut task);
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        Ok(task)
    }

    #[cfg(test)]
    pub(crate) fn recovery_timers_started(&self) -> bool {
        self.recovery_timers_started.load(Ordering::SeqCst)
    }

    pub fn broadcast_task(&self, task: &Task) {
        let _ = self.events_tx.send(Event::Task {
            task: Box::new(task.clone()),
        });
    }

    /// Appends one overseer action to `overseer.jsonl` and broadcasts it.
    pub(crate) fn record_overseer(&self, entry: crate::overseer::Entry) {
        if crate::overseer::append(&self.data_dir, &entry).is_ok() {
            let _ = self.events_tx.send(Event::Overseer {
                entry: Box::new(entry),
            });
        }
    }

    pub fn broadcast_log(&self, task_id: &str, attempt: u32, line: String) {
        let _ = self.events_tx.send(Event::Log {
            task_id: task_id.to_string(),
            attempt,
            line,
        });
    }

    /// Cancels every live task loop (each one's own `run_harness`/verify
    /// call kills its child's process group on seeing this), every live
    /// orchestrator chat turn (`chat::run` kills its child the same way on
    /// seeing its own `CancelToken`). Called when the daemon stops -- a chat turn's `setsid`'d
    /// child is just as capable of leaking past the daemon exiting as a task
    /// attempt's, so it needs the same cancel-on-shutdown treatment.
    pub fn shutdown(&self) {
        self.shutdown_for(false);
    }

    /// `true` while a daemon shutdown leaves agent runs going for the next daemon to adopt.
    pub(super) fn runs_survive_shutdown(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst) && !self.disabling.load(Ordering::SeqCst)
    }

    /// `shutdown`; `disabling` also kills every agent run (see `runs_survive_shutdown`).
    pub fn shutdown_for(&self, disabling: bool) {
        self.disabling.store(disabling, Ordering::SeqCst);
        self.shutting_down.store(true, Ordering::SeqCst);
        self.deferred_wakes.lock().unwrap().clear();
        for ctrl in self.controls.lock().unwrap().values() {
            ctrl.cancel.cancel();
        }
        for cancel in self.chat_turns.lock().unwrap().values() {
            cancel.cancel();
        }
        for cancel in self.audits.lock().unwrap().values() {
            cancel.cancel();
        }
        for cancel in self.proposals.lock().unwrap().values() {
            cancel.cancel();
        }
        self.base_cancel.cancel();
    }

    /// The status a task takes when its loop is cancelled: `stopped` for an
    /// owner's stop, but a daemon shutdown (upgrade, SIGTERM) leaves the task
    /// in the state crash recovery already resumes -- a running task goes back
    /// to `queued`, a drafting or waiting one stays as it is.
    pub(super) fn cancelled_status(&self, current: &TaskStatus) -> TaskStatus {
        if !self.shutting_down.load(Ordering::SeqCst) {
            return TaskStatus::Stopped;
        }
        match current {
            TaskStatus::Running => TaskStatus::Queued,
            other => *other,
        }
    }

    pub(super) fn close_stopped_task(&self, task: &mut Task) {
        if let Some(question) = task.question.take() {
            task.decisions.push(format!(
                "Stop: closed the open question \"{}\"",
                truncate_chars(&question.text, 200)
            ));
        }
        self.leases.lock().unwrap().remove(&task.id);
        self.permission_waits.lock().unwrap().remove(&task.id);
        task.queue.dirty_since = None;
    }

    /// Registers `token` for a run's Stop hook and keeps it in the run's token
    /// file, so a restarted daemon can register it again for a run it re-adopts.
    fn register_hook(&self, token: &str, ctx: &Arc<HookContext>, run_dir: &Path) {
        let file = RunFiles::of(&run_dir.join("events.jsonl")).token;
        let _ = store::write_secret_file(&file, token);
        self.hook_tokens
            .write()
            .unwrap()
            .insert(token.to_string(), ctx.clone());
    }

    /// Forgets a token in memory only; its file stays for the next daemon.
    fn unload_hook(&self, token: &str) {
        self.hook_tokens.write().unwrap().remove(token);
    }

    /// Forgets the token of a run that ended, in memory and on disk.
    fn forget_hook(&self, token: &str, run_dir: &Path) {
        self.unload_hook(token);
        let _ = std::fs::remove_file(RunFiles::of(&run_dir.join("events.jsonl")).token);
    }

    /// The hook token registered for attempt `n` of a task, if its run has one.
    pub(super) fn hook_token_of(&self, task_id: &str, n: u32) -> Option<String> {
        let tokens = self.hook_tokens.read().unwrap();
        tokens
            .iter()
            .find(|(_, ctx)| ctx.task_id == task_id && ctx.attempt_n == n)
            .map(|(token, _)| token.clone())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events_tx.subscribe()
    }

    /// Whether any task loop is still winding down after `shutdown()` --
    /// `Orch::shutdown_with` polls this briefly before the daemon exits, so
    /// a cancelled child gets a real chance to be killed instead of just
    /// orphaned by the daemon disappearing out from under it.
    pub fn any_task_loop_running(&self) -> bool {
        !self.controls.lock().unwrap().is_empty()
    }

    /// Same as `any_task_loop_running`, but for orchestrator chat turns:
    /// `chat::run_turn` only removes its `chat_turns` entry once `run()` has
    /// returned (child killed and reaped, or exited on its own), so an
    /// empty map here means there is no live chat child left to leak.
    pub fn any_chat_turn_running(&self) -> bool {
        !self.chat_turns.lock().unwrap().is_empty()
    }

    /// Same again for `repo.audit` and evolution proposer runs: an entry
    /// goes only once its run has returned.
    pub fn any_audit_running(&self) -> bool {
        !self.audits.lock().unwrap().is_empty() || !self.proposals.lock().unwrap().is_empty()
    }

    /// Same again for background base runs: a key leaves `base_jobs` only
    /// after its run returned and its scratch worktree is gone.
    pub fn any_base_job_running(&self) -> bool {
        !self.base_jobs.lock().unwrap().is_empty()
    }

    /// Appends a line to a task's `decisions` and persists it right away --
    /// for call sites that don't otherwise hold the task in memory. Call sites that
    /// already hold `&mut Task` push the line onto their own copy instead, so
    /// it rides along with their next save instead of racing a reload.
    fn append_decision(&self, task_id: &str, line: String) {
        if let Err(e) = self.update_task(task_id, |task| record_check_decisions(task, vec![line])) {
            eprintln!("orch: updating task {task_id} failed: {e}");
        }
    }

    // -- orch.* methods ----------------------------------------------------

    /// Runs one `orch.<method>` call. An unknown method answers with
    /// [`UNKNOWN_METHOD`] as the start of its message.
    pub async fn dispatch(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match method {
            "settings.get" => self.handle_settings_get().await,
            "settings.set" => self.handle_settings_set(params).await,
            "settings.defaults" => self.handle_settings_defaults().await,
            "secrets.set" => self.handle_secrets_set(params).await,
            "task.list" => self.handle_task_list(params).await,
            "task.get" => self.handle_task_get(params).await,
            "task.create" => self.handle_task_create(params).await,
            "task.start" => self.handle_task_start(params).await,
            "task.backlog" => self.handle_task_backlog(params).await,
            "task.land" => self.handle_task_land(params).await,
            "task.pr" => self.handle_task_pr(params).await,
            "task.stop" => self.handle_task_stop(params).await,
            "task.park" => self.handle_task_park(params).await,
            "task.sync" => self.handle_task_sync(params).await,
            "task.answer" => self.handle_task_answer(params).await,
            "task.report" => self.handle_task_report(params).await,
            "task.leadTouch" => self.handle_task_lead_touch(params).await,
            "task.overturn" => self.handle_task_overturn(params).await,
            "task.amend" => self.handle_task_amend(params).await,
            "task.preset" => self.handle_task_preset(params).await,
            "pipeline.describe" => self.handle_pipeline_describe(params).await,
            "task.delete" => self.handle_task_delete(params).await,
            "costs.summary" => self.handle_costs_summary(params).await,
            "overseer.log" => self.handle_overseer_log(params).await,
            "worktrees.gc" => self.handle_worktrees_gc(params).await,
            "task.archive" => self.handle_task_archive(params).await,
            "task.unarchive" => self.handle_task_unarchive(params).await,
            "task.timeline" => {
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                validate_task_id(&self.store, id)?;
                crate::timeline::timeline_json(&self.store, id)
            }
            "task.evidence" => self.handle_task_evidence(params).await,
            "task.openDeliverable" => self.handle_task_open_deliverable(params).await,
            "task.log" => self.handle_task_log(params).await,
            "failures.catalogue" => crate::timeline::catalogue_json(&self.store, &params),
            "chat.get" => chat::handle_get(self, params).await,
            "chat.send" => chat::handle_send(self, params).await,
            "chat.edit" => chat::handle_edit(self, params).await,
            "chat.cancel" => chat::handle_cancel(self, params).await,
            "chat.list" => chat::handle_list(self, params).await,
            "chat.new" => chat::handle_new(self, params).await,
            "chat.switch" => chat::handle_switch(self, params).await,
            "chat.clear" => chat::handle_clear(self, params).await,
            "chat.createProposal" => chat::handle_create_proposal(self, params).await,
            "chat.tools" => chat_tools::handle_tools(self, params).await,
            "chat.toolServers" => chat_tools::handle_tool_servers(self, params).await,
            "chat.actionSend" => chat::handle_action_send(self, params).await,
            "chat.actionDecline" => chat::handle_action_decline(self, params).await,
            "peer.list" => messages::handle_peers(self, params).await,
            "message.send" => messages::handle_send(self, params).await,
            "message.inbox" => messages::handle_inbox(self, params).await,
            "message.list" => messages::handle_list(self, params).await,
            "repo.audit" => audit::handle_start(self, params).await,
            "repo.audit.get" => audit::handle_get(self, params).await,
            "repo.audit.list" => audit::handle_list(self, params).await,
            "evolution.run" => self.handle_evolution_run().await,
            "evolution.list" => self.handle_evolution_list(params).await,
            "evolution.approve" => self.handle_evolution_approve(params).await,
            "evolution.reject" => self.handle_evolution_reject(params).await,
            "evolution.adopt" => self.handle_evolution_adopt(params).await,
            "repo.notes.list" => self.handle_repo_notes_list(params).await,
            "repo.notes.add" => self.handle_repo_notes_add(params).await,
            "repo.notes.remove" => self.handle_repo_notes_remove(params).await,
            "precedents.list" => self.handle_precedents_list(params).await,
            "precedents.remove" => self.handle_precedents_remove(params).await,
            "precedents.rule.accept" => self.handle_precedents_rule_accept(params).await,
            "precedents.rule.dismiss" => self.handle_precedents_rule_dismiss(params).await,
            "hook.stop" => self.handle_hook_stop(params).await,
            "hook.edit" => self.handle_hook_edit(params).await,
            other => Err(format!("{UNKNOWN_METHOD}{other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests live beside each concern in `tests/<file>.rs` and are included here so
    //! the qualified test names stay `engine::tests::*`.
    use super::*;

    include!("tests/availability.rs");
    include!("tests/cost.rs");
    include!("tests/decision.rs");
    include!("tests/graph.rs");
    include!("tests/lines.rs");
    include!("tests/core.rs");
    include!("tests/plan.rs");
    include!("tests/recovery.rs");
    include!("tests/review.rs");
    include!("tests/routing.rs");
    include!("tests/rpc_tasks.rs");
    include!("tests/stop.rs");
    include!("tests/verify.rs");
    include!("tests/worktrees.rs");
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn the_login_shell_path_is_read_after_its_marker_and_merged_once() {
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("fake-shell");
        std::fs::write(
            &shell,
            "#!/bin/sh\necho 'welcome banner'\nPATH=/login/bin:/usr/bin\neval \"$2\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        // A script just written can fail to exec with ETXTBSY while a parallel
        // test forks (Linux); the spawn then reads as "no shell". Retry that.
        let login = (0..5).find_map(|_| {
            login_shell_path(shell.to_str().unwrap(), std::time::Duration::from_secs(5))
        });
        assert_eq!(login.as_deref(), Some("/login/bin:/usr/bin"));
        let merged = merge_paths("/usr/bin:/bin", login.as_deref().unwrap(), "/h");
        assert!(merged.starts_with("/usr/bin:/bin:/login/bin:"), "{merged}");
        assert!(merged.contains("/h/.cargo/bin"));
        assert_eq!(merged.matches("/usr/bin").count(), 1);
    }

    #[test]
    fn a_shell_that_hangs_or_is_missing_gives_no_path() {
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("slow-shell");
        std::fs::write(&shell, "#!/bin/sh\nsleep 5\n").unwrap();
        std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let slow = login_shell_path(
            shell.to_str().unwrap(),
            std::time::Duration::from_millis(200),
        );
        assert!(slow.is_none());
        assert!(login_shell_path("/nope/shell", std::time::Duration::from_secs(1)).is_none());
        assert!(login_shell_path("", std::time::Duration::from_secs(1)).is_none());
    }
}
