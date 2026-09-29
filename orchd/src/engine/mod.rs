//! The attempt loop, gates, review, commit and failure rules (spec
//! "Engine"), plus the `Dispatcher` implementation (`App`) that wires the
//! protocol methods to the store, git, harness and agents. Pure
//! decision helpers live at the top with their own unit tests; `App` and
//! the async run loop are below.

use crate::brief;
use crate::git;
use crate::harness;
use crate::hook;
use crate::loop_detect::LoopDetector;
use crate::model::*;
use crate::protocol::{CallFuture, Dispatcher, Event};
use crate::store::{self, Store};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, RwLock};
use std::time::Duration;
use tokio::sync::{broadcast, oneshot, Notify, Semaphore};

#[path = "../audit.rs"]
mod audit;
#[path = "../chat.rs"]
mod chat;
#[path = "../evolution/mod.rs"]
mod evolution;
#[path = "../messages.rs"]
mod messages;

mod advisor;
mod answer_policy;
mod attempt_run;
mod base_check;
mod best_of;
mod brief_check;
mod cost;
mod decision;
mod finding_judge;
mod graph;
mod healing;
mod hooks;
mod land;
mod leases;
mod lines;
mod past_work;
mod plan;
mod questions;
mod recovery;
mod repo_notes;
mod reports;
mod review;
mod routing;
mod rpc_misc;
mod rpc_tasks;
mod task_loop;
#[cfg(test)]
mod test_support;
mod verify;
mod worktrees;

use advisor::*;
use answer_policy::*;
use attempt_run::*;
use base_check::*;
use best_of::*;
use brief_check::*;
use cost::*;
use decision::*;
use finding_judge::*;
use graph::*;
use healing::*;
use land::*;
use leases::*;
use lines::*;
use plan::*;
use questions::*;
use review::*;
use routing::*;
use rpc_tasks::*;
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
fn simple_hash(s: &str) -> String {
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
}

impl CancelToken {
    fn new() -> Self {
        CancelToken(Arc::new(CancelInner {
            flag: AtomicBool::new(false),
            notify: Notify::new(),
        }))
    }

    fn cancel(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        // Register interest *before* checking the flag, so a cancel() that
        // lands in between can't be missed (the documented tokio::Notify
        // pattern for exactly this race).
        let notified = self.0.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
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

/// In-memory only (spec: "travel from Electron to the daemon in memory
/// only"); `secrets.set` is a full replace, never a merge.
#[derive(Default)]
struct Secrets {
    profiles: HashMap<String, ProfileSecret>,
}

struct TaskControl {
    cancel: CancelToken,
    /// One-shot per question: created fresh by `wait_for_answer` each time
    /// the loop actually waits, taken (and consumed) by `task.answer`. No
    /// long-lived queue -- an answer that arrives when nobody is waiting is
    /// simply not delivered (`task.answer` rejects it before it gets here).
    pending_answer: Arc<StdMutex<Option<oneshot::Sender<String>>>>,
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
    hook_running: Arc<AtomicBool>,
}

/// An implement attempt's stall watchdog: the silence limit, paused while
/// `paused` is set.
struct Stall {
    limit: Duration,
    paused: Arc<AtomicBool>,
}

struct ClearOnDrop<'a>(&'a AtomicBool);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub struct App {
    pub store: Store,
    pub data_dir: PathBuf,
    pub socket_path: PathBuf,
    pub orchd_path: String,
    settings: RwLock<Settings>,
    secrets: RwLock<Secrets>,
    events_tx: broadcast::Sender<Event>,
    shutdown_tx: broadcast::Sender<()>,
    controls: std::sync::Mutex<HashMap<String, TaskControl>>,
    /// Keyed by repo: the orchestrator chat turn running for it, if any.
    chat_turns: std::sync::Mutex<HashMap<String, CancelToken>>,
    /// Keyed by audit id: the `repo.audit` runs still in flight.
    audits: std::sync::Mutex<HashMap<String, CancelToken>>,
    /// Every agent-to-agent message, mirrored to `<data>/messages.json`.
    messages: StdMutex<Vec<Message>>,
    hook_tokens: RwLock<HashMap<String, Arc<HookContext>>>,
    /// Keyed by task id: the files each live task holds (`leases.rs`).
    leases: StdMutex<HashMap<String, Lease>>,
    /// Held while a graph start counts a parent's running children and
    /// spawns one more, so `childParallel` is never exceeded by a race.
    child_admission: StdMutex<()>,
    /// Keyed by task id: the diff+untracked-list hash a verify run was last
    /// computed for, and its results -- shared by `hook.stop` and the
    /// post-session gate so an unchanged diff never re-runs verify twice.
    verify_cache: std::sync::Mutex<HashMap<String, (String, Vec<VerifyOutcome>)>>,
    /// Keyed by (base sha, command): what a command did on that commit, so an
    /// unchanged base is never checked twice for the same command.
    base_runs: std::sync::Mutex<HashMap<(String, String), VerifyOutcome>>,
    pid: u32,
    /// The executable's mtime at startup, so the app can tell a rebuilt
    /// binary from the one this daemon is running.
    binary_mtime_ms: u64,
    /// Random per-start control-channel secret (`<data>/control.token`,
    /// mode 0600): every request but `ping`/`hook.stop` must carry it.
    control_token: String,
    /// Concurrency limit (`settings.parallel` at startup -- a live
    /// `settings.set` change takes effect on the next restart, not
    /// immediately; resizing a `Semaphore` down isn't a thing tokio
    /// supports, and the spec explicitly allows skipping this).
    slots: Arc<Semaphore>,
    parallel_limit: u32,
    /// Keyed by parent task id: held while one child lands on the parent's
    /// branch, so landings onto one branch happen one at a time.
    landing_locks: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Set by `shutdown()`: the loops it cancels end `stopped`, and that
    /// must not read as a dependency ending.
    shutting_down: AtomicBool,
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

fn generate_control_token() -> String {
    // 32 bytes of randomness as 64 hex chars, built from two v4 UUIDs
    // rather than a `rand` dependency the crate list doesn't include --
    // `uuid`'s v4 feature already pulls in a real CSPRNG (`getrandom`).
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

impl Dispatcher for App {
    fn call<'a>(&'a self, method: String, params: serde_json::Value) -> CallFuture<'a> {
        Box::pin(async move { self.dispatch(&method, params).await })
    }

    fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events_tx.subscribe()
    }

    fn check_auth(&self, method: &str, auth: Option<&str>) -> bool {
        App::check_auth(self, method, auth)
    }
}

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

fn augmented_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut extra = vec![format!("{home}/.local/bin")];
    #[cfg(target_os = "macos")]
    extra.push("/opt/homebrew/bin".to_string());
    extra.push("/usr/local/bin".to_string());
    let base = std::env::var("PATH").unwrap_or_default();
    let mut parts: Vec<String> = base.split(':').map(|s| s.to_string()).collect();
    for e in extra {
        if !parts.contains(&e) {
            parts.push(e);
        }
    }
    parts.join(":")
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
    if let Ok(Some(mut t)) = app.store.load_task(task_id) {
        if let Some(a) = t.attempts.iter_mut().find(|a| a.n == attempt_n) {
            f(a);
            t.updated_at = now_ms();
            let _ = app.store.save_task(&t);
            app.broadcast_task(&t);
        }
    }
}

impl App {
    pub fn new(
        data_dir: PathBuf,
        socket_path: PathBuf,
        orchd_path: String,
    ) -> std::io::Result<Arc<App>> {
        let store = Store::new(&data_dir)?;
        let settings = store.load_settings()?;
        let (events_tx, _) = broadcast::channel(1024);
        let (shutdown_tx, _) = broadcast::channel(4);
        let binary = PathBuf::from(&orchd_path);
        let binary_mtime_ms = std::fs::metadata(&binary)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let control_token = generate_control_token();
        store::write_secret_file(
            &data_dir.join("control.token"),
            &format!("{control_token}\n"),
        )?;

        let parallel_limit = settings.parallel.max(1);
        let slots = Arc::new(Semaphore::new(parallel_limit as usize));

        let messages = messages::load(&data_dir);
        let app = Arc::new(App {
            store,
            data_dir,
            socket_path,
            orchd_path,
            settings: RwLock::new(settings),
            secrets: RwLock::new(Secrets::default()),
            events_tx,
            shutdown_tx,
            controls: std::sync::Mutex::new(HashMap::new()),
            chat_turns: std::sync::Mutex::new(HashMap::new()),
            audits: std::sync::Mutex::new(HashMap::new()),
            messages: StdMutex::new(messages),
            hook_tokens: RwLock::new(HashMap::new()),
            leases: StdMutex::new(HashMap::new()),
            child_admission: StdMutex::new(()),
            verify_cache: std::sync::Mutex::new(HashMap::new()),
            base_runs: std::sync::Mutex::new(HashMap::new()),
            pid: std::process::id(),
            binary_mtime_ms,
            control_token,
            slots,
            parallel_limit,
            landing_locks: StdMutex::new(HashMap::new()),
            shutting_down: AtomicBool::new(false),
            evolution_lock: tokio::sync::Mutex::new(()),
            proposals: std::sync::Mutex::new(HashMap::new()),
            proposal_lock: tokio::sync::Mutex::new(()),
            notes_lock: tokio::sync::Mutex::new(()),
            self_ref: OnceLock::new(),
        });
        let _ = app.self_ref.set(Arc::downgrade(&app));
        Ok(app)
    }

    fn arc(&self) -> Arc<App> {
        self.self_ref
            .get()
            .and_then(|w| w.upgrade())
            .expect("App is always constructed behind an Arc")
    }

    pub fn broadcast_task(&self, task: &Task) {
        let _ = self.events_tx.send(Event::Task {
            task: Box::new(task.clone()),
        });
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
    /// seeing its own `CancelToken`), and stops the socket server. Called
    /// for both the `shutdown` RPC and SIGTERM -- a chat turn's `setsid`'d
    /// child is just as capable of leaking past the daemon exiting as a task
    /// attempt's, so it needs the same cancel-on-shutdown treatment.
    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
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
        let _ = self.shutdown_tx.send(());
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

    pub fn subscribe_shutdown(&self) -> broadcast::Receiver<()> {
        self.shutdown_tx.subscribe()
    }

    /// Whether any task loop is still winding down after `shutdown()` --
    /// `main.rs` polls this briefly before actually exiting the process, so
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

    /// `ping`/`hook.stop`/`hook.edit` are the only methods reachable without
    /// the control token: `ping` so Electron can probe/tell daemons apart
    /// before it has read the token file, the hooks because they are only
    /// ever invoked by `orchd hook ...` over the same trusted local
    /// machine, matched by their own per-run token instead.
    fn check_auth(&self, method: &str, auth: Option<&str>) -> bool {
        if matches!(method, "ping" | "hook.stop" | "hook.edit") {
            return true;
        }
        auth.map(|a| a == self.control_token).unwrap_or(false)
    }

    /// Appends a line to a task's `decisions` and persists it right away --
    /// for call sites that don't otherwise hold the task in memory. Call sites that
    /// already hold `&mut Task` push the line onto their own copy instead, so
    /// it rides along with their next save instead of racing a reload.
    fn append_decision(&self, task_id: &str, line: String) {
        if let Ok(Some(mut task)) = self.store.load_task(task_id) {
            task.decisions.push(line);
            task.updated_at = now_ms();
            let _ = self.store.save_task(&task);
            self.broadcast_task(&task);
        }
    }

    // -- protocol methods --------------------------------------------------

    async fn dispatch(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match method {
            "ping" => self.handle_ping().await,
            "settings.get" => self.handle_settings_get().await,
            "settings.set" => self.handle_settings_set(params).await,
            "settings.defaults" => self.handle_settings_defaults().await,
            "secrets.set" => self.handle_secrets_set(params).await,
            "task.list" => self.handle_task_list(params).await,
            "task.get" => self.handle_task_get(params).await,
            "task.create" => self.handle_task_create(params).await,
            "task.start" => self.handle_task_start(params).await,
            "task.land" => self.handle_task_land(params).await,
            "task.stop" => self.handle_task_stop(params).await,
            "task.answer" => self.handle_task_answer(params).await,
            "task.report" => self.handle_task_report(params).await,
            "task.leadTouch" => self.handle_task_lead_touch(params).await,
            "task.overturn" => self.handle_task_overturn(params).await,
            "task.amend" => self.handle_task_amend(params).await,
            "task.delete" => self.handle_task_delete(params).await,
            "costs.summary" => self.handle_costs_summary(params).await,
            "worktrees.gc" => self.handle_worktrees_gc(params).await,
            "task.archive" => self.handle_task_archive(params).await,
            "task.unarchive" => self.handle_task_unarchive(params).await,
            "task.timeline" => {
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                validate_task_id(&self.store, id)?;
                crate::timeline::timeline_json(&self.store, id)
            }
            "task.evidence" => self.handle_task_evidence(params).await,
            "failures.catalogue" => crate::timeline::catalogue_json(&self.store, &params),
            "chat.get" => chat::handle_get(self, params).await,
            "chat.send" => chat::handle_send(self, params).await,
            "chat.cancel" => chat::handle_cancel(self, params).await,
            "chat.list" => chat::handle_list(self, params).await,
            "chat.new" => chat::handle_new(self, params).await,
            "chat.switch" => chat::handle_switch(self, params).await,
            "chat.clear" => chat::handle_clear(self, params).await,
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
            "hook.stop" => self.handle_hook_stop(params).await,
            "hook.edit" => self.handle_hook_edit(params).await,
            "shutdown" => self.handle_shutdown().await,
            other => Err(format!("unknown method: {other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests live beside each concern in `tests/<file>.rs` and are included here so
    //! the qualified test names stay `engine::tests::*`.
    use super::*;

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
    include!("tests/verify.rs");
    include!("tests/worktrees.rs");
}
