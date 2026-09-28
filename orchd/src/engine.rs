//! The attempt loop, gates, review, commit and failure rules (spec
//! "Engine"), plus the `Dispatcher` implementation (`App`) that wires the
//! protocol methods to the store, git, harness and classifier. Pure
//! decision helpers live at the top with their own unit tests; `App` and
//! the async run loop are below.

use crate::brief;
use crate::classify;
use crate::git;
use crate::harness;
use crate::hook;
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

#[path = "audit.rs"]
mod audit;
#[path = "chat.rs"]
mod chat;
#[path = "messages.rs"]
mod messages;

// ===========================================================================
// Pure helpers (signature, tier-up/waiting, blocked-question, protected
// globs, review-route selection, task-id validation) -- no IO, unit tested
// at the bottom.
// ===========================================================================

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

const ERROR_KEYWORDS: &[&str] = &["error", "fail", "assert", "panic", "exception"];

/// The first line that looks like it's reporting a failure (contains one of
/// `ERROR_KEYWORDS`, case-insensitive); `None` when nothing in `text` does.
fn find_error_line(text: &str) -> Option<&str> {
    text.lines().find(|l| {
        let lower = l.to_ascii_lowercase();
        ERROR_KEYWORDS.iter().any(|k| lower.contains(k))
    })
}

/// Strips digits and absolute-path-looking tokens (so `/tmp/xyz123/a.ts:42`
/// across two runs still normalizes to the same signature) and collapses
/// whitespace.
fn normalize_signature_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' {
            while let Some(&next) = chars.peek() {
                if next.is_whitespace() {
                    break;
                }
                chars.next();
            }
            continue;
        }
        if c.is_ascii_digit() {
            continue;
        }
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// spec step 8 (as sharpened by review): "kind + normalized first
/// error-looking line of the tail (lines containing
/// error|fail|assert|panic|exception, case-insensitive; strip digits and
/// absolute paths), fallback to first tail line."
pub fn failure_signature(kind: FailureKind, detail: &str) -> String {
    let line = find_error_line(detail).unwrap_or_else(|| detail.lines().next().unwrap_or(""));
    let normalized = normalize_signature_line(line.trim());
    format!("{}:{}", kind.as_str(), truncate_chars(&normalized, 120))
}

/// Count trailing attempts (including the most recent) whose failure
/// signature matches, i.e. how many times in a row this exact failure has
/// happened.
pub fn consecutive_same_signature(attempts: &[Attempt], signature: &str) -> u32 {
    let mut count = 0;
    for a in attempts.iter().rev() {
        match &a.failure {
            Some(f) if f.signature == signature => count += 1,
            _ => break,
        }
    }
    count
}

pub struct FailureDecisionInput<'a> {
    pub tier: Tier,
    pub signature: &'a str,
    pub previous_signature: Option<&'a str>,
    pub consecutive_same: u32,
    pub attempt_n: u32,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FailureDecision {
    NextAttempt { tier: Tier },
    Waiting { question: String },
}

/// spec step 8: same signature as previous -> tier up; same signature 3x in
/// a row, or attempts exhausted -> waiting.
const EXHAUSTED_QUESTION: &str = "Attempts keep failing";
/// The decision line an orchestrator answer to that question leaves
/// (see `orchestrator_answer_line`).
const EXHAUSTED_ANSWER_PREFIX: &str = "Orchestrator: Attempts keep failing";

pub fn decide_after_failure(input: &FailureDecisionInput) -> FailureDecision {
    if input.consecutive_same >= 3 || input.attempt_n >= input.max_attempts {
        return FailureDecision::Waiting {
            question: format!(
                "{EXHAUSTED_QUESTION} with {}: continue, change approach, or stop?",
                input.signature
            ),
        };
    }
    let tier = if input.previous_signature == Some(input.signature) {
        input.tier.up()
    } else {
        input.tier
    };
    FailureDecision::NextAttempt { tier }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockedDecision {
    AnswerSelf,
    Waiting,
}

/// spec step 8, blocked branch: classifier p >= 0.7 -> agent answers itself.
pub fn decide_blocked_question(answerable_p: Option<f64>) -> BlockedDecision {
    match answerable_p {
        Some(p) if p >= 0.7 => BlockedDecision::AnswerSelf,
        _ => BlockedDecision::Waiting,
    }
}

/// Simple glob match: `*` matches any run of characters (including `/`,
/// deliberately simpler than shell globbing since protected paths are
/// meant to be broad, e.g. `src/app/**`), `?` matches one character,
/// anything else matches literally.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_chars(&p, &t)
}

fn glob_match_chars(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => {
            if glob_match_chars(&p[1..], t) {
                return true;
            }
            !t.is_empty() && glob_match_chars(p, &t[1..])
        }
        '?' => !t.is_empty() && glob_match_chars(&p[1..], &t[1..]),
        c => !t.is_empty() && t[0] == c && glob_match_chars(&p[1..], &t[1..]),
    }
}

pub fn matches_any_protected(path: &str, globs: &[String]) -> bool {
    globs.iter().any(|g| glob_match(g, path))
}

/// `review == "auto"` -> the hard tier's route, so a cheaper implementer is
/// checked by the strongest model; when the implementer already *is* that
/// route, the first route on a different harness instead. Explicit id ->
/// that route; `""` -> no review (handled by the caller before this).
pub fn select_review_route<'a>(
    settings: &'a Settings,
    implementer: &Route,
    other_family: bool,
) -> Option<&'a Route> {
    if settings.review != "auto" {
        return settings.routes.iter().find(|r| r.id == settings.review);
    }
    let hard = settings.tiers.get(&Tier::Hard);
    if other_family {
        let other = |r: &&Route| r.harness != implementer.harness;
        let found = settings
            .routes
            .iter()
            .filter(other)
            .find(|r| Some(&r.id) == hard)
            .or_else(|| settings.routes.iter().find(other));
        if found.is_some() {
            return found;
        }
    }
    settings
        .routes
        .iter()
        .find(|r| Some(&r.id) == hard && r.id != implementer.id)
        .or_else(|| {
            settings
                .routes
                .iter()
                .find(|r| r.harness != implementer.harness)
        })
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

// ===========================================================================
// Task graph: dependencies and parents (pure)
// ===========================================================================

/// Whether the wait-for graph has a cycle; `edges[a]` lists what `a` waits
/// for.
fn has_cycle(edges: &HashMap<String, Vec<String>>) -> bool {
    fn visit<'a>(
        node: &'a str,
        edges: &'a HashMap<String, Vec<String>>,
        state: &mut HashMap<&'a str, bool>,
    ) -> bool {
        match state.get(node) {
            Some(true) => return true,
            Some(false) => return false,
            None => {}
        }
        state.insert(node, true);
        for next in edges.get(node).into_iter().flatten() {
            if visit(next, edges, state) {
                return true;
            }
        }
        state.insert(node, false);
        false
    }
    // `true` = on the current path, `false` = finished without a cycle.
    let mut state: HashMap<&str, bool> = HashMap::new();
    edges.keys().any(|n| visit(n, edges, &mut state))
}

/// A task waits for its dependencies; a parent also waits for each child.
fn wait_edges(tasks: &[Task]) -> HashMap<String, Vec<String>> {
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for t in tasks {
        edges
            .entry(t.id.clone())
            .or_default()
            .extend(t.depends_on.iter().cloned());
        if let Some(parent) = &t.parent {
            edges.entry(parent.clone()).or_default().push(t.id.clone());
        }
    }
    edges
}

/// What `task` waits for among `all` (its repo's tasks): the dependencies
/// that still exist, and for a parent its children that are not archived.
fn waits_for<'a>(task: &Task, all: &'a [Task]) -> Vec<&'a Task> {
    all.iter()
        .filter(|t| {
            task.depends_on.contains(&t.id)
                || (t.parent.as_deref() == Some(task.id.as_str()) && !t.archived)
        })
        .collect()
}

fn is_parent(task: &Task, all: &[Task]) -> bool {
    all.iter()
        .any(|t| t.parent.as_deref() == Some(task.id.as_str()))
}

#[derive(Debug, PartialEq)]
enum Waits {
    /// Waits for nothing.
    Nothing,
    Ready,
    Pending,
    /// These ended `failed` or `stopped` and will not finish on their own.
    Ended(Vec<String>),
}

fn waits_state(task: &Task, all: &[Task]) -> Waits {
    state_of(waits_for(task, all))
}

/// Like `waits_state` but only for the task's own `dependsOn`, not for its
/// children: what a parent must wait for before its children may start.
fn own_waits_state(task: &Task, all: &[Task]) -> Waits {
    state_of(
        all.iter()
            .filter(|t| task.depends_on.contains(&t.id))
            .collect(),
    )
}

fn state_of(waits: Vec<&Task>) -> Waits {
    if waits.is_empty() {
        return Waits::Nothing;
    }
    let ended: Vec<String> = waits
        .iter()
        .filter(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Stopped))
        .map(|t| t.id.clone())
        .collect();
    if !ended.is_empty() {
        Waits::Ended(ended)
    } else if waits.iter().all(|t| t.status == TaskStatus::Done) {
        Waits::Ready
    } else {
        Waits::Pending
    }
}

/// Asked when something a task waits for ended `failed`/`stopped`; its
/// answer is handled by `answer_dependency_question`, not the attempt loop.
const DEPENDENCY_QUESTION: &str = "A task this one waits for ended";

fn status_word(status: TaskStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

const MAX_SUBTASKS: usize = 8;

/// Whether the planner's split can become a graph: at least two parts,
/// unique non-empty keys, a request each, dependencies on known keys only,
/// and no cycle.
fn check_subtasks(subtasks: &[brief::PlanSubtask]) -> Result<(), String> {
    if subtasks.len() < 2 {
        return Err("a single part is the task itself".to_string());
    }
    if subtasks.len() > MAX_SUBTASKS {
        return Err(format!("more than {MAX_SUBTASKS} parts"));
    }
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for s in subtasks {
        let key = s.key.trim();
        if key.is_empty() || s.request.trim().is_empty() {
            return Err("a part without a key or a request".to_string());
        }
        if edges.contains_key(key) {
            return Err(format!("duplicate key {key}"));
        }
        edges.insert(key.to_string(), s.depends_on.clone());
    }
    for (key, deps) in &edges {
        if let Some(unknown) = deps.iter().find(|d| !edges.contains_key(d.trim())) {
            return Err(format!("{key} depends on unknown key {unknown}"));
        }
    }
    if has_cycle(&edges) {
        return Err("the parts depend on each other in a cycle".to_string());
    }
    Ok(())
}

/// A subtask's request: the planner's text first (what its own planner
/// drafts from), then the whole it belongs to and what was already decided
/// for it.
fn subtask_request(parent: &Task, part: &str) -> String {
    let mut out = format!(
        "{}\n\nThis is one part of a larger task, \"{}\": {}",
        part.trim(),
        parent.title,
        parent.goal
    );
    let decided: Vec<&String> = parent
        .decisions
        .iter()
        .filter(|d| d.starts_with("Owner:") || d.starts_with("Orchestrator:"))
        .collect();
    if !decided.is_empty() {
        out.push_str("\n\nAlready decided for the whole task:\n");
        for d in decided {
            out.push_str(&format!("- {d}\n"));
        }
    }
    out
}

/// A parent's cost: its own runs plus every child's total.
fn parent_cost(parent: &Task, all: &[Task]) -> f64 {
    let own: f64 = parent
        .attempts
        .iter()
        .map(|a| a.cost_usd.unwrap_or(0.0) + a.review_cost_usd.unwrap_or(0.0))
        .sum();
    own + all
        .iter()
        .filter(|t| t.parent.as_deref() == Some(parent.id.as_str()))
        .map(|t| t.cost_usd)
        .sum::<f64>()
}

// ===========================================================================
// Cancellation
// ===========================================================================

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

// ===========================================================================
// App / Dispatcher
// ===========================================================================

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
    classifier_key: Option<String>,
    classifier_base_url: Option<String>,
    profiles: HashMap<String, ProfileSecret>,
}

struct TaskControl {
    cancel: CancelToken,
    /// One-shot per question: created fresh by `wait_for_answer` each time
    /// the loop actually waits, taken (and consumed) by `task.answer`. No
    /// long-lived queue -- an answer that arrives when nobody is waiting is
    /// simply not delivered (`task.answer` rejects it before it gets here).
    pending_answer: Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    handle: tokio::task::JoinHandle<()>,
}

struct HookContext {
    task_id: String,
    attempt_n: u32,
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
    /// Keyed by task id: the diff+untracked-list hash a verify run was last
    /// computed for, and its results -- shared by `hook.stop` and the
    /// post-session gate so an unchanged diff never re-runs verify twice.
    verify_cache: std::sync::Mutex<HashMap<String, (String, Vec<VerifyOutcome>)>>,
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
    self_ref: OnceLock<std::sync::Weak<App>>,
}

/// Everything `create_task_record` needs for a new task.
struct NewTask {
    id: String,
    repo_root: PathBuf,
    title: String,
    goal: String,
    criteria: Vec<String>,
    verify: Vec<String>,
    final_verify: Vec<String>,
    request: Option<String>,
    branch: Option<String>,
    base: String,
    variant: Variant,
    depends_on: Vec<String>,
    parent: Option<String>,
    eval_set: Option<String>,
    eval_name: Option<String>,
    created_at: i64,
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
            verify_cache: std::sync::Mutex::new(HashMap::new()),
            pid: std::process::id(),
            binary_mtime_ms,
            control_token,
            slots,
            parallel_limit,
            landing_locks: StdMutex::new(HashMap::new()),
            shutting_down: AtomicBool::new(false),
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
        let _ = self.shutdown_tx.send(());
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

    /// Same again for `repo.audit` runs: an entry goes only once its run
    /// has returned.
    pub fn any_audit_running(&self) -> bool {
        !self.audits.lock().unwrap().is_empty()
    }

    /// `ping`/`hook.stop` are the only methods reachable without the
    /// control token: `ping` so Electron can probe/tell daemons apart
    /// before it has read the token file, `hook.stop` because it's only
    /// ever invoked by `orchd hook stop` over the same trusted local
    /// machine, matched by its own per-run token instead.
    fn check_auth(&self, method: &str, auth: Option<&str>) -> bool {
        if method == "ping" || method == "hook.stop" {
            return true;
        }
        auth.map(|a| a == self.control_token).unwrap_or(false)
    }

    /// spec step 10: attempts left `running` from a previous process
    /// become `interrupted`; their tasks become `stopped`.
    /// Every task that was in flight when the daemon stopped gets its loop
    /// back: interrupted attempts are marked and requeued by the store, and
    /// queued or drafting tasks simply resume. A `waiting` task needs no loop
    /// until its answer arrives (`task.answer` relaunches one).
    pub fn recover_on_start(&self) -> std::io::Result<()> {
        let prices = self.settings.read().unwrap().prices.clone();
        let recovered = self.store.recover_interrupted(|task, idx| {
            let run_dir = self.store.run_dir(&task.id, task.attempts[idx].n);
            settle_unfinished_cost(task, idx, &run_dir, &prices);
        })?;
        for t in recovered {
            self.broadcast_task(&t);
        }
        for a in self.store.recover_interrupted_audits()? {
            audit::broadcast(self, &a);
        }
        for mut t in self.store.list_tasks()? {
            if t.archived {
                continue;
            }
            if matches!(
                t.status,
                TaskStatus::Queued | TaskStatus::Running | TaskStatus::Drafting
            ) {
                let id = t.id.clone();
                if settle_interrupted_advisor(&mut t, |n| self.store.run_dir(&id, n), &prices) {
                    t.updated_at = now_ms();
                    self.store.save_task(&t)?;
                    self.broadcast_task(&t);
                }
                self.start_task_loop(t.id);
            }
        }
        Ok(())
    }

    fn journal(
        &self,
        task_id: &str,
        point: &str,
        result: &Result<classify::Answers, classify::ClassifyError>,
        elapsed: Duration,
    ) {
        // A classifier the owner switched off is not a failed decision.
        if self.settings.read().unwrap().classifier.backend == ClassifierBackend::None {
            return;
        }
        let entry = classify::DecisionLogEntry {
            ts: now_ms(),
            point,
            task_id,
            answers: result.as_ref().ok(),
            error: result.as_ref().err().map(|e| e.0.as_str()),
            ms: elapsed.as_millis() as u64,
        };
        let line = classify::journal_line(&entry);
        let _ = self.store.append_decision_line(&line);
    }

    /// Appends a `"Jev: ..."` line to a task's `decisions` and persists it
    /// right away -- for classifier call sites that don't otherwise hold the
    /// task in memory (`handle_hook_stop`'s stop-gate check). Call sites that
    /// already hold `&mut Task` push the line onto their own copy instead, so
    /// it rides along with their next save instead of racing a reload.
    fn append_jev_decision(&self, task_id: &str, line: String) {
        if let Ok(Some(mut task)) = self.store.load_task(task_id) {
            task.decisions.push(line);
            task.updated_at = now_ms();
            let _ = self.store.save_task(&task);
            self.broadcast_task(&task);
        }
    }

    fn start_task_loop(&self, task_id: String) {
        self.spawn_task_loop(task_id, true);
    }

    /// `auto_start_after_plan` only matters for a task still `drafting`: once
    /// planning finishes, `true` lets the loop fall straight through into
    /// the implement stage, `false` leaves it `stopped` (fields filled in)
    /// for the owner to review before calling `task.start` themselves. Every
    /// caller except the `{repo, request}` form of `task.create` wants the
    /// former, since for them planning has either already happened or never
    /// applies.
    fn spawn_task_loop(&self, task_id: String, auto_start_after_plan: bool) {
        let mut controls = self.controls.lock().unwrap();
        if controls.contains_key(&task_id) {
            return;
        }
        let cancel = CancelToken::new();
        let pending_answer = Arc::new(StdMutex::new(None));
        let app = self.arc();
        let cancel_for_loop = cancel.clone();
        let pending_for_loop = pending_answer.clone();
        let tid = task_id.clone();
        let handle = tokio::spawn(async move {
            run_task_loop(
                app,
                tid,
                pending_for_loop,
                cancel_for_loop,
                auto_start_after_plan,
            )
            .await;
        });
        controls.insert(
            task_id,
            TaskControl {
                cancel,
                pending_answer,
                handle,
            },
        );
    }

    fn finish_task_loop(&self, task_id: &str) {
        self.controls.lock().unwrap().remove(task_id);
        if let Ok(Some(task)) = self.store.load_task(task_id) {
            self.advance_graph(&task.repo);
        }
    }

    fn landing_lock(&self, parent_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.landing_locks
            .lock()
            .unwrap()
            .entry(parent_id.to_string())
            .or_default()
            .clone()
    }

    fn repo_tasks(&self, repo: &str) -> Vec<Task> {
        let mut tasks = self.store.list_tasks().unwrap_or_default();
        tasks.retain(|t| t.repo == repo);
        tasks
    }

    /// Moves a repo's task graph along after something in it changed: a
    /// queued task (or a running parent) with no live loop starts once all
    /// it waits for is done, and waits for the owner when one of those ended
    /// `failed`/`stopped`. Tasks outside any graph are never touched.
    fn advance_graph(&self, repo: &str) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let all = self.repo_tasks(repo);
        for task in all.iter().filter(|t| !t.archived) {
            let parent = is_parent(task, &all);
            let asking = task.status == TaskStatus::Waiting
                && task
                    .question
                    .as_ref()
                    .is_some_and(|q| q.text.starts_with(DEPENDENCY_QUESTION));
            let idle = task.status == TaskStatus::Queued
                || (parent && task.status == TaskStatus::Running)
                || asking;
            if !idle || self.controls.lock().unwrap().contains_key(&task.id) {
                continue;
            }
            // A subtask never starts while its parent still waits for the
            // parent's own dependencies.
            if let Some(p) = task
                .parent
                .as_deref()
                .and_then(|pid| all.iter().find(|t| t.id == pid))
            {
                if !matches!(own_waits_state(p, &all), Waits::Ready | Waits::Nothing) {
                    continue;
                }
            }
            let in_graph = parent || task.parent.is_some() || !task.depends_on.is_empty();
            let state = waits_state(task, &all);
            // A parent whose children were only just drafted has to start
            // them itself once its own dependencies are done.
            let unstarted_children = parent
                && matches!(own_waits_state(task, &all), Waits::Ready | Waits::Nothing)
                && all.iter().any(|t| {
                    t.parent.as_deref() == Some(task.id.as_str())
                        && !t.archived
                        && t.status == TaskStatus::Drafting
                });
            match state {
                // Also picks up a task restarted while its previous loop was
                // still winding down (that spawn was a no-op), and a
                // dependent whose question is moot because everything it
                // waited for finished meanwhile.
                Waits::Ready | Waits::Nothing if in_graph => {
                    if asking {
                        let mut task = task.clone();
                        task.question = None;
                        task.status = TaskStatus::Queued;
                        task.updated_at = now_ms();
                        let _ = self.store.save_task(&task);
                        self.broadcast_task(&task);
                    }
                    self.spawn_task_loop(task.id.clone(), true)
                }
                Waits::Pending if unstarted_children => self.spawn_task_loop(task.id.clone(), true),
                Waits::Ended(ended) => {
                    if !asking {
                        self.ask_dependency_question(task, &all, &ended);
                    }
                }
                _ => {}
            }
        }
    }

    /// Parks `task` `waiting` on the question of what to do about the
    /// dependencies in `ended`.
    fn ask_dependency_question(&self, task: &Task, all: &[Task], ended: &[String]) {
        let names: Vec<String> = all
            .iter()
            .filter(|t| ended.contains(&t.id))
            .map(|t| format!("\"{}\" ({})", t.title, status_word(t.status)))
            .collect();
        let mut task = task.clone();
        task.question = Some(Question {
            text: format!(
                "{DEPENDENCY_QUESTION}: {}. Retry it, drop it from what this task waits for, or stop?",
                names.join(", ")
            ),
            options: vec![
                "retry the dependency".into(),
                "drop the dependency".into(),
                "stop".into(),
            ],
        });
        task.status = TaskStatus::Waiting;
        task.updated_at = now_ms();
        let _ = self.store.save_task(&task);
        self.broadcast_task(&task);
    }

    /// Stops every child of `parent_id` that has not finished: a live loop
    /// is cancelled (it marks itself stopped), an idle one is marked here.
    fn stop_children(&self, parent_id: &str, repo: &str) {
        for mut child in self.repo_tasks(repo) {
            if child.parent.as_deref() != Some(parent_id)
                || matches!(
                    child.status,
                    TaskStatus::Done | TaskStatus::Failed | TaskStatus::Stopped
                )
            {
                continue;
            }
            if let Some(ctrl) = self.controls.lock().unwrap().get(&child.id) {
                ctrl.cancel.cancel();
                continue;
            }
            child.question = None;
            child.status = TaskStatus::Stopped;
            child.updated_at = now_ms();
            let _ = self.store.save_task(&child);
            self.broadcast_task(&child);
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
            "task.stop" => self.handle_task_stop(params).await,
            "task.answer" => self.handle_task_answer(params).await,
            "task.delete" => self.handle_task_delete(params).await,
            "task.archive" => self.handle_task_archive(params).await,
            "task.unarchive" => self.handle_task_unarchive(params).await,
            "task.preflight" => self.handle_task_preflight(params).await,
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
            "hook.stop" => self.handle_hook_stop(params).await,
            "shutdown" => self.handle_shutdown().await,
            other => Err(format!("unknown method: {other}")),
        }
    }

    async fn handle_ping(&self) -> Result<serde_json::Value, String> {
        // Only slots actually held by a running attempt count -- a task
        // still queued behind the concurrency limit is not "running".
        // An orchestrator chat reply in progress counts too: replacing the
        // daemon under it would lose the reply.
        let running = self.parallel_limit as usize - self.slots.available_permits()
            + self.chat_turns.lock().unwrap().len();
        Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "pid": self.pid,
            "dataDir": self.data_dir.to_string_lossy(),
            "binaryMtimeMs": self.binary_mtime_ms,
            "running": running,
        }))
    }

    async fn handle_settings_get(&self) -> Result<serde_json::Value, String> {
        let s = self.settings.read().unwrap().clone();
        serde_json::to_value(&s).map_err(|e| e.to_string())
    }

    /// The built-in defaults, never the saved settings: the panel compares
    /// against these to show what an owner's older save has frozen.
    async fn handle_settings_defaults(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(Settings::default()).map_err(|e| e.to_string())
    }

    async fn handle_settings_set(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            settings: Settings,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        p.settings.experiments.check()?;
        p.settings.experiments.check_routes(&p.settings.routes)?;
        self.store
            .save_settings(&p.settings)
            .map_err(|e| e.to_string())?;
        *self.settings.write().unwrap() = p.settings.clone();
        serde_json::to_value(&p.settings).map_err(|e| e.to_string())
    }

    /// A *full* replace (spec): every call overwrites the whole in-memory
    /// secrets state, it never merges into what's already there.
    async fn handle_secrets_set(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct ClassifierSecretIn {
            #[serde(default)]
            key: Option<String>,
            #[serde(default)]
            base_url: Option<String>,
        }
        #[derive(Deserialize, Default)]
        struct P {
            #[serde(default)]
            classifier: Option<ClassifierSecretIn>,
            #[serde(default)]
            profiles: HashMap<String, ProfileSecret>,
        }
        let p: P = serde_json::from_value(params).unwrap_or_default();
        let mut secrets = self.secrets.write().unwrap();
        *secrets = Secrets {
            classifier_key: p.classifier.as_ref().and_then(|c| c.key.clone()),
            classifier_base_url: p.classifier.as_ref().and_then(|c| c.base_url.clone()),
            profiles: p.profiles,
        };
        Ok(json!({}))
    }

    async fn handle_task_list(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct P {
            repo: Option<String>,
            #[serde(default)]
            include_archived: bool,
        }
        let p: P = serde_json::from_value(params).unwrap_or_default();
        let mut tasks = self.store.list_tasks().map_err(|e| e.to_string())?;
        if let Some(repo) = p.repo {
            tasks.retain(|t| t.repo == repo);
        }
        if !p.include_archived {
            tasks.retain(|t| !t.archived);
        }
        serde_json::to_value(&tasks).map_err(|e| e.to_string())
    }

    async fn handle_task_get(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    async fn handle_task_create(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            repo: String,
            #[serde(default)]
            title: Option<String>,
            #[serde(default)]
            goal: Option<String>,
            #[serde(default)]
            criteria: Vec<String>,
            #[serde(default)]
            verify: Vec<String>,
            /// Slow checks run once, after review passes.
            #[serde(default, rename = "finalVerify")]
            final_verify: Vec<String>,
            #[serde(default)]
            branch: Option<String>,
            /// Commit-ish the task branches from; the repo's HEAD if unset.
            #[serde(default)]
            base: Option<String>,
            #[serde(default)]
            mcp: Option<serde_json::Value>,
            #[serde(default)]
            start: Option<bool>,
            /// The `{repo, request}` form: a one-sentence ask instead of a
            /// filled-in title/goal/criteria/verify, drafted by the plan
            /// stage before anything is queued (spec "drafting stage").
            #[serde(default)]
            request: Option<String>,
            /// Experiment flags laid over `settings.experiments` for this
            /// task only (an A/B arm).
            #[serde(default)]
            variant: Option<serde_json::Value>,
            /// Set by `orchd eval run`: the eval set and the task's name in it.
            #[serde(default, rename = "evalSet")]
            eval_set: Option<String>,
            #[serde(default, rename = "evalName")]
            eval_name: Option<String>,
            /// Ids of tasks that must be done before this one implements.
            #[serde(default, rename = "dependsOn")]
            depends_on: Vec<String>,
            /// The task this one is a part of; it branches from and lands
            /// on that task's branch.
            #[serde(default)]
            parent: Option<String>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let settings = self.settings.read().unwrap().clone();
        let variant = resolve_variant(&settings.experiments, p.variant.as_ref())?;
        variant.check_routes(&settings.routes)?;
        let request_text = p.request.clone().filter(|s| !s.trim().is_empty());
        if request_text.is_some() {
            let planner = variant.plan_route_id(&settings);
            if planner.is_empty() {
                return Err(
                    "planner is disabled; task.create needs title/goal instead of request"
                        .to_string(),
                );
            }
            // Same "off, not a silent fallback" rule `run_plan_stage` applies
            // at drafting time -- reject up front instead of creating a
            // worktree for a task that can only ever fail to plan.
            let known = settings.routes.iter().any(|r| r.id == planner);
            if !known {
                return Err(format!("planner route \"{planner}\" is not configured"));
            }
        }
        let title = match (&request_text, &p.title) {
            (Some(r), _) => truncate_chars(r, 60),
            (None, Some(t)) => t.clone(),
            (None, None) => {
                return Err("task.create requires either request or title/goal".to_string())
            }
        };
        let goal = if request_text.is_some() {
            String::new()
        } else {
            p.goal
                .clone()
                .ok_or_else(|| "task.create requires goal".to_string())?
        };

        let repo_input = PathBuf::from(&p.repo);
        let repo_root = tokio::task::spawn_blocking(move || git::repo_toplevel(&repo_input))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let repo = repo_root.to_string_lossy().to_string();
        // Checked before anything is created: a rejected graph leaves no
        // worktree, branch or record behind.
        let (depends_on, parent) =
            self.check_graph_params(&repo, &p.depends_on, p.parent.as_deref())?;
        let base = p.base.clone().filter(|b| !b.trim().is_empty());
        let base = match (&parent, base) {
            (Some(parent), Some(base)) if base != parent.branch => {
                return Err(format!(
                    "a subtask starts from its parent's branch {}, not {base}",
                    parent.branch
                ))
            }
            (Some(parent), _) => parent.branch.clone(),
            (None, base) => base.unwrap_or_else(|| "HEAD".to_string()),
        };
        let in_graph = parent.is_some() || !depends_on.is_empty();
        let task = self
            .create_task_record(NewTask {
                id: uuid::Uuid::new_v4().to_string(),
                repo_root,
                title,
                goal,
                criteria: p.criteria,
                verify: p.verify,
                final_verify: p.final_verify,
                request: request_text.clone(),
                branch: p.branch.clone(),
                base,
                variant,
                depends_on,
                parent: parent.map(|t| t.id),
                eval_set: p.eval_set.clone().filter(|s| !s.trim().is_empty()),
                eval_name: p.eval_name.clone().filter(|s| !s.trim().is_empty()),
                created_at: now_ms(),
            })
            .await?;
        let id = task.id.clone();
        if let Some(mcp) = &p.mcp {
            let path = self.store.task_dir(&id).join("mcp.json");
            store::write_json_atomic(&path, mcp).map_err(|e| e.to_string())?;
        }
        self.broadcast_task(&task);
        if request_text.is_some() {
            // Planning always runs regardless of `start`; only whether the
            // loop falls through into implementing once it's done depends
            // on it (spec step 6).
            self.spawn_task_loop(id.clone(), p.start.unwrap_or(true));
        } else if p.start.unwrap_or(true) || in_graph {
            // Same default as the request form: a `queued` task that never
            // starts until a daemon restart looks like work in progress. A
            // task in a graph starts on its own: its loop waits for what it
            // depends on.
            self.start_task_loop(id.clone());
        }
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    /// `task.create`'s `dependsOn`/`parent`: every id a task of this repo,
    /// the parent a top-level task that has not implemented anything itself
    /// and is not done, and no dependency cycle once the new task is added
    /// (a parent waits for its children, so a child depending on its own
    /// parent is one). Returns the deduplicated dependencies and the parent.
    fn check_graph_params(
        &self,
        repo: &str,
        depends_on: &[String],
        parent: Option<&str>,
    ) -> Result<(Vec<String>, Option<Task>), String> {
        let load = |id: &str, what: &str| -> Result<Task, String> {
            validate_task_id(&self.store, id).map_err(|_| format!("unknown {what} {id}"))?;
            let task = self
                .store
                .load_task(id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("unknown {what} {id}"))?;
            if task.repo != repo {
                return Err(format!("{what} {id} belongs to another repository"));
            }
            Ok(task)
        };
        let mut deps: Vec<String> = Vec::new();
        for id in depends_on {
            load(id, "dependency")?;
            if !deps.contains(id) {
                deps.push(id.clone());
            }
        }
        let parent = match parent.filter(|p| !p.trim().is_empty()) {
            None => None,
            Some(id) => {
                let parent = load(id, "parent")?;
                if parent.parent.is_some() {
                    return Err("a subtask cannot have subtasks of its own".to_string());
                }
                if parent.status == TaskStatus::Done {
                    return Err("the parent task is already done".to_string());
                }
                if implement_attempt_count(&parent) > 0 {
                    return Err(
                        "the parent task has already run an implement attempt of its own"
                            .to_string(),
                    );
                }
                // A live loop may already be queued for a slot to implement
                // the whole request; only a drafting one re-checks after
                // planning.
                if parent.status != TaskStatus::Drafting
                    && self.controls.lock().unwrap().contains_key(&parent.id)
                {
                    return Err(
                        "the parent task is already running; create it with start: false, add its subtasks, then start it"
                            .to_string(),
                    );
                }
                Some(parent)
            }
        };
        let new_id = "(new task)".to_string();
        let mut edges = wait_edges(&self.repo_tasks(repo));
        edges.insert(new_id.clone(), deps.clone());
        if let Some(parent) = &parent {
            edges.entry(parent.id.clone()).or_default().push(new_id);
        }
        if has_cycle(&edges) {
            return Err("dependsOn would make a dependency cycle".to_string());
        }
        Ok((deps, parent))
    }

    /// Creates the task's branch and worktree and saves its record, status
    /// `drafting` for a request, `queued` otherwise. Shared by `task.create`
    /// and the planner's split into subtasks.
    async fn create_task_record(&self, new: NewTask) -> Result<Task, String> {
        let repo_root = new.repo_root.clone();
        let title_for_branch = new.title.clone();
        let branch_opt = new.branch.clone();
        let base = new.base.clone();
        let created = tokio::task::spawn_blocking(move || {
            let branch = branch_opt
                .unwrap_or_else(|| git::unique_branch_name(&repo_root, &title_for_branch));
            let wt_path = git::worktree_path(&repo_root, &branch);
            let base_ref = git::branch_of(&repo_root, &base);
            let created = git::create_worktree(&repo_root, &branch, &wt_path, &base)?;
            if let Err(e) = git::bootstrap_worktree(&repo_root, &created.path) {
                git::discard_worktree(&repo_root, &created.path, &branch);
                return Err(git::GitError(e.to_string()));
            }
            Ok::<_, git::GitError>((branch, created, base_ref))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        let (branch, created, base_ref) = created;

        let task = Task {
            id: new.id,
            title: new.title,
            goal: new.goal,
            criteria: new.criteria,
            verify: new.verify,
            final_verify: new.final_verify,
            status: if new.request.is_some() {
                TaskStatus::Drafting
            } else {
                TaskStatus::Queued
            },
            request: new.request,
            repo: new.repo_root.to_string_lossy().to_string(),
            worktree: created.path.to_string_lossy().to_string(),
            branch,
            base_sha: created.base_sha,
            base_ref,
            depends_on: new.depends_on,
            parent: new.parent,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            budget_raises: 0,
            archived: false,
            planned_tier: None,
            tier_fallback: None,
            variant: Some(new.variant),
            eval_set: new.eval_set,
            eval_name: new.eval_name,
            created_at: new.created_at,
            updated_at: new.created_at,
        };
        if let Err(e) = self.store.save_task(&task) {
            let _ = std::fs::remove_dir_all(self.store.task_dir(&task.id));
            let (repo, wt, branch) = (
                task.repo.clone(),
                task.worktree.clone(),
                task.branch.clone(),
            );
            let _ = tokio::task::spawn_blocking(move || {
                git::discard_worktree(Path::new(&repo), Path::new(&wt), &branch)
            })
            .await;
            return Err(e.to_string());
        }
        Ok(task)
    }

    async fn handle_task_start(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let already_running = self.controls.lock().unwrap().contains_key(&p.id);
        if !already_running {
            let task = self
                .store
                .load_task(&p.id)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "task not found".to_string())?;
            if task.archived {
                return Err("task is archived; unarchive it first".to_string());
            }
            if matches!(
                task.status,
                TaskStatus::Queued
                    | TaskStatus::Stopped
                    | TaskStatus::Failed
                    | TaskStatus::Drafting
            ) {
                // A manual `task.start` on a still-drafting task (e.g. one
                // left there by a daemon that died mid-plan) means proceed
                // straight to implementing once planning finishes.
                self.spawn_task_loop(p.id.clone(), true);
            }
        }
        let latest = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        serde_json::to_value(&latest).map_err(|e| e.to_string())
    }

    /// Works in every state, including a task still queued behind the
    /// concurrency limit or parked waiting for an answer -- both select
    /// against the same `CancelToken`.
    async fn handle_task_stop(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let live = match self.controls.lock().unwrap().get(&p.id) {
            Some(ctrl) => {
                ctrl.cancel.cancel();
                true
            }
            None => false,
        };
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        // A task in a graph can sit idle with no loop to cancel: a parent
        // while its children work, a task waiting for its dependencies.
        let all = self.repo_tasks(&task.repo);
        let parent = is_parent(&task, &all);
        if parent {
            self.stop_children(&task.id, &task.repo);
        }
        if !live
            && (parent || !task.depends_on.is_empty())
            && matches!(
                task.status,
                TaskStatus::Queued | TaskStatus::Running | TaskStatus::Waiting
            )
        {
            task.question = None;
            task.status = TaskStatus::Stopped;
            task.updated_at = now_ms();
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
            self.advance_graph(&task.repo);
        }
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    async fn handle_task_answer(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            answer: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.status != TaskStatus::Waiting || task.question.is_none() {
            return Err("task is not waiting for an answer".to_string());
        }

        if p.answer.trim().eq_ignore_ascii_case("stop") {
            if let Some(ctrl) = self.controls.lock().unwrap().get(&p.id) {
                ctrl.cancel.cancel();
            }
            task.decisions.push("Owner: stop".to_string());
            task.question = None;
            task.status = TaskStatus::Stopped;
            task.updated_at = now_ms();
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
            self.stop_children(&task.id, &task.repo);
            self.advance_graph(&task.repo);
            return serde_json::to_value(&task).map_err(|e| e.to_string());
        }

        if task
            .question
            .as_ref()
            .is_some_and(|q| q.text.starts_with(DEPENDENCY_QUESTION))
        {
            return self.answer_dependency_question(task, &p.answer);
        }

        let delivered = {
            let controls = self.controls.lock().unwrap();
            controls
                .get(&p.id)
                .and_then(|c| c.pending_answer.lock().unwrap().take())
                .map(|tx| tx.send(p.answer.clone()).is_ok())
                .unwrap_or(false)
        };
        if !delivered {
            // No live parked loop (e.g. after a daemon restart): apply the
            // decision synchronously and relaunch (spec step 9: "status
            // queued, loop continues").
            task.decisions.push(format!("Owner: {}", p.answer));
            if task
                .question
                .as_ref()
                .is_some_and(|q| is_budget_raise(q, &p.answer))
            {
                task.budget_raises += 1;
            }
            task.question = None;
            task.status = TaskStatus::Queued;
            task.updated_at = now_ms();
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
            self.start_task_loop(p.id.clone());
        }
        let latest = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        serde_json::to_value(&latest).map_err(|e| e.to_string())
    }

    /// "retry" restarts every task this one waits for that ended
    /// `failed`/`stopped`; "drop" stops waiting for them (a dependency leaves
    /// `dependsOn`, a child is detached from this parent). Either way this
    /// task goes back to waiting for the rest.
    fn answer_dependency_question(
        &self,
        mut task: Task,
        answer: &str,
    ) -> Result<serde_json::Value, String> {
        let all = self.repo_tasks(&task.repo);
        let was_parent = is_parent(&task, &all);
        let ended: Vec<Task> = waits_for(&task, &all)
            .into_iter()
            .filter(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Stopped))
            .cloned()
            .collect();
        let choice = answer.trim().to_ascii_lowercase();
        if choice.starts_with("retry") {
            for dep in ended.iter().filter(|t| !t.archived) {
                if self.controls.lock().unwrap().contains_key(&dep.id) {
                    continue;
                }
                // Queued before its loop runs, so nothing reads it as
                // ended again while it retries.
                let mut dep = dep.clone();
                dep.question = None;
                dep.status = TaskStatus::Queued;
                dep.updated_at = now_ms();
                self.store.save_task(&dep).map_err(|e| e.to_string())?;
                self.broadcast_task(&dep);
                self.spawn_task_loop(dep.id.clone(), true);
            }
        } else if choice.starts_with("drop") {
            task.depends_on
                .retain(|id| !ended.iter().any(|t| &t.id == id));
            for mut child in ended
                .into_iter()
                .filter(|t| t.parent.as_deref() == Some(task.id.as_str()))
            {
                child.parent = None;
                child
                    .decisions
                    .push(format!("Owner: dropped from \"{}\"", task.title));
                child.updated_at = now_ms();
                self.store.save_task(&child).map_err(|e| e.to_string())?;
                self.broadcast_task(&child);
            }
        } else {
            return Err("answer retry the dependency, drop the dependency, or stop".to_string());
        }
        task.decisions.push(format!("Owner: {}", answer.trim()));
        task.question = None;
        let fresh = self.repo_tasks(&task.repo);
        if was_parent && !is_parent(&task, &fresh) {
            // Left without children it would implement the whole request.
            task.decisions.push(
                "Orchestrator: no subtasks are left, so this task was stopped instead of implementing the whole request itself"
                    .to_string(),
            );
            task.status = TaskStatus::Stopped;
            task.updated_at = now_ms();
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
            return serde_json::to_value(&task).map_err(|e| e.to_string());
        }
        task.status = if is_parent(&task, &fresh) {
            TaskStatus::Running
        } else {
            TaskStatus::Queued
        };
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        // Its loop waits for whatever is still pending, or goes ahead.
        self.spawn_task_loop(task.id.clone(), true);
        let latest = self
            .store
            .load_task(&task.id)
            .map_err(|e| e.to_string())?
            .unwrap_or(task);
        serde_json::to_value(&latest).map_err(|e| e.to_string())
    }

    /// Cancels the loop and waits for it to actually exit before touching
    /// the filesystem, so a still-running attempt can never write into a
    /// directory that's mid-deletion.
    async fn handle_task_delete(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let removed = self.controls.lock().unwrap().remove(&p.id);
        if let Some(ctrl) = removed {
            ctrl.cancel.cancel();
            let _ = ctrl.handle.await;
        }
        let repo = self.store.load_task(&p.id).ok().flatten().map(|t| t.repo);
        let dir = self.store.task_dir(&p.id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        }
        // A deleted dependency no longer holds anything back.
        if let Some(repo) = repo {
            self.advance_graph(&repo);
        }
        Ok(json!({}))
    }

    /// Hides a task from the default `task.list` without touching its
    /// worktree, branch, or record -- refused while a loop could still be
    /// mutating it (running, drafting a plan, or parked waiting for an
    /// answer), same as `task.delete` would need to stop it first, except
    /// this is never destructive so there's nothing to reconcile after. Also
    /// refused while a loop is merely queued behind the concurrency limit
    /// (present in `controls` even though `status` still reads `Queued`) --
    /// that loop is still going to run and mutate the task the moment a
    /// slot frees up, same hazard as an already-running one.
    async fn handle_task_archive(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if matches!(
            task.status,
            TaskStatus::Running | TaskStatus::Drafting | TaskStatus::Waiting
        ) {
            return Err("cannot archive a running, drafting, or waiting task".to_string());
        }
        if self.controls.lock().unwrap().contains_key(&p.id) {
            return Err("cannot archive a task with a live loop".to_string());
        }
        task.archived = true;
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        // An archived child no longer holds its parent back.
        self.advance_graph(&task.repo);
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    async fn handle_task_unarchive(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        task.archived = false;
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    async fn handle_task_preflight(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize, Default)]
        struct P {
            #[serde(default)]
            goal: String,
            #[serde(default)]
            criteria: Vec<String>,
            #[serde(default)]
            verify: Vec<String>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;

        let classifier_settings = self.settings.read().unwrap().classifier.clone();
        let key = self.secrets.read().unwrap().classifier_key.clone();
        let base_url = self.secrets.read().unwrap().classifier_base_url.clone();
        let classifier_available =
            classifier_settings.backend != ClassifierBackend::None && key.is_some();

        if classifier_available {
            let state = json!({"goal": p.goal, "criteria": p.criteria, "verify": p.verify});
            let questions = vec![
                classify::QuestionSpec::Noul {
                    name: "goal_specific".to_string(),
                    prompt: "Is the goal specific enough to act on without asking?".to_string(),
                },
                classify::QuestionSpec::Noul {
                    name: "criteria_checkable".to_string(),
                    prompt: "Can each acceptance criterion be checked objectively from outside?"
                        .to_string(),
                },
                classify::QuestionSpec::Noul {
                    name: "has_verification".to_string(),
                    prompt: "Do the verification commands actually exercise the criteria?"
                        .to_string(),
                },
            ];
            let start = std::time::Instant::now();
            let s2 = classifier_settings.clone();
            let k2 = key.clone();
            let b2 = base_url.clone();
            let q2 = questions.clone();
            let state2 = state.clone();
            let result = tokio::task::spawn_blocking(move || {
                classify::decide(&s2, k2.as_deref(), b2.as_deref(), &state2, &q2)
            })
            .await
            .unwrap_or_else(|e| Err(classify::ClassifyError(e.to_string())));
            // No task exists yet at preflight time -- journaled with an
            // empty task id, the only place that happens.
            self.journal("", "preflight", &result, start.elapsed());
            if let Ok(answers) = result {
                let labels: [(&str, &str); 3] = [
                    ("goal_specific", "Goal is specific enough to act on"),
                    (
                        "criteria_checkable",
                        "Each criterion is objectively checkable",
                    ),
                    (
                        "has_verification",
                        "Verification commands exercise the criteria",
                    ),
                ];
                let checks: Vec<serde_json::Value> = labels
                    .iter()
                    .map(|(id, label)| {
                        let p = answers.get(*id).and_then(|a| a.noul).unwrap_or(0.0);
                        json!({"id": id, "label": label, "ok": p >= 0.5, "p": p})
                    })
                    .collect();
                return Ok(json!({"available": true, "checks": checks}));
            }
            // Classifier configured but the call itself failed (timeout,
            // network, bad key): fall back to the deterministic checks
            // below rather than blocking progress.
        }

        let mut checks = Vec::new();
        for (i, cmd) in p.verify.iter().enumerate() {
            let program = cmd.split_whitespace().next().unwrap_or("");
            let ok = !program.is_empty() && which_on_path(program);
            checks.push(json!({"id": format!("verify-{i}"), "label": cmd, "ok": ok, "p": if ok {1.0} else {0.0}}));
        }
        Ok(json!({"available": false, "checks": checks}))
    }

    async fn handle_shutdown(&self) -> Result<serde_json::Value, String> {
        self.shutdown();
        Ok(json!({}))
    }

    async fn handle_hook_stop(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            token: String,
            #[serde(default)]
            payload: serde_json::Value,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let ctx = { self.hook_tokens.read().unwrap().get(&p.token).cloned() };
        let Some(ctx) = ctx else {
            return Ok(json!({}));
        };

        ctx.hook_running.store(true, Ordering::SeqCst);
        let _hook_done = ClearOnDrop(&ctx.hook_running);
        let worktree = ctx.worktree.clone();
        let base_sha = ctx.base_sha.clone();
        let wt = worktree.clone();
        let base = base_sha.clone();
        let changed =
            tokio::task::spawn_blocking(move || git::changed_files(&wt, &base).unwrap_or_default())
                .await
                .unwrap_or_default();
        let has_changed = !changed.is_empty();
        let verify_configured = !ctx.verify.is_empty();

        let verify_results = if verify_configured && has_changed {
            let app = self.arc();
            let run_dir = self.store.run_dir(&ctx.task_id, ctx.attempt_n);

            // Budget well under Claude's own 600s hook timeout; on timeout,
            // fail open rather than block the agent forever.
            match tokio::time::timeout(
                Duration::from_secs(540),
                run_verify_cached(
                    &app,
                    &ctx.task_id,
                    &worktree,
                    &run_dir,
                    &base_sha,
                    &ctx.verify,
                    &ctx.cancel,
                ),
            )
            .await
            {
                Ok(results) => results,
                Err(_) => return Ok(json!({})),
            }
        } else {
            vec![]
        };

        let classifier_answers = if !verify_configured && has_changed {
            let last_msg = extract_last_assistant_message(&p.payload).unwrap_or_default();
            let wt2 = worktree.clone();
            let base2 = base_sha.clone();
            let diff_stat = tokio::task::spawn_blocking(move || {
                git::diff_stat(&wt2, &base2).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            let state = json!({"last_assistant_message": last_msg, "diff_stat": diff_stat});
            let questions = jev_belay_questions();
            let settings = self.settings.read().unwrap().classifier.clone();
            let key = self.secrets.read().unwrap().classifier_key.clone();
            let base_url = self.secrets.read().unwrap().classifier_base_url.clone();
            let start = std::time::Instant::now();
            let result = tokio::task::spawn_blocking(move || {
                classify::decide(
                    &settings,
                    key.as_deref(),
                    base_url.as_deref(),
                    &state,
                    &questions,
                )
            })
            .await
            .unwrap_or_else(|e| Err(classify::ClassifyError(e.to_string())));
            self.journal(&ctx.task_id, "stop_gate", &result, start.elapsed());
            result.ok().as_ref().and_then(to_jev_belay)
        } else {
            None
        };

        let facts = hook::StopFacts {
            blocks_so_far: ctx.blocks.load(Ordering::SeqCst),
            has_changed_files: has_changed,
            verify_configured,
            verify_results: &verify_results,
        };
        let decision = hook::decide_stop(&facts, classifier_answers.as_ref());
        if let Some(a) = classifier_answers.as_ref() {
            let blocked = matches!(decision, hook::StopDecision::Block { .. });
            let line = jev_stop_gate_line(blocked, a.claims_done, a.claims_verified);
            self.append_jev_decision(&ctx.task_id, line);
        }
        match decision {
            hook::StopDecision::Allow => Ok(json!({})),
            hook::StopDecision::Block { reason } => {
                ctx.blocks.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"decision": "block", "reason": reason}))
            }
        }
    }
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

/// `overrides` is a partial `Variant` object; its keys replace the defaults'.
fn resolve_variant(
    defaults: &Variant,
    overrides: Option<&serde_json::Value>,
) -> Result<Variant, String> {
    let Some(overrides) = overrides else {
        return Ok(defaults.clone());
    };
    let Some(fields) = overrides.as_object() else {
        return Err("variant must be an object".to_string());
    };
    let mut merged = serde_json::to_value(defaults).map_err(|e| e.to_string())?;
    for (k, v) in fields {
        // Checked here, not with deny_unknown_fields: a task.json keeps
        // loading after a flag it names is retired.
        if merged.get(k).is_none() && !Variant::OPTIONAL_KEYS.contains(&k.as_str()) {
            return Err(format!("unknown variant flag: {k}"));
        }
        merged[k] = v.clone();
    }
    let variant: Variant =
        serde_json::from_value(merged).map_err(|e| format!("invalid variant: {e}"))?;
    variant.check()?;
    Ok(variant)
}

const MAX_REVIEW_SCREENSHOTS: usize = 8;

/// Images under the worktree's `artifacts/` (gitignored, so never in the
/// diff) written since `since_ms`, newest first.
fn attempt_screenshots(worktree: &Path, since_ms: i64) -> Vec<PathBuf> {
    let mut found: Vec<(i64, PathBuf)> = Vec::new();
    let mut dirs = vec![worktree.join("artifacts")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                dirs.push(path);
                continue;
            }
            let is_image = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "webp"
                )
            });
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            if is_image && modified >= since_ms {
                found.push((modified, path));
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found
        .into_iter()
        .take(MAX_REVIEW_SCREENSHOTS)
        .map(|(_, p)| p)
        .collect()
}

fn which_on_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':')
        .any(|dir| Path::new(dir).join(program).is_file())
}

fn jev_belay_questions() -> Vec<classify::QuestionSpec> {
    vec![
        classify::QuestionSpec::Noul {
            name: "claims_done".to_string(),
            prompt: "Does the assistant's last message claim the work is done?".to_string(),
        },
        classify::QuestionSpec::Noul {
            name: "claims_verified".to_string(),
            prompt:
                "Does the message show the work was actually verified (tests run, checks passed)?"
                    .to_string(),
        },
        classify::QuestionSpec::Noul {
            name: "verification_applies".to_string(),
            prompt: "Does a verification step meaningfully apply to this task?".to_string(),
        },
        classify::QuestionSpec::Choice {
            name: "outcome".to_string(),
            prompt: "What outcome does the message report?".to_string(),
            options: vec!["complete", "partial", "blocked", "other"]
                .into_iter()
                .map(String::from)
                .collect(),
        },
    ]
}

fn to_jev_belay(answers: &classify::Answers) -> Option<hook::JevBelayAnswers> {
    let claims_done = answers.get("claims_done")?.noul?;
    let claims_verified = answers.get("claims_verified")?.noul?;
    let verification_applies = answers.get("verification_applies")?.noul?;
    let outcome = match answers.get("outcome").and_then(|a| a.choice.as_deref()) {
        Some("complete") => hook::ClassifiedOutcome::Complete,
        Some("partial") => hook::ClassifiedOutcome::Partial,
        Some("blocked") => hook::ClassifiedOutcome::Blocked,
        _ => hook::ClassifiedOutcome::Other,
    };
    Some(hook::JevBelayAnswers {
        claims_done,
        claims_verified,
        verification_applies,
        outcome,
    })
}

fn extract_last_assistant_message(payload: &serde_json::Value) -> Option<String> {
    if let Some(s) = payload
        .get("last_assistant_message")
        .and_then(|v| v.as_str())
    {
        return Some(s.to_string());
    }
    let transcript_path = payload.get("transcript_path").and_then(|v| v.as_str())?;
    let text = std::fs::read_to_string(transcript_path).ok()?;
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let role = v
            .get("role")
            .or_else(|| v.get("message").and_then(|m| m.get("role")))
            .and_then(|r| r.as_str());
        if role != Some("assistant") {
            continue;
        }
        let content = v
            .get("message")
            .and_then(|m| m.get("content"))
            .or_else(|| v.get("content"));
        if let Some(content) = content {
            if let Some(text) = content.as_str() {
                return Some(text.to_string());
            }
            if let Some(arr) = content.as_array() {
                let joined: String = arr
                    .iter()
                    .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !joined.is_empty() {
                    return Some(joined);
                }
            }
        }
    }
    None
}

// ===========================================================================
// Verify commands: sandboxed on macOS, cached by diff hash, cancellable.
// ===========================================================================

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

async fn run_verify_commands(
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
async fn run_verify_cached(
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

fn diff_hash(worktree: &Path, base: &str) -> String {
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
async fn kill_group(pgid: Option<i32>, child: &mut tokio::process::Child) {
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
fn split_verify_commands(cwd: &Path, commands: &[String]) -> (Vec<String>, Vec<String>) {
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

/// Claude's `total_cost_usd` covers the whole session, so a resumed
/// attempt's figure already includes every earlier attempt on that session;
/// adding it as-is counted the first attempt's cost once per resume.
/// That includes an earlier attempt whose cost was estimated after the
/// daemon killed it: its messages are part of the session the resume
/// continues, so it is subtracted like any other. An estimated cost covers
/// only its own run's messages, so it is taken as is.
fn attempt_cost(earlier: &[Attempt], attempt: &Attempt, run_cost: f64, estimated: bool) -> f64 {
    if estimated || !attempt.resumed || attempt.session_id.is_none() {
        return run_cost;
    }
    let already: f64 = earlier
        .iter()
        .filter(|a| a.session_id == attempt.session_id)
        .filter_map(|a| a.cost_usd)
        .sum();
    (run_cost - already).max(0.0)
}

type Prices = std::collections::BTreeMap<String, crate::model::Price>;

/// What a Claude run spent, replayed from its saved `events.jsonl`: the
/// CLI's own figure when its `result` made it to disk, else the priced
/// per-message usage, marked estimated. `None` for a missing file or one
/// with nothing priced -- which is every Codex run, whose events have no
/// Claude-shaped message and report usage only at the end of a turn.
fn replay_run_cost(events: &Path, prices: &Prices) -> Option<harness::RunOutcome> {
    let text = std::fs::read_to_string(events).ok()?;
    let mut outcome = harness::replay_events(Harness::Claude, &text);
    outcome.estimate_cost(prices);
    outcome.cost_usd.is_some().then_some(outcome)
}

/// The `events.jsonl` files of an attempt's own harness runs whose cost is
/// not in `cost_usd` yet. A plan attempt saves its first run's cost before
/// the retry run starts, so a recorded cost means only the retry is left.
fn uncounted_event_files(run_dir: &Path, attempt: &Attempt) -> Vec<PathBuf> {
    match attempt.stage {
        Stage::Plan => {
            let plan = run_dir.join("plan");
            let retry = plan.join("events-retry.jsonl");
            if attempt.cost_usd.is_some() {
                vec![retry]
            } else {
                vec![plan.join("events.jsonl"), retry]
            }
        }
        Stage::Implement | Stage::Review if attempt.cost_usd.is_none() => {
            vec![run_dir.join("events.jsonl")]
        }
        Stage::Implement | Stage::Review => vec![],
    }
}

/// For an attempt that ended before all its spend was recorded (stopped, or
/// the daemon died under it): adds whatever its runs left in their events
/// -- see [`replay_run_cost`] -- to the attempt and the task. That covers
/// the attempt's review run too (`runs/<n>/review`), kept in
/// `review_cost_usd`. Claude only, for the attempt's own runs; the review
/// may run on either harness.
fn settle_unfinished_cost(task: &mut Task, idx: usize, run_dir: &Path, prices: &Prices) {
    let attempt = &task.attempts[idx];
    if attempt.harness == Harness::Claude {
        let mut total = None;
        let mut estimated = false;
        let mut usage = Usage {
            input: 0,
            output: 0,
            cached: 0,
        };
        for path in uncounted_event_files(run_dir, attempt) {
            let Some(outcome) = replay_run_cost(&path, prices) else {
                continue;
            };
            let cost = outcome.cost_usd.unwrap_or(0.0);
            let cost = attempt_cost(&task.attempts[..idx], attempt, cost, outcome.cost_estimated);
            *total.get_or_insert(0.0) += cost;
            estimated |= outcome.cost_estimated;
            usage.input += outcome.usage_input;
            usage.output += outcome.usage_output;
            usage.cached += outcome.usage_cached;
        }
        if let Some(cost) = total {
            let attempt = &mut task.attempts[idx];
            attempt.cost_usd = Some(attempt.cost_usd.unwrap_or(0.0) + cost);
            attempt.cost_estimated |= estimated;
            let saved = attempt.usage.get_or_insert(Usage {
                input: 0,
                output: 0,
                cached: 0,
            });
            saved.input += usage.input;
            saved.output += usage.output;
            saved.cached += usage.cached;
            task.cost_usd += cost;
        }
    }
    // An attempt reviews once, and its review_cost_usd is saved right after.
    let attempt = &task.attempts[idx];
    if attempt.stage == Stage::Implement && attempt.review_cost_usd.is_none() {
        if let Some(o) = replay_run_cost(&run_dir.join("review").join("events.jsonl"), prices) {
            let cost = o.cost_usd.unwrap_or(0.0);
            task.attempts[idx].review_cost_usd = Some(cost);
            task.cost_usd += cost;
        }
    }
}

/// On daemon start, for a task that was between attempts: an advisor run
/// about its last failed attempt that the daemon died under. Its cost was
/// never saved (`advisor_cost_usd` is saved the moment one ends), so it is
/// replayed from `runs/<n>/advisor/events.jsonl`. Returns whether it added
/// anything.
fn settle_interrupted_advisor(
    task: &mut Task,
    run_dir_of: impl Fn(u32) -> PathBuf,
    prices: &Prices,
) -> bool {
    let Some(idx) = task
        .attempts
        .iter()
        .rposition(|a| a.stage == Stage::Implement)
    else {
        return false;
    };
    let attempt = &task.attempts[idx];
    if attempt.status != AttemptStatus::Failed || attempt.advisor_cost_usd.is_some() {
        return false;
    }
    let events = run_dir_of(attempt.n).join("advisor").join("events.jsonl");
    let Some(o) = replay_run_cost(&events, prices) else {
        return false;
    };
    let cost = o.cost_usd.unwrap_or(0.0);
    task.attempts[idx].advisor_cost_usd = Some(cost);
    task.cost_usd += cost;
    true
}

fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
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
fn verify_tail(stdout: &[u8], stderr: &[u8]) -> String {
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
async fn run_one_verify_command(
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

// ===========================================================================
// Harness process
// ===========================================================================

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
    let extra = [
        format!("{home}/.local/bin"),
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
    ];
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

enum RunError {
    Cancelled,
    Io(String),
}

/// Spawn `claude`/`codex` in its own process group, write `brief` to
/// stdin, stream stdout into `events.jsonl` and the log broadcast, and wait
/// for it to exit. `cancel` triggers SIGTERM to the group, then SIGKILL
/// after 5s (spec: "Each child in its own process group; stop = SIGTERM to
/// the group, SIGKILL after 5s").
///
/// `track_attempt` gates the mid-run `pgid`/`session_id` persistence below:
/// `true` for a session that *is* `task.attempts[_]` with number `attempt_n`
/// (the implement or plan attempt itself); `false` for a nested session
/// that merely borrows that attempt's number for its run directory (review,
/// orchestrator triage) -- otherwise its own pgid/session id would
/// overwrite the real attempt's, leaving it stuck `running` with a session
/// id `--resume` should never see and a pgid recovery would `killpg` on a
/// process that already exited (P1, triage review). A nested session's own
/// child is still reachable through `cancel` for as long as this call is
/// awaited, so skipping persistence here only affects recovery after a
/// daemon crash mid-review/-triage, not the normal stop path.
#[allow(clippy::too_many_arguments)]
async fn run_harness(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    track_attempt: bool,
    worktree: &Path,
    req: &harness::RunRequest<'_>,
    brief_text: &str,
    events_path: &Path,
    cancel: &CancelToken,
    stall: Option<Stall>,
) -> Result<harness::RunOutcome, RunError> {
    let argv = harness::build_argv(req);
    let bin = resolve_binary(req.harness);
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(&argv)
        .current_dir(worktree)
        .env("PATH", augmented_path())
        // Lets a repo's own hooks tell an orchd-run agent from a person's
        // session (sushiAI's lesson reminder stays quiet for it).
        .env("ORCHD_TASK", task_id)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| RunError::Io(format!("{bin}: {e}")))?;
    let pgid = child.id().map(|p| p as i32);
    if track_attempt {
        if let Some(pgid) = pgid {
            persist_attempt_field(app, task_id, attempt_n, move |a| a.pgid = Some(pgid)).await;
        }
    }

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(brief_text.as_bytes()).await;
        drop(stdin);
    }

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let mut out_lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stdout));
    let mut err_lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stderr));

    let mut outcome = harness::RunOutcome::default();
    let harness_kind = req.harness;
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut stderr_tail = String::new();
    let mut session_persisted = false;
    // ponytail: silence is the only stall signal; a long Bash call or the
    // Stop hook's verify (up to 540s) is silent too, so the timeout must sit
    // above them.
    // select! builds a disabled branch's future too: an unset timeout still
    // needs a deadline that doesn't overflow `Instant`.
    let stall_limit = stall
        .as_ref()
        .map_or(Duration::from_secs(365 * 24 * 3600), |s| s.limit);
    let mut last_output = tokio::time::Instant::now();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                kill_group(pgid, &mut child).await;
                return Err(RunError::Cancelled);
            }
            _ = tokio::time::sleep_until(last_output + stall_limit), if stall.is_some() => {
                if stall.as_ref().is_some_and(|s| s.paused.load(Ordering::SeqCst)) {
                    last_output = tokio::time::Instant::now();
                    continue;
                }
                kill_group(pgid, &mut child).await;
                outcome.stalled = true;
                outcome.error = Some(format!(
                    "No output for {}s; the run was stopped as stalled.",
                    stall_limit.as_secs()
                ));
                break;
            }
            line = out_lines.next_line(), if !stdout_done => {
                match line {
                    Ok(Some(l)) => {
                        last_output = tokio::time::Instant::now();
                        append_line(events_path, &l);
                        if let Some(note) = harness::feed_stream_line(harness_kind, &l, &mut outcome) {
                            app.broadcast_log(task_id, attempt_n, note);
                        }
                        if track_attempt && !session_persisted {
                            if let Some(sid) = outcome.session_id.clone() {
                                session_persisted = true;
                                persist_attempt_field(app, task_id, attempt_n, move |a| a.session_id = Some(sid)).await;
                            }
                        }
                    }
                    _ => { stdout_done = true; }
                }
            }
            line = err_lines.next_line(), if !stderr_done => {
                match line {
                    Ok(Some(l)) => {
                        last_output = tokio::time::Instant::now();
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
                    if !status.success() && outcome.error.is_none() {
                        let tail = stderr_tail.trim();
                        outcome.error = Some(if tail.is_empty() {
                            format!("{:?} exited with {status}.", req.harness)
                        } else {
                            tail.to_string()
                        });
                    }
                }
                break;
            }
        }
    }
    if outcome.cost_usd.is_none() && req.harness == Harness::Codex {
        let price = req
            .model
            .and_then(|m| app.settings.read().unwrap().prices.get(m).copied());
        outcome.cost_usd = price.map(|p| {
            p.codex_cost(
                outcome.usage_input,
                outcome.usage_cached,
                outcome.usage_output,
            )
        });
    }
    if req.harness == Harness::Claude {
        outcome.estimate_cost(&app.settings.read().unwrap().prices);
    }
    Ok(outcome)
}

// ===========================================================================
// Failure / waiting bookkeeping
// ===========================================================================

fn record_failure(task: &mut Task, idx: usize, kind: FailureKind, detail: String) {
    let signature = failure_signature(kind, &detail);
    let a = &mut task.attempts[idx];
    a.status = AttemptStatus::Failed;
    a.ended_at = Some(now_ms());
    a.failure = Some(Failure {
        kind,
        detail,
        signature,
    });
}

/// How many *implement* attempts a task has made -- the plan attempt (if
/// any) never counts against `maxAttempts`.
fn implement_attempt_count(task: &Task) -> u32 {
    task.attempts
        .iter()
        .filter(|a| a.stage == Stage::Implement)
        .count() as u32
}

fn advance_after_failure(task: &mut Task, max_attempts: u32) -> bool {
    let last = task.attempts.last().expect("failure just recorded");
    let signature = last
        .failure
        .as_ref()
        .map(|f| f.signature.clone())
        .unwrap_or_default();
    let consecutive = consecutive_same_signature(&task.attempts, &signature);
    let previous_signature = if task.attempts.len() >= 2 {
        task.attempts[task.attempts.len() - 2]
            .failure
            .as_ref()
            .map(|f| f.signature.clone())
    } else {
        None
    };
    let input = FailureDecisionInput {
        tier: task.tier,
        signature: &signature,
        previous_signature: previous_signature.as_deref(),
        consecutive_same: consecutive,
        attempt_n: implement_attempt_count(task),
        max_attempts,
    };
    match decide_after_failure(&input) {
        FailureDecision::NextAttempt { tier } => {
            // The fallback note stays until the tier actually changes.
            if tier != task.tier {
                task.tier_fallback = None;
            }
            task.tier = tier;
            task.status = TaskStatus::Queued;
            true
        }
        FailureDecision::Waiting { question } => {
            task.question = Some(Question {
                text: question,
                options: vec!["continue".into(), "change approach".into(), "stop".into()],
            });
            task.status = TaskStatus::Waiting;
            false
        }
    }
}

enum LoopSignal {
    /// `answered` is `true` when this iteration ends because the owner just
    /// answered a waiting question -- the caller uses it to decide whether
    /// the *next* attempt is still allowed to resume a previous session
    /// (spec: "never resume after a waiting/answer cycle").
    Continue {
        answered: bool,
    },
    Stop,
}

/// Record a failure, apply the tier/waiting rules, and -- when now waiting
/// -- park for an answer (the orchestrator's, within its bound, else the
/// owner's), extending the attempt budget by 2 on any
/// answer (spec step 9; every waiting state `advance_after_failure` reaches
/// is the "attempts exhausted" kind).
#[allow(clippy::too_many_arguments)]
async fn fail_and_continue(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    kind: FailureKind,
    detail: String,
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> LoopSignal {
    record_failure(task, idx, kind, detail);
    let should_continue = advance_after_failure(task, *attempt_budget);
    if should_continue {
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
        return LoopSignal::Continue { answered: false };
    }
    // Not persisted here: the wait does it itself, after the one-shot is
    // installed (see `wait_for_answer`).
    match wait_for_exhausted_answer(app, task_id, task, idx, pending_answer, cancel, permit).await {
        Some(_answer) => {
            *attempt_budget += 2;
            LoopSignal::Continue { answered: true }
        }
        None => LoopSignal::Stop,
    }
}

/// How many times the orchestrator may extend a task's budget on its own
/// before "attempts keep failing" goes to the owner: enough to push past a
/// fixable failure or a wrong review finding, not enough to burn the
/// budget in a loop.
const MAX_ORCHESTRATOR_CONTINUES: usize = 2;

/// The "attempts keep failing" question: the orchestrator triages it first
/// (it sees the last failure, e.g. the review findings, and can say what to
/// fix or that a finding is wrong), up to [`MAX_ORCHESTRATOR_CONTINUES`]
/// times per task; after that, or with auto-answer off, only the owner.
async fn wait_for_exhausted_answer(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<String> {
    let continues = task
        .decisions
        .iter()
        .filter(|d| d.starts_with(EXHAUSTED_ANSWER_PREFIX))
        .count();
    if continues < MAX_ORCHESTRATOR_CONTINUES {
        wait_for_answer_with_triage(app, task_id, task, idx, pending_answer, cancel, permit).await
    } else {
        wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await
    }
}

/// Parks on a fresh one-shot channel until either `task.answer` delivers an
/// answer or `cancel` fires. `task` must already have its `question`/
/// `status: Waiting` set by the caller -- it's persisted here, *after* the
/// one-shot is installed, so a `task.answer` that arrives the instant the
/// broadcast reaches a client can never beat the one-shot into existence
/// and get wrongly rejected as "no live parked loop". Persists the
/// decision/status transition either way afterwards, so a caller never has
/// to duplicate that bookkeeping.
async fn wait_for_answer(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<String> {
    // A task waiting for its owner holds no slot: parallel limits running
    // agents, not open questions.
    permit.take();
    let (tx, rx) = oneshot::channel();
    *pending_answer.lock().unwrap() = Some(tx);
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    let result = tokio::select! {
        _ = cancel.cancelled() => None,
        answer = rx => answer.ok(),
    };
    *pending_answer.lock().unwrap() = None;
    match &result {
        // The caller keeps using (and later saves) its own copy, so the
        // transition is applied to it rather than to a fresh load -- a
        // stale copy used to overwrite it with the question still set.
        Some(answer) => {
            task.decisions.push(format!("Owner: {answer}"));
            task.question = None;
            task.status = TaskStatus::Queued;
            task.updated_at = now_ms();
            let _ = app.store.save_task(task);
            app.broadcast_task(task);
        }
        None => {
            if let Ok(Some(mut task)) = app.store.load_task(task_id) {
                if task.status != TaskStatus::Stopped {
                    task.status = TaskStatus::Stopped;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                }
            }
        }
    }
    result
}

/// Starts every question the budget gate asks; `task.answer` uses it to
/// apply a "raise" that arrives with no live loop to deliver it to.
const BUDGET_QUESTION_PREFIX: &str = "This task has spent";

fn is_budget_raise(question: &Question, answer: &str) -> bool {
    question.text.starts_with(BUDGET_QUESTION_PREFIX) && answer.trim().eq_ignore_ascii_case("raise")
}

/// The gate before every stage run (plan, advisor, implement attempt,
/// review): while the task's spend meets its `variant.max_cost_usd` budget,
/// it waits for the owner -- "raise" adds the budget once more, "stop"
/// stops it -- instead of running. Never interrupts a run already going.
/// On `true` the task is back in `resume_status` holding a slot; on `false`
/// it is stopped (the wait already persisted that).
#[allow(clippy::too_many_arguments)]
async fn wait_while_over_budget(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    run: &str,
    resume_status: TaskStatus,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> bool {
    let mut waited = false;
    while let Some(budget) = task.cost_budget().filter(|b| task.cost_usd >= *b) {
        let step = task.variant().max_cost_usd;
        task.decisions.push(format!(
            "Orchestrator: budget ${budget:.2} reached (${:.2} spent); the {run} run waits for the owner",
            task.cost_usd
        ));
        let question = Question {
            text: format!(
                "{BUDGET_QUESTION_PREFIX} ${:.2}, reaching its ${budget:.2} budget, before the {run} run. Raise the budget by ${step:.2}, or stop?",
                task.cost_usd
            ),
            options: vec!["raise".into(), "stop".into()],
        };
        task.question = Some(question.clone());
        task.status = TaskStatus::Waiting;
        task.updated_at = now_ms();
        waited = true;
        match wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await {
            None => return false,
            Some(answer) => {
                if is_budget_raise(&question, &answer) {
                    task.budget_raises += 1;
                }
            }
        }
    }
    if !waited {
        return true;
    }
    task.status = resume_status;
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    if permit.is_none() {
        let acquired = tokio::select! {
            _ = cancel.cancelled() => None,
            p = app.slots.clone().acquire_owned() => p.ok(),
        };
        match acquired {
            Some(p) => *permit = Some(p),
            None => {
                mark_stopped_if_not_already(app, task_id).await;
                return false;
            }
        }
    }
    true
}

// ===========================================================================
// Orchestrator triage ("wake the orchestrator before bothering the owner")
// ===========================================================================

/// Owner-visible `task.decisions` lines for a triage outcome, kept as pure
/// formatting helpers (same convention as the `jev_*` ones above) so
/// they're unit-testable without a harness run.
fn orchestrator_answer_line(question: &str, answer: &str, reason: &str) -> String {
    format!("Orchestrator: {question} -> {answer} ({reason})")
}

fn orchestrator_escalate_line(reason: &str) -> String {
    format!("Orchestrator: escalated ({reason})")
}

/// A decision line produced by [`orchestrator_answer_line`] specifically --
/// `false` for an escalation line, an owner/agent line, or anything else.
/// Used to cap triage to one *answer* in a row (P1-2).
fn is_orchestrator_answer_decision(line: &str) -> bool {
    line.starts_with("Orchestrator: ") && line.contains(" -> ")
}

/// A completed triage session: the decision, plus whatever it cost (the
/// caller adds this to `task.cost_usd` itself, same as an implement/plan
/// attempt would).
struct TriageRun {
    decision: brief::TriageDecision,
    cost_usd: Option<f64>,
}

/// One fresh, read-only session on the `orchestrator` route, asked to
/// answer or escalate a question that would otherwise go straight to the
/// owner. `None` means "don't triage this one" -- the route is off or names
/// no configured route (same semantics as `planner`'s unknown-id case,
/// except a triage failure never fails the task, it just falls through to
/// asking the owner as before). A harness error or an unparseable reply
/// still produces `Some`, escalating with the original question untouched
/// (`sanitize_triage`) -- fail-open to the owner, never loops.
#[allow(clippy::too_many_arguments)]
async fn run_triage(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    task: &Task,
    worktree: &Path,
    question: &str,
    options: &[String],
    cancel: &CancelToken,
) -> Option<TriageRun> {
    // P1-2: at most one consecutive triage *answer* per task -- if the
    // previous decision recorded is already an orchestrator answer, this
    // question goes straight to the owner. Without this, a recurring
    // blocked question (or a planner that keeps re-asking) could let
    // triage answer every single time with no bound at all.
    // "Attempts keep failing" has its own bound (MAX_ORCHESTRATOR_CONTINUES).
    if !question.starts_with(EXHAUSTED_QUESTION)
        && task
            .decisions
            .last()
            .map(|d| is_orchestrator_answer_decision(d))
            .unwrap_or(false)
    {
        return None;
    }
    let settings = app.settings.read().unwrap().clone();
    if !settings.auto_answer {
        return None;
    }
    let route = chat::orchestrator_route(&settings)?;

    let run_dir = app.store.run_dir(task_id, attempt_n).join("triage");
    let _ = std::fs::create_dir_all(&run_dir);
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            &route,
            app,
            &key_path,
            &settings,
            &deny_read,
            &settings_path,
        );
    }
    let req = harness::RunRequest {
        harness: route.harness,
        worktree,
        model: route.model.as_deref(),
        effort: route.effort.as_deref(),
        resume: None,
        review: true,
        mcp_config: Some(&mcp_path),
        settings_path: Some(&settings_path),
        network_allowed: false,
        codex_mcp: None,
        images: &[],
        repo_settings: true,
    };
    let brief_text = brief::build_triage_brief(task, question, options);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let run_result = run_harness(
        app,
        task_id,
        attempt_n,
        false,
        worktree,
        &req,
        &brief_text,
        &events_path,
        cancel,
        None,
    )
    .await;
    let _ = std::fs::remove_file(&key_path);

    match run_result {
        // The task was stopped mid-triage: don't manufacture an escalation
        // for it, just skip triage and let the normal wait pick up the
        // cancellation itself (its own `cancel.cancelled()` branch).
        Err(RunError::Cancelled) => None,
        Ok(o) if o.error.is_none() => {
            let cost_usd = o.cost_usd;
            let text = o.final_text.unwrap_or_default();
            Some(TriageRun {
                decision: brief::sanitize_triage(brief::parse_triage(&text), question, options),
                cost_usd,
            })
        }
        Ok(o) => Some(TriageRun {
            decision: brief::sanitize_triage(None, question, options),
            cost_usd: o.cost_usd,
        }),
        Err(RunError::Io(_)) => Some(TriageRun {
            decision: brief::sanitize_triage(None, question, options),
            cost_usd: None,
        }),
    }
}

/// Wraps [`wait_for_answer`] with one shot at orchestrator triage first --
/// for the agent's own blocked question, and for "attempts keep failing"
/// through [`wait_for_exhausted_answer`], which bounds how often the
/// orchestrator may extend the budget. `run_triage` caps any other question
/// to one *answer* in a row. `task.question` must already be set by the
/// caller, exactly as for a plain `wait_for_answer` -- an escalation only
/// ever sharpens it, it never invents a question from nothing. Never use
/// this for the protected-path approval question: only the owner may
/// approve that.
///
/// P1-1: on `Answer`, mutates and saves the caller's own `task` in place
/// (exactly like `wait_for_answer`'s own post-answer step) instead of
/// reloading from disk -- `task` is this iteration's accumulated attempt
/// state (failure, cost, usage, summary, changed files, decisions), most of
/// which is never persisted anywhere until this call's `save_task`; an
/// independent reload would silently discard all of it and leave the
/// attempt stuck `running`.
#[allow(clippy::too_many_arguments)]
async fn wait_for_answer_with_triage(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<String> {
    let question = task.question.clone().unwrap_or(Question {
        text: String::new(),
        options: vec![],
    });
    let attempt_n = task.attempts[idx].n;
    let worktree = PathBuf::from(&task.worktree);
    if let Some(TriageRun { decision, cost_usd }) = run_triage(
        app,
        task_id,
        attempt_n,
        task,
        &worktree,
        &question.text,
        &question.options,
        cancel,
    )
    .await
    {
        if let Some(cost) = cost_usd {
            task.cost_usd += cost;
        }
        match decision.action {
            brief::TriageAction::Answer => {
                let line =
                    orchestrator_answer_line(&question.text, &decision.answer, &decision.reason);
                app.broadcast_log(task_id, attempt_n, line.clone());
                task.decisions.push(line);
                task.question = None;
                task.status = TaskStatus::Queued;
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
                app.broadcast_task(task);
                return Some(decision.answer);
            }
            brief::TriageAction::Escalate => {
                task.question = Some(Question {
                    text: decision.question,
                    options: decision.options,
                });
                task.decisions
                    .push(orchestrator_escalate_line(&decision.reason));
            }
        }
    }
    wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await
}

/// Wraps [`ask_plan_question`] with the same orchestrator-triage shot, for
/// the planner's own draft questions specifically -- not the "no
/// verification command" synthesized question or the retry-clarification
/// question, which still go straight to the owner unchanged. Same P1-1 fix
/// as [`wait_for_answer_with_triage`]: mutates and saves the caller's own
/// `task` in place rather than an independent reload.
#[allow(clippy::too_many_arguments)]
async fn ask_plan_question_with_triage(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    attempt_n: u32,
    worktree: &Path,
    question_text: &str,
    options: Vec<String>,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<String> {
    if let Some(TriageRun { decision, cost_usd }) = run_triage(
        app,
        task_id,
        attempt_n,
        task,
        worktree,
        question_text,
        &options,
        cancel,
    )
    .await
    {
        if let Some(cost) = cost_usd {
            task.cost_usd += cost;
        }
        match decision.action {
            brief::TriageAction::Answer => {
                let line =
                    orchestrator_answer_line(question_text, &decision.answer, &decision.reason);
                app.broadcast_log(task_id, attempt_n, line.clone());
                task.decisions.push(line);
                task.question = None;
                task.status = TaskStatus::Drafting;
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
                app.broadcast_task(task);
                return Some(decision.answer);
            }
            brief::TriageAction::Escalate => {
                task.decisions
                    .push(orchestrator_escalate_line(&decision.reason));
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
                app.broadcast_task(task);
                return ask_plan_question(
                    app,
                    task_id,
                    &decision.question,
                    decision.options,
                    pending_answer,
                    cancel,
                    permit,
                )
                .await;
            }
        }
    }
    ask_plan_question(
        app,
        task_id,
        question_text,
        options,
        pending_answer,
        cancel,
        permit,
    )
    .await
}

// ===========================================================================
// Classifier calls used by the engine loop
// ===========================================================================

/// Recorded once per task and stage/tier when a variant's route override
/// replaces the settings' route.
fn variant_route_line(what: &str, route_id: &str) -> String {
    format!("Variant: {what} -> route {route_id} (override)")
}

/// Owner-visible `task.decisions` lines for a successful classifier call,
/// kept as pure formatting helpers so they're unit-testable without a
/// network call. Never fed anything but probabilities/choices/route ids --
/// no key, base URL, or raw response body ever reaches these.
fn jev_tier_line(choice: &str, p: f64, route_id: &str) -> String {
    format!("Jev: tier {choice} (p {p:.2}) -> route {route_id}")
}

fn jev_tier_fallback_line(reason: &str, route_id: &str) -> String {
    format!("Jev: tier unavailable ({reason}) -> fallback standard, route {route_id}")
}

fn jev_answerable_line(p: f64, answer_self: bool) -> String {
    let outcome = if answer_self {
        "agent sent back"
    } else {
        "asked owner"
    };
    format!("Jev: answerable from repo (p {p:.2}) -> {outcome}")
}

fn jev_stop_gate_line(blocked: bool, claims_done: f64, claims_verified: f64) -> String {
    if blocked {
        format!("Jev: premature finish (p {claims_done:.2}) -> sent back")
    } else {
        format!("Jev: verification looks fine (p {claims_verified:.2}) -> allowed")
    }
}

fn jev_plan_preflight_line(
    goal_specific: f64,
    criteria_checkable: f64,
    has_verification: f64,
) -> String {
    format!(
        "Jev: goal {goal_specific:.2}, criteria {criteria_checkable:.2}, verification {has_verification:.2}"
    )
}

/// Either a classified tier (with the raw `choice`/`p` for the caller to
/// turn into a `task.decisions` line once it knows the resolved route), or
/// a fallback to `Tier::Standard` with a reason from the fixed set `pick_tier`
/// documents -- this type doesn't persist anything itself so the caller can
/// push a line onto the same in-memory `Task` it's about to save (see
/// `append_jev_decision` for why a call site with no live `Task` in scope
/// has to do it differently).
enum TierPick {
    Classified { tier: Tier, choice: String, p: f64 },
    Fallback { reason: String },
}

/// Turns a classifier call outcome into a tier decision. Pure and
/// unit-testable: no network, no store, no app state. `reason` is always
/// one of `classifier off`, `no classifier key`, `classifier call failed`,
/// `no tier answer`, or `unsure: <choice> p <p:.2>` -- never the raw
/// classifier error text, a key, a URL, or a response body.
fn pick_tier(
    result: &Result<classify::Answers, classify::ClassifyError>,
    backend_off: bool,
) -> TierPick {
    if backend_off {
        return TierPick::Fallback {
            reason: "classifier off".to_string(),
        };
    }
    let answers = match result {
        Err(e) if e.0 == classify::NO_KEY_REASON => {
            return TierPick::Fallback {
                reason: "no classifier key".to_string(),
            };
        }
        Err(_) => {
            return TierPick::Fallback {
                reason: "classifier call failed".to_string(),
            };
        }
        Ok(answers) => answers,
    };
    let Some(a) = answers.get("tier") else {
        return TierPick::Fallback {
            reason: "no tier answer".to_string(),
        };
    };
    let Some(choice) = &a.choice else {
        return TierPick::Fallback {
            reason: "no tier answer".to_string(),
        };
    };
    let p = a
        .probabilities
        .as_ref()
        .and_then(|p| p.get(choice))
        .copied()
        .unwrap_or(1.0);
    if p < 0.5 {
        return TierPick::Fallback {
            reason: format!("unsure: {choice} p {p:.2}"),
        };
    }
    let tier = match choice.as_str() {
        "mechanical" => Tier::Mechanical,
        "hard" => Tier::Hard,
        _ => Tier::Standard,
    };
    TierPick::Classified {
        tier,
        choice: choice.clone(),
        p,
    }
}

async fn classify_tier(app: &Arc<App>, task: &Task) -> TierPick {
    let settings = app.settings.read().unwrap().classifier.clone();
    let key = app.secrets.read().unwrap().classifier_key.clone();
    let base_url = app.secrets.read().unwrap().classifier_base_url.clone();
    let backend_off = settings.backend == ClassifierBackend::None;
    let state = json!({"goal": task.goal, "criteria": task.criteria});
    let questions = vec![classify::QuestionSpec::Choice {
        name: "tier".to_string(),
        prompt: "How hard is this task: mechanical, standard, or hard?".to_string(),
        options: vec!["mechanical".into(), "standard".into(), "hard".into()],
    }];
    let start = std::time::Instant::now();
    let s2 = settings.clone();
    let k2 = key.clone();
    let b2 = base_url.clone();
    let q2 = questions.clone();
    let state2 = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        classify::decide(&s2, k2.as_deref(), b2.as_deref(), &state2, &q2)
    })
    .await
    .unwrap_or_else(|e| Err(classify::ClassifyError(e.to_string())));
    app.journal(&task.id, "tier", &result, start.elapsed());
    pick_tier(&result, backend_off)
}

async fn classify_answerable(app: &Arc<App>, task: &Task, question: &str) -> Option<f64> {
    let settings = app.settings.read().unwrap().classifier.clone();
    let key = app.secrets.read().unwrap().classifier_key.clone();
    let base_url = app.secrets.read().unwrap().classifier_base_url.clone();
    let state = json!({"goal": task.goal, "criteria": task.criteria, "question": question});
    let questions = vec![classify::QuestionSpec::Noul {
        name: "answerable".to_string(),
        prompt: "Is this question answerable from the repository and task, without the owner?"
            .to_string(),
    }];
    let start = std::time::Instant::now();
    let s2 = settings.clone();
    let k2 = key.clone();
    let b2 = base_url.clone();
    let q2 = questions.clone();
    let state2 = state.clone();
    let result = tokio::task::spawn_blocking(move || {
        classify::decide(&s2, k2.as_deref(), b2.as_deref(), &state2, &q2)
    })
    .await
    .unwrap_or_else(|e| Err(classify::ClassifyError(e.to_string())));
    app.journal(&task.id, "blocked_question", &result, start.elapsed());
    result
        .ok()
        .and_then(|answers| answers.get("answerable").and_then(|a| a.noul))
}

// ===========================================================================
// Review
// ===========================================================================

/// The run request of a read-only session: review's, and anything that must
/// be held to the same restrictions (no edits, no network, never resumed,
/// no MCP servers but the empty config it is handed).
fn review_request<'a>(
    route: &'a Route,
    worktree: &'a Path,
    mcp_config: &'a Path,
    settings_path: &'a Path,
    images: &'a [PathBuf],
) -> harness::RunRequest<'a> {
    harness::RunRequest {
        harness: route.harness,
        worktree,
        model: route.model.as_deref(),
        effort: route.effort.as_deref(),
        resume: None,
        review: true,
        mcp_config: Some(mcp_config),
        settings_path: Some(settings_path),
        network_allowed: false,
        codex_mcp: None,
        images,
        repo_settings: true,
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_review(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    task: &Task,
    worktree: &Path,
    base_sha: &str,
    verify_results: &[VerifyOutcome],
    implementer_note: &str,
    review_route: &Route,
    deny_read: &[String],
    cancel: &CancelToken,
    cost_usd: &mut f64,
) -> Result<ReviewResult, RunError> {
    let wt = worktree.to_path_buf();
    let base = base_sha.to_string();
    let diff =
        tokio::task::spawn_blocking(move || git::diff_full(&wt, &base, 60_000).unwrap_or_default())
            .await
            .unwrap_or_default();

    let mut brief_text = String::new();
    brief_text.push_str("## Review\n\n");
    brief_text.push_str(&task.goal);
    brief_text.push_str("\n\n## Acceptance criteria\n\n");
    for c in &task.criteria {
        brief_text.push_str("- ");
        brief_text.push_str(c);
        brief_text.push('\n');
    }
    // Blind review: the implementer's account is left out entirely.
    if !task.variant().review_blind {
        brief_text.push_str("\n## Implementer\n\n");
        brief_text.push_str(implementer_note);
    }
    brief_text.push_str("\n\nCriteria marked \"Checked by review\" have no command behind them: check them from the diff and the repository yourself.\n\nThe repository's process rules about commits, pull requests, release notes and lesson or changelog files belong to the orchestrator, not this task: judge the change against the task and its criteria, and do not fail it for those.\n");
    let evidence = task.variant().review_evidence;
    brief_text.push_str("\n## Verify results\n\n");
    for v in verify_results {
        brief_text.push_str(&format!(
            "- `{}` -> exit {:?}\n```\n{}\n```\n",
            v.command,
            v.code,
            tail_chars(v.tail.trim(), if evidence { 3000 } else { 800 })
        ));
    }
    let images = if evidence {
        let since = task
            .attempts
            .iter()
            .find(|a| a.n == attempt_n && a.stage == Stage::Implement)
            .map_or(0, |a| a.started_at);
        attempt_screenshots(worktree, since)
    } else {
        Vec::new()
    };
    if !images.is_empty() {
        brief_text.push_str("\n## Screenshots\n\nSaved by this attempt. Open each one and check it against the criteria it is meant to prove; a screenshot that does not show what a criterion claims is a finding.\n\n");
        for image in &images {
            let shown = image.strip_prefix(worktree).unwrap_or(image);
            brief_text.push_str(&format!("- `{}`\n", shown.display()));
        }
    }
    brief_text.push_str("\n## Diff\n\n```diff\n");
    brief_text.push_str(&diff);
    brief_text.push_str("\n```\n\n## Report format\n\nReply with:\n\n```sushi-review\n{\"verdict\":\"PASS|FAIL\",\"findings\":[]}\n```\n");
    if task.variant().contract {
        brief_text.push('\n');
        brief_text.push_str(brief::REVIEW_CONTRACT);
        brief_text.push('\n');
    }

    // Its own subdirectory: a review's events/brief/settings must never
    // land in the implement attempt's `runs/<n>/` files.
    let run_dir = app.store.run_dir(task_id, attempt_n).join("review");
    let _ = std::fs::create_dir_all(&run_dir);
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let settings_snapshot = app.settings.read().unwrap().clone();
    let settings_path = run_dir.join("settings.json");
    if matches!(review_route.harness, Harness::Claude) {
        // No token is ever registered for a review run (it's read-only and
        // never gated), so it must not get a Stop hook either -- installing
        // one would just be a guaranteed fail-open round trip.
        let claude_settings = harness::build_claude_settings(
            None,
            settings_snapshot.sandbox,
            &settings_snapshot.allowed_domains,
            deny_read,
            None,
        );
        let _ = store::write_json_atomic(&settings_path, &claude_settings);
    }

    let req = review_request(review_route, worktree, &mcp_path, &settings_path, &images);
    match run_harness(
        app,
        task_id,
        attempt_n,
        false,
        worktree,
        &req,
        &brief_text,
        &events_path,
        cancel,
        None,
    )
    .await
    {
        Ok(outcome) => {
            *cost_usd += outcome.cost_usd.unwrap_or(0.0);
            let text = outcome.final_text.unwrap_or_default();
            if let Some(result) = brief::parse_review(&text) {
                return Ok(result);
            }
            Err(RunError::Io(match outcome.error {
                Some(error) => format!("review did not run ({error})"),
                None => format!(
                    "review reply had no sushi-review verdict: {}",
                    tail_chars(text.trim(), 600)
                ),
            }))
        }
        Err(RunError::Cancelled) => {
            // Stopped mid-review: what it streamed so far is still spent.
            *cost_usd += replay_run_cost(&events_path, &settings_snapshot.prices)
                .and_then(|o| o.cost_usd)
                .unwrap_or(0.0);
            Err(RunError::Cancelled)
        }
        Err(e) => Err(e),
    }
}

// ===========================================================================
// Advisor
// ===========================================================================

/// Longest advice kept, in characters.
const MAX_ADVICE_CHARS: usize = 1500;

/// One read-only call on the task's planner route about the implement attempt
/// that just failed, made before the next attempt starts. The answer is
/// stored as the failed attempt's `advice`, and the run's cost as its
/// `advisor_cost_usd`, which is also returned (`None` when no run happened).
/// A failed attempt is advised at most once: a run that was stopped or gave
/// no answer is not paid for again. Never fails the task: no route, an
/// escalation to the advisor's own model, a failed or stopped run or an
/// empty answer all just mean no advice.
async fn run_advisor_before_retry(
    app: &Arc<App>,
    task: &mut Task,
    settings: &Settings,
    next_route: &Route,
    worktree: &Path,
    base_sha: &str,
    cancel: &CancelToken,
) -> Option<f64> {
    let failed = task
        .attempts
        .iter()
        .rposition(|a| a.stage == Stage::Implement)?;
    let attempt = &task.attempts[failed];
    let failure = attempt.failure.as_ref()?;
    if attempt.status != AttemptStatus::Failed
        || failure.kind == FailureKind::Blocked
        || attempt.advice.is_some()
        || attempt.advisor_cost_usd.is_some()
    {
        return None;
    }
    let variant = task.variant();
    let planner = variant.plan_route_id(settings);
    let advisor = settings.routes.iter().find(|r| r.id == planner)?;
    if advisor.id == next_route.id
        || (advisor.harness == next_route.harness
            && advisor.model.is_some()
            && advisor.model == next_route.model)
    {
        return None;
    }
    let attempt_n = attempt.n;
    let failure_detail = failure.detail.clone();

    let (wt, base) = (worktree.to_path_buf(), base_sha.to_string());
    let diff =
        tokio::task::spawn_blocking(move || git::diff_full(&wt, &base, 20_000).unwrap_or_default())
            .await
            .unwrap_or_default();
    let brief_text = brief::build_advisor_brief(task, &diff, &failure_detail);

    let run_dir = app.store.run_dir(&task.id, attempt_n).join("advisor");
    let _ = std::fs::create_dir_all(&run_dir);
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
    let events_path = run_dir.join("events.jsonl");
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(advisor.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            advisor,
            app,
            &key_path,
            settings,
            &deny_read,
            &settings_path,
        );
    }
    let req = harness::RunRequest {
        harness: advisor.harness,
        worktree,
        model: advisor.model.as_deref(),
        effort: advisor.effort.as_deref(),
        resume: None,
        review: true,
        mcp_config: Some(&mcp_path),
        settings_path: Some(&settings_path),
        network_allowed: false,
        codex_mcp: None,
        images: &[],
        repo_settings: true,
    };
    let result = run_harness(
        app,
        &task.id,
        attempt_n,
        false,
        worktree,
        &req,
        &brief_text,
        &events_path,
        cancel,
        None,
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    // The run wrote the attempt's session/pgid to disk mid-run; keep the
    // caller's copy and only add the advice.
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(RunError::Cancelled) => {
            let cost = replay_run_cost(&events_path, &settings.prices)
                .and_then(|o| o.cost_usd)
                .unwrap_or(0.0);
            task.attempts[failed].advisor_cost_usd = Some(cost);
            return Some(cost);
        }
        Err(RunError::Io(_)) => return None,
    };
    let cost = outcome.cost_usd.unwrap_or(0.0);
    task.attempts[failed].advisor_cost_usd = Some(cost);
    let text = outcome.final_text.unwrap_or_default();
    let advice = text.trim();
    if outcome.error.is_none() && !advice.is_empty() {
        task.attempts[failed].advice = Some(truncate_chars(advice, MAX_ADVICE_CHARS));
    }
    Some(cost)
}

// ===========================================================================
// Drafting / plan stage
// ===========================================================================

/// Suggested quick-reply options for the synthesized "no verification
/// command" question: each of the target repo's own `package.json` scripts,
/// as `npm run <script>`, `test`-named scripts ranked first (they're by far
/// the most likely answer). Empty (not an error) when there's no
/// `package.json` or no `scripts` -- the question still accepts free text.
fn verify_options_from_package_json(worktree: &Path) -> Vec<String> {
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

/// Writes `settings.json` for a read-only Claude session (no Stop hook --
/// used by the plan stage and orchestrator triage, never the implement
/// path, which additionally wires one up) that still carries its route's
/// profile (env/API key) the same way an implement attempt would. A no-op
/// for a Codex route, which has no such settings file.
fn write_readonly_claude_settings_with_profile(
    route: &Route,
    app: &App,
    key_path: &Path,
    settings: &Settings,
    deny_read: &[String],
    settings_path: &Path,
) {
    let profile = route
        .profile_id
        .as_ref()
        .and_then(|pid| app.secrets.read().unwrap().profiles.get(pid).cloned());
    let mut profile_obj = serde_json::Map::new();
    if let Some(p) = &profile {
        if !p.env.is_empty() {
            profile_obj.insert(
                "env".to_string(),
                serde_json::to_value(&p.env).unwrap_or(serde_json::Value::Null),
            );
        }
        if let Some(key) = &p.key {
            if store::write_secret_file(key_path, key).is_ok() {
                profile_obj.insert(
                    "apiKeyHelper".to_string(),
                    serde_json::Value::String(format!(
                        "cat {}",
                        harness::shell_quote(&key_path.to_string_lossy())
                    )),
                );
            }
        }
    }
    let profile_value = if profile_obj.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(profile_obj))
    };
    let claude_settings = harness::build_claude_settings(
        profile_value.as_ref(),
        settings.sandbox,
        &settings.allowed_domains,
        deny_read,
        None,
    );
    let _ = store::write_json_atomic(settings_path, &claude_settings);
}

/// Sets the question/waiting state, parks on a fresh one-shot the same way
/// [`wait_for_answer`] does, then records the decision as `"Owner: <question>
/// -> <answer>"` (spec step 5) instead of the generic attempt-failure
/// phrasing. `"stop"` is still handled centrally by `handle_task_answer`
/// before it ever reaches here.
async fn ask_plan_question(
    app: &Arc<App>,
    task_id: &str,
    question_text: &str,
    options: Vec<String>,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> Option<String> {
    // Same as wait_for_answer: a task parked on a question holds no slot.
    permit.take();
    // Install the one-shot *before* the state that makes this externally
    // visible as "waiting" is even persisted: otherwise a `task.answer`
    // that arrives the instant the broadcast reaches a client could beat
    // this into existence and get rejected as "no live parked loop" even
    // though a loop genuinely is about to park.
    let (tx, rx) = oneshot::channel();
    *pending_answer.lock().unwrap() = Some(tx);
    if let Ok(Some(mut task)) = app.store.load_task(task_id) {
        task.question = Some(Question {
            text: question_text.to_string(),
            options,
        });
        task.status = TaskStatus::Waiting;
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);
    }

    let result = tokio::select! {
        _ = cancel.cancelled() => None,
        answer = rx => answer.ok(),
    };
    *pending_answer.lock().unwrap() = None;

    match &result {
        Some(answer) => {
            if let Ok(Some(mut task)) = app.store.load_task(task_id) {
                task.decisions
                    .push(format!("Owner: {question_text} -> {answer}"));
                task.question = None;
                task.status = TaskStatus::Drafting;
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
            }
        }
        None => {
            if let Ok(Some(mut task)) = app.store.load_task(task_id) {
                if task.status != TaskStatus::Stopped {
                    task.status = TaskStatus::Stopped;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                }
            }
        }
    }
    result
}

pub enum PlanOutcome {
    /// Planning finished and `auto_start_after_plan` was set: the task is
    /// now `queued`, fall straight through to the implement stage.
    Proceed,
    /// Planning finished but `auto_start_after_plan` was not set: the task
    /// is `stopped` with everything filled in for the owner to review.
    StoppedForReview,
    /// Cancelled, or ended in `failed`/`stopped` some other way -- the task
    /// loop itself is done, the caller should return.
    Ended,
}

/// Whether a task still needs to run (or re-run) the plan stage, derived
/// purely from data -- never from `task.status` -- so a stop-then-start
/// while drafting, an owner answer that arrives with no live loop to catch
/// it (daemon restart), or a manual `task.start` on a task recovery left
/// `stopped`/`failed` mid-draft, all correctly go back through planning
/// instead of quietly falling through to implement with blank fields. Once
/// one plan attempt has *passed*, this is permanently `false` for the rest
/// of the task's life -- planning runs at most once, ever.
fn needs_planning(task: &Task) -> bool {
    task.request.is_some()
        && !task
            .attempts
            .iter()
            .any(|a| a.stage == Stage::Plan && a.status == AttemptStatus::Passed)
}

/// Common cleanup before returning `PlanOutcome::Ended`: if the plan
/// attempt at `idx` is still `running` (cut short by cancellation between
/// its own steps, rather than already concluded `passed`/`failed` by the
/// caller), mark it `interrupted`; either way, make sure the task itself
/// ends up `stopped` unless it's already some other terminal status.
async fn end_plan_stage(app: &Arc<App>, task_id: &str, idx: Option<usize>) -> PlanOutcome {
    if let Some(idx) = idx {
        if let Ok(Some(mut t)) = app.store.load_task(task_id) {
            if let Some(a) = t.attempts.get_mut(idx) {
                if a.status == AttemptStatus::Running {
                    a.status = AttemptStatus::Interrupted;
                    a.ended_at = Some(now_ms());
                    let run_dir = app.store.run_dir(task_id, a.n);
                    let prices = app.settings.read().unwrap().prices.clone();
                    settle_unfinished_cost(&mut t, idx, &run_dir, &prices);
                    t.updated_at = now_ms();
                    let _ = app.store.save_task(&t);
                    app.broadcast_task(&t);
                }
            }
        }
    }
    mark_stopped_if_not_already(app, task_id).await;
    PlanOutcome::Ended
}

/// Runs the drafting stage (spec "Plan stage" steps 1-6) for a task created
/// via the `{repo, request}` form: a fresh read-only planner session drafts
/// title/goal/criteria/verify from the owner's one-sentence request (one
/// retry if the reply doesn't parse, then the owner is asked directly, up
/// to 2 such clarification rounds before giving up), runs the classifier
/// preflight on the draft, and asks any of the planner's own questions
/// (plus a verification question whenever the draft still has none) one at
/// a time before handing off to the ordinary implement loop.
async fn run_plan_stage(
    app: &Arc<App>,
    task_id: &str,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
    auto_start_after_plan: bool,
) -> PlanOutcome {
    let mut clarify_rounds: u32 = 0;
    loop {
        if cancel.is_cancelled() {
            return end_plan_stage(app, task_id, None).await;
        }
        // The very first round already holds the slot `run_task_loop`
        // acquired before calling in; a round that starts fresh right after
        // releasing one to park on a clarifying question needs its own, or
        // it would run the next harness session with no permit at all.
        if permit.is_none() {
            let acquired = tokio::select! {
                _ = cancel.cancelled() => None,
                p = app.slots.clone().acquire_owned() => p.ok(),
            };
            match acquired {
                Some(p) => *permit = Some(p),
                None => return end_plan_stage(app, task_id, None).await,
            }
        }

        let mut task = match app.store.load_task(task_id) {
            Ok(Some(t)) => t,
            _ => return end_plan_stage(app, task_id, None).await,
        };
        if !wait_while_over_budget(
            app,
            task_id,
            &mut task,
            "plan",
            TaskStatus::Drafting,
            pending_answer,
            cancel,
            permit,
        )
        .await
        {
            return end_plan_stage(app, task_id, None).await;
        }

        let settings = app.settings.read().unwrap().clone();
        let variant = task.variant();
        let planner = variant.plan_route_id(&settings);
        let Some(route) = settings.routes.iter().find(|r| r.id == planner).cloned() else {
            // An unconfigured planner id is treated exactly like planner ==
            // "" -- never a silent fallback to some route the owner never
            // chose for this.
            if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                t.status = TaskStatus::Failed;
                t.updated_at = now_ms();
                let _ = app.store.save_task(&t);
                app.broadcast_task(&t);
            }
            return end_plan_stage(app, task_id, None).await;
        };

        if variant.planner_route.is_some() {
            let line = variant_route_line("planner", &route.id);
            if !task.decisions.contains(&line) {
                task.decisions.push(line);
            }
        }

        let request_text = task.request.clone().unwrap_or_default();
        let attempt_n = task.attempts.len() as u32 + 1;
        let worktree = PathBuf::from(&task.worktree);
        let run_dir = app.store.run_dir(task_id, attempt_n).join("plan");
        let _ = std::fs::create_dir_all(&run_dir);

        let attempt = Attempt {
            n: attempt_n,
            stage: Stage::Plan,
            route_id: route.id.clone(),
            harness: route.harness,
            model: route.model.clone().unwrap_or_default(),
            reason: "drafting".to_string(),
            session_id: None,
            pgid: None,
            resumed: false,
            started_at: now_ms(),
            ended_at: None,
            status: AttemptStatus::Running,
            summary: None,
            handoff: None,
            changed_files: vec![],
            verify: vec![],
            gate_blocks: 0,
            prefix_tokens: None,
            review: None,
            failure: None,
            usage: None,
            cost_usd: None,
            cost_estimated: false,
            review_cost_usd: None,
            advice: None,
            advisor_cost_usd: None,
        };
        task.attempts.push(attempt);
        let idx = task.attempts.len() - 1;
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);

        let mcp_path = run_dir.join("mcp.json");
        let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
        let settings_path = run_dir.join("settings.json");
        let key_path = run_dir.join("key");
        let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
        if matches!(route.harness, Harness::Claude) {
            // Same lean shape as review (read-only, no Stop hook -- no
            // token is ever registered for a drafting session), but the
            // planner route can still carry a profile (env/API key) the
            // same way an implement route can.
            write_readonly_claude_settings_with_profile(
                &route,
                app,
                &key_path,
                &settings,
                &deny_read,
                &settings_path,
            );
        }
        let req = harness::RunRequest {
            harness: route.harness,
            worktree: &worktree,
            model: route.model.as_deref(),
            effort: route.effort.as_deref(),
            resume: None,
            review: true,
            mcp_config: Some(&mcp_path),
            settings_path: Some(&settings_path),
            network_allowed: false,
            codex_mcp: None,
            images: &[],
            repo_settings: true,
        };

        let mut draft: Option<brief::PlanDraft> = None;
        let mut hard_failure = false;
        let mut over_budget = false;
        for retry in 0..2 {
            if cancel.is_cancelled() {
                let _ = std::fs::remove_file(&key_path);
                return end_plan_stage(app, task_id, Some(idx)).await;
            }
            if retry > 0 {
                // The retry is a run of its own: when the first one reached
                // the budget, this attempt ends and the next round's gate
                // asks the owner before any further planner run.
                if task.cost_budget().is_some_and(|b| task.cost_usd >= b) {
                    over_budget = true;
                    break;
                }
                // The first run's cost, saved before the reload after the
                // retry would drop it; recovery then counts only the retry.
                let _ = app.store.save_task(&task);
            }
            let brief_text = match (retry == 0, task.parent.is_some()) {
                (true, false) => brief::build_plan_brief(&request_text, &task.variant()),
                (false, false) => brief::build_plan_retry_brief(&request_text, &task.variant()),
                (true, true) => brief::build_subtask_plan_brief(&request_text, &task.variant()),
                (false, true) => {
                    brief::build_subtask_plan_retry_brief(&request_text, &task.variant())
                }
            };
            let file_stem = if retry == 0 { "" } else { "-retry" };
            let _ = std::fs::write(run_dir.join(format!("brief{file_stem}.md")), &brief_text);
            let events_path = run_dir.join(format!("events{file_stem}.jsonl"));
            let run_result = run_harness(
                app,
                task_id,
                attempt_n,
                true,
                &worktree,
                &req,
                &brief_text,
                &events_path,
                cancel,
                None,
            )
            .await;
            if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                task = reloaded;
            }
            match run_result {
                Ok(o) => {
                    if let Some(cost) = o.cost_usd {
                        let a = &mut task.attempts[idx];
                        a.cost_usd = Some(a.cost_usd.unwrap_or(0.0) + cost);
                        a.cost_estimated |= o.cost_estimated;
                        task.cost_usd += cost;
                    }
                    let usage = task.attempts[idx].usage.get_or_insert(Usage {
                        input: 0,
                        output: 0,
                        cached: 0,
                    });
                    usage.input += o.usage_input;
                    usage.output += o.usage_output;
                    usage.cached += o.usage_cached;
                    // A harness-reported failure (Claude `is_error`, Codex
                    // `turn.failed`) is an infra/tooling problem, not an
                    // ambiguous-request problem -- fail outright rather than
                    // spend a clarification round asking the owner "more
                    // detail" about something they can't fix by replying.
                    if let Some(err) = o.error {
                        record_failure(&mut task, idx, FailureKind::Error, err);
                        task.status = TaskStatus::Failed;
                        task.updated_at = now_ms();
                        let _ = app.store.save_task(&task);
                        app.broadcast_task(&task);
                        hard_failure = true;
                        break;
                    }
                    let text = o.final_text.unwrap_or_default();
                    if let Some(d) = brief::parse_plan(&text) {
                        draft = Some(d);
                        break;
                    }
                }
                Err(RunError::Cancelled) => {
                    let _ = std::fs::remove_file(&key_path);
                    return end_plan_stage(app, task_id, Some(idx)).await;
                }
                Err(RunError::Io(msg)) => {
                    record_failure(&mut task, idx, FailureKind::Error, msg);
                    task.status = TaskStatus::Failed;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                    hard_failure = true;
                    break;
                }
            }
        }
        // Delete the per-run key file the moment the run(s) end, same as
        // the implement path; recovery also deletes it for an attempt
        // interrupted by an unclean shutdown.
        let _ = std::fs::remove_file(&key_path);
        if hard_failure {
            return end_plan_stage(app, task_id, Some(idx)).await;
        }
        if over_budget {
            // Not a clarification round: the owner is asked about the
            // budget, not for more detail, and "raise" drafts again.
            record_failure(
                &mut task,
                idx,
                FailureKind::NoDeliverable,
                "planner did not return a parseable sushi-plan block; the budget stopped the retry"
                    .to_string(),
            );
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
            continue;
        }

        let Some(draft) = draft else {
            record_failure(
                &mut task,
                idx,
                FailureKind::NoDeliverable,
                "planner did not return a parseable sushi-plan block".to_string(),
            );
            if clarify_rounds >= 2 {
                // Already asked the owner twice for more detail; a third
                // unparseable round in a row means this isn't going anywhere.
                task.status = TaskStatus::Failed;
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
                return end_plan_stage(app, task_id, Some(idx)).await;
            }
            clarify_rounds += 1;
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
            let question =
                "The planner could not draft this task: edit it or answer with more detail";
            match ask_plan_question(
                app,
                task_id,
                question,
                vec![],
                pending_answer,
                cancel,
                permit,
            )
            .await
            {
                Some(answer) => {
                    if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                        let combined = format!(
                            "{}\n\n(Owner clarification: {answer})",
                            t.request.clone().unwrap_or_default()
                        );
                        t.request = Some(combined);
                        t.status = TaskStatus::Drafting;
                        t.updated_at = now_ms();
                        let _ = app.store.save_task(&t);
                        app.broadcast_task(&t);
                    }
                    continue;
                }
                None => return end_plan_stage(app, task_id, Some(idx)).await,
            }
        };

        task.title = draft.title.clone();
        task.goal = draft.goal.clone();
        task.planned_tier = draft.tier;
        // With review off nobody would check a moved entry: keep it as is.
        let (verify, judged) = if settings.review.is_empty() {
            (draft.verify.clone(), Vec::new())
        } else {
            split_verify_commands(&worktree, &draft.verify)
        };
        task.criteria = draft.criteria.clone();
        task.criteria.extend(
            judged
                .into_iter()
                .map(|v| format!("Checked by review (not a shell command): {v}")),
        );
        task.verify = verify;
        task.final_verify = draft
            .final_verify
            .iter()
            .filter(|c| !c.trim().is_empty())
            .cloned()
            .collect();
        task.attempts[idx].status = AttemptStatus::Passed;
        task.attempts[idx].ended_at = Some(now_ms());
        task.attempts[idx].summary = Some(format!("Drafted: {}", draft.title));
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);

        // Classifier preflight on the draft (spec step 4), same three
        // checks as `task.preflight`, journaled under its own point --
        // still run for its goal/criteria signal even though it no longer
        // gates the verify question (that's unconditional on
        // `verify.is_empty()` now, review item P2e).
        let classifier_settings = settings.classifier.clone();
        let key = app.secrets.read().unwrap().classifier_key.clone();
        let base_url = app.secrets.read().unwrap().classifier_base_url.clone();
        if classifier_settings.backend != ClassifierBackend::None && key.is_some() {
            let state =
                json!({"goal": task.goal, "criteria": task.criteria, "verify": task.verify});
            let questions = vec![
                classify::QuestionSpec::Noul {
                    name: "goal_specific".to_string(),
                    prompt: "Is the goal specific enough to act on without asking?".to_string(),
                },
                classify::QuestionSpec::Noul {
                    name: "criteria_checkable".to_string(),
                    prompt: "Can each acceptance criterion be checked objectively from outside?"
                        .to_string(),
                },
                classify::QuestionSpec::Noul {
                    name: "has_verification".to_string(),
                    prompt: "Do the verification commands actually exercise the criteria?"
                        .to_string(),
                },
            ];
            let start = std::time::Instant::now();
            let s2 = classifier_settings.clone();
            let k2 = key.clone();
            let b2 = base_url.clone();
            let q2 = questions.clone();
            let state2 = state.clone();
            let result = tokio::task::spawn_blocking(move || {
                classify::decide(&s2, k2.as_deref(), b2.as_deref(), &state2, &q2)
            })
            .await
            .unwrap_or_else(|e| Err(classify::ClassifyError(e.to_string())));
            app.journal(task_id, "plan_preflight", &result, start.elapsed());
            if let Ok(answers) = &result {
                if let (Some(g), Some(c), Some(v)) = (
                    answers.get("goal_specific").and_then(|a| a.noul),
                    answers.get("criteria_checkable").and_then(|a| a.noul),
                    answers.get("has_verification").and_then(|a| a.noul),
                ) {
                    app.append_jev_decision(task_id, jev_plan_preflight_line(g, c, v));
                }
            }
        }

        for q in draft.questions.iter().take(3) {
            // P2: reload before each question -- an earlier question in
            // this same loop may have gone to the owner and back (or been
            // triage-answered) since `task` was last captured, and triage's
            // brief for this one must see that, not a stale snapshot.
            if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                task = reloaded;
            }
            match ask_plan_question_with_triage(
                app,
                task_id,
                &mut task,
                attempt_n,
                &worktree,
                &q.text,
                q.options.clone(),
                pending_answer,
                cancel,
                permit,
            )
            .await
            {
                Some(_) => {}
                None => return end_plan_stage(app, task_id, Some(idx)).await,
            }
        }

        // The planner split the request: the task becomes a parent of one
        // child per part and never implements anything itself.
        if !draft.subtasks.is_empty() && task.parent.is_none() {
            let split = match check_subtasks(&draft.subtasks) {
                Ok(()) => split_into_subtasks(app, task_id, &draft.subtasks, auto_start_after_plan)
                    .await
                    .map_err(|e| format!("could not create the subtasks: {e}")),
                Err(reason) => Err(format!("subtasks ignored: {reason}")),
            };
            match split {
                Ok(()) => {
                    return if auto_start_after_plan {
                        PlanOutcome::Proceed
                    } else {
                        PlanOutcome::StoppedForReview
                    };
                }
                Err(why) => {
                    app.append_jev_decision(
                        task_id,
                        format!("Planner: {why}; running as one task"),
                    );
                    if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                        task = reloaded;
                    }
                }
            }
        }

        // Unconditional on an empty `verify` now (review item P2e): a
        // classifier that's off, missing a key, or simply unsure is no
        // reason to skip asking outright.
        if task.verify.is_empty() {
            let options = verify_options_from_package_json(&worktree);
            match ask_plan_question(
                app,
                task_id,
                "No verification command was found. Which command proves this task?",
                options,
                pending_answer,
                cancel,
                permit,
            )
            .await
            {
                Some(answer) => {
                    if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                        t.verify = vec![answer];
                        t.updated_at = now_ms();
                        let _ = app.store.save_task(&t);
                        app.broadcast_task(&t);
                    }
                }
                None => return end_plan_stage(app, task_id, Some(idx)).await,
            }
        }

        if let Ok(Some(mut t)) = app.store.load_task(task_id) {
            t.status = if auto_start_after_plan {
                TaskStatus::Queued
            } else {
                TaskStatus::Stopped
            };
            t.updated_at = now_ms();
            let _ = app.store.save_task(&t);
            app.broadcast_task(&t);
        }
        return if auto_start_after_plan {
            PlanOutcome::Proceed
        } else {
            PlanOutcome::StoppedForReview
        };
    }
}

/// Turns a drafted task into a parent: one child per part, each drafted by
/// its own planner, branching from the parent's branch, with `dependsOn`
/// mapped from the parts' keys. The parent is left `running` (its loop then
/// starts the children) or `stopped` for the owner to review first.
async fn split_into_subtasks(
    app: &Arc<App>,
    parent_id: &str,
    parts: &[brief::PlanSubtask],
    start: bool,
) -> Result<(), String> {
    let mut parent = app
        .store
        .load_task(parent_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    let ids: HashMap<&str, String> = parts
        .iter()
        .map(|p| (p.key.trim(), uuid::Uuid::new_v4().to_string()))
        .collect();
    let parent_mcp = std::fs::read_to_string(app.store.task_dir(parent_id).join("mcp.json")).ok();
    let titles_by_key: HashMap<&str, String> = parts
        .iter()
        .map(|p| {
            let title = if p.title.trim().is_empty() {
                truncate_chars(p.request.trim(), 60)
            } else {
                p.title.trim().to_string()
            };
            (p.key.trim(), title)
        })
        .collect();
    let now = now_ms();
    let mut titles = Vec::new();
    let mut children: Vec<Task> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let title = titles_by_key[part.key.trim()].clone();
        let created = app
            .create_task_record(NewTask {
                id: ids[part.key.trim()].clone(),
                repo_root: PathBuf::from(&parent.repo),
                title: title.clone(),
                goal: String::new(),
                criteria: vec![],
                verify: vec![],
                final_verify: vec![],
                request: Some(subtask_request(&parent, &part.request)),
                branch: None,
                base: parent.branch.clone(),
                variant: parent.variant(),
                depends_on: part
                    .depends_on
                    .iter()
                    .map(|k| ids[k.trim()].clone())
                    .collect(),
                parent: Some(parent.id.clone()),
                // Eval reports count the parent, which carries the children's
                // cost.
                eval_set: None,
                eval_name: None,
                // Keeps the planner's order when children are listed by age.
                created_at: now + i as i64,
            })
            .await;
        let child = match created {
            Ok(child) => child,
            Err(e) => {
                // All parts or none: a partial split would leave the parent
                // waiting on children that cover only part of the plan.
                discard_children(app, children).await;
                return Err(e);
            }
        };
        if let Some(mcp) = &parent_mcp {
            let _ = std::fs::write(app.store.task_dir(&child.id).join("mcp.json"), mcp);
        }
        titles.push(if part.depends_on.is_empty() {
            title
        } else {
            let after: Vec<&str> = part
                .depends_on
                .iter()
                .map(|k| titles_by_key[k.trim()].as_str())
                .collect();
            format!("{title} (after {})", after.join(", "))
        });
        children.push(child);
    }
    parent.decisions.push(format!(
        "Planner: split into {} subtasks: {}",
        parts.len(),
        titles.join("; ")
    ));
    parent.status = if start {
        TaskStatus::Running
    } else {
        TaskStatus::Stopped
    };
    parent.updated_at = now_ms();
    if let Err(e) = app.store.save_task(&parent) {
        discard_children(app, children).await;
        return Err(e.to_string());
    }
    for child in &children {
        app.broadcast_task(child);
    }
    app.broadcast_task(&parent);
    Ok(())
}

/// Undoes a split that could not create every part: removes each child
/// already created (its record, worktree and branch). None has a loop or
/// has been broadcast yet, so nothing else has seen them.
async fn discard_children(app: &Arc<App>, children: Vec<Task>) {
    for child in children {
        let _ = std::fs::remove_dir_all(app.store.task_dir(&child.id));
        let _ = tokio::task::spawn_blocking(move || {
            git::discard_worktree(
                Path::new(&child.repo),
                Path::new(&child.worktree),
                &child.branch,
            )
        })
        .await;
    }
}

// ===========================================================================
// Task graph: parents and landing
// ===========================================================================

/// A parent's loop: starts its children that have not started yet, and once
/// every child has landed runs its own checks on its branch and ends `done`
/// (`failed` when a check fails). Until then it leaves the task `running`
/// with no loop; `advance_graph` comes back to it.
async fn run_parent(app: &Arc<App>, task_id: &str, cancel: &CancelToken) {
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return;
    };
    let all = app.repo_tasks(&task.repo);
    // No child starts before the parent's own dependencies are done.
    match own_waits_state(&task, &all) {
        Waits::Ready | Waits::Nothing => {}
        Waits::Ended(ended) => {
            app.ask_dependency_question(&task, &all, &ended);
            return;
        }
        Waits::Pending => {
            if task.status != TaskStatus::Queued {
                task.status = TaskStatus::Queued;
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
            }
            return;
        }
    }
    // `advance_graph` only relaunches a queued or running parent, so a
    // stopped or failed one here is the owner's own restart: its unfinished
    // children restart with it.
    let restarted = matches!(task.status, TaskStatus::Stopped | TaskStatus::Failed);
    for child in all
        .iter()
        .filter(|t| t.parent.as_deref() == Some(task_id) && !t.archived)
    {
        match child.status {
            TaskStatus::Drafting | TaskStatus::Queued => {
                app.spawn_task_loop(child.id.clone(), true);
            }
            TaskStatus::Stopped | TaskStatus::Failed if restarted => {
                // Queued before its loop runs, so this parent never reads
                // it as ended.
                let mut child = child.clone();
                child.question = None;
                child.status = TaskStatus::Queued;
                child.updated_at = now_ms();
                let _ = app.store.save_task(&child);
                app.broadcast_task(&child);
                app.spawn_task_loop(child.id.clone(), true);
            }
            _ => {}
        }
    }
    let all = if restarted {
        app.repo_tasks(&task.repo)
    } else {
        all
    };
    if !matches!(waits_state(&task, &all), Waits::Ready | Waits::Nothing) {
        if task.status != TaskStatus::Running {
            task.status = TaskStatus::Running;
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
        }
        return;
    }

    let permit = tokio::select! {
        _ = cancel.cancelled() => None,
        p = app.slots.clone().acquire_owned() => p.ok(),
    };
    if permit.is_none() {
        mark_stopped_if_not_already(app, task_id).await;
        return;
    }
    task.status = TaskStatus::Running;
    task.updated_at = now_ms();
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);

    let checks: Vec<String> = task
        .verify
        .iter()
        .chain(&task.final_verify)
        .cloned()
        .collect();
    let sandbox = app.settings.read().unwrap().sandbox;
    let results = run_verify_commands(
        Path::new(&task.worktree),
        &app.store.task_dir(task_id).join("runs").join("parent"),
        &checks,
        sandbox,
        cancel,
    )
    .await;
    if cancel.is_cancelled() {
        mark_stopped_if_not_already(app, task_id).await;
        return;
    }
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return;
    };
    task.cost_usd = parent_cost(&task, &app.repo_tasks(&task.repo));
    match results.iter().find(|v| v.code != Some(0)) {
        Some(failed) => {
            task.decisions.push(format!(
                "Orchestrator: every subtask landed, but `{}` exited {} on {}: {}",
                failed.command,
                failed
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                task.branch,
                tail_chars(failed.tail.trim(), 600)
            ));
            task.status = TaskStatus::Failed;
        }
        None => {
            task.decisions.push(format!(
                "Orchestrator: every subtask landed on {}",
                task.branch
            ));
            task.status = TaskStatus::Done;
        }
    }
    task.updated_at = now_ms();
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);
}

/// The carry-onto-moved-base failure detail the agent gets: which files
/// conflict and how to resolve them.
fn conflict_detail(base_ref: &str, new_sha: &str, files: &[String], task_id: &str) -> String {
    format!(
        "{base_ref} moved ahead to {}, and your changes were carried onto it. \
         These files conflict: {}. Text files carry <<<<<<< / >>>>>>> markers; \
         for a binary or deleted file, your version is `git show {}:<path>`. \
         Resolve each one so both the base's change and yours survive, then finish the task.",
        short_sha(new_sha),
        files.join(", "),
        git::wip_ref(task_id)
    )
}

enum Landing {
    /// Committed and fast-forwarded onto the parent's branch (or the parent
    /// already held the work).
    Landed,
    /// Back to the agent as an ordinary failure; it retries on the new base.
    Failed {
        kind: FailureKind,
        detail: String,
    },
    Cancelled,
    /// The parent is gone: commit like a task without one.
    NoParent,
}

/// Lands a finished child on its parent's branch, one child per parent at a
/// time: its work is carried onto the parent's current head (earlier
/// siblings may have landed since it started), verify runs again when the
/// head moved, then the child commits and the parent's branch
/// fast-forwards to that commit.
#[allow(clippy::too_many_arguments)]
async fn land_on_parent(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    attempt_n: u32,
    worktree: &Path,
    run_dir: &Path,
    parent_id: &str,
    cancel: &CancelToken,
) -> Landing {
    let lock = app.landing_lock(parent_id);
    let _landing = tokio::select! {
        _ = cancel.cancelled() => return Landing::Cancelled,
        guard = lock.lock_owned() => guard,
    };
    let Ok(Some(parent)) = app.store.load_task(parent_id) else {
        return Landing::NoParent;
    };
    let branch = parent.branch.clone();
    let failed = |kind, detail| Landing::Failed { kind, detail };

    let (wt, base, tid, b) = (
        worktree.to_path_buf(),
        task.base_sha.clone(),
        task.id.clone(),
        branch.clone(),
    );
    let carried = tokio::task::spawn_blocking(move || {
        // Commits the agent made itself go back to being uncommitted work,
        // so they are carried (and later committed) like the rest.
        if git::head_sha(&wt)? != base {
            git::uncommit_to(&wt, &base)?;
        }
        git::carry_onto_moved_base(&wt, &b, &base, &tid)
    })
    .await
    .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
    match carried {
        Ok(git::Rebase::Unchanged) => {}
        Ok(git::Rebase::Moved { new_sha }) => {
            task.decisions.push(format!(
                "Land: carried the work onto {branch} at {}",
                short_sha(&new_sha)
            ));
            task.base_sha = new_sha.clone();
            let (wt, b) = (worktree.to_path_buf(), new_sha.clone());
            let changed = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &b).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            task.attempts[idx].changed_files = changed.clone();
            if changed.is_empty() {
                task.decisions
                    .push(format!("Land: {branch} already contains this work"));
                return Landing::Landed;
            }
            let results = run_verify_cached(
                app,
                &task.id,
                worktree,
                run_dir,
                &new_sha,
                &task.verify,
                cancel,
            )
            .await;
            if cancel.is_cancelled() {
                return Landing::Cancelled;
            }
            task.attempts[idx].verify = results.clone();
            if let Some(v) = results.iter().find(|v| v.code != Some(0)) {
                return failed(
                    FailureKind::Verify,
                    format!(
                        "Another subtask landed on {branch} first; carried onto it, {} exited {}.\n{}",
                        v.command,
                        v.code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "null".to_string()),
                        v.tail
                    ),
                );
            }
        }
        Ok(git::Rebase::Conflicts { new_sha, files }) => {
            task.base_sha = new_sha.clone();
            return failed(
                FailureKind::Verify,
                conflict_detail(&branch, &new_sha, &files, &task.id),
            );
        }
        Ok(git::Rebase::Skipped { reason }) => {
            return failed(
                FailureKind::Error,
                format!("Could not carry the work onto {branch} ({reason})."),
            );
        }
        Err(e) => {
            return failed(
                FailureKind::Error,
                format!("Could not carry the work onto {branch} ({e})."),
            );
        }
    }

    let (wt, parent_wt, title, tid, base) = (
        worktree.to_path_buf(),
        PathBuf::from(&parent.worktree),
        task.title.clone(),
        task.id.clone(),
        task.base_sha.clone(),
    );
    let landed = tokio::task::spawn_blocking(move || {
        git::commit(&wt, &title, &tid, attempt_n)?;
        let sha = git::head_sha(&wt)?;
        if let Err(e) = git::fast_forward(&parent_wt, &sha) {
            // Keep the work, uncommitted, for the next attempt to carry.
            let _ = git::uncommit_to(&wt, &base);
            return Err(e);
        }
        Ok(sha)
    })
    .await
    .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
    match landed {
        Ok(sha) => {
            task.decisions
                .push(format!("Land: landed on {branch} at {}", short_sha(&sha)));
            Landing::Landed
        }
        Err(e) => failed(
            FailureKind::Error,
            format!("Could not land on {branch}: {e}"),
        ),
    }
}

// ===========================================================================
// The attempt loop
// ===========================================================================

/// The per-task attempt loop (spec "Engine", steps 1-10). Runs as its own
/// tokio task from `task.start` until the task reaches `done`/`failed`, is
/// stopped, or the daemon shuts down; `waiting` parks it on `pending_answer`
/// rather than exiting, so the in-memory attempt-budget extension (step 9)
/// survives across a wait/answer cycle.
async fn run_task_loop(
    app: Arc<App>,
    task_id: String,
    pending_answer: Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: CancelToken,
    auto_start_after_plan: bool,
) {
    let mut attempt_budget = app.settings.read().unwrap().max_attempts;
    // Never resume immediately after a waiting/answer cycle -- a fresh
    // brief carries the owner's answer instead (spec item 11). Reset each
    // iteration; set back to `true` only when this iteration itself ends
    // via an answered wait.
    let mut just_answered = false;

    loop {
        if cancel.is_cancelled() {
            mark_stopped_if_not_already(&app, &task_id).await;
            app.finish_task_loop(&task_id);
            return;
        }

        let mut task = match app.store.load_task(&task_id) {
            Ok(Some(t)) => t,
            _ => {
                app.finish_task_loop(&task_id);
                return;
            }
        };

        // The task graph: a parent never implements, and a task whose
        // dependencies are not all done waits `queued` with no loop until
        // `advance_graph` starts it again. Drafting runs meanwhile.
        if !needs_planning(&task) {
            let all = app.repo_tasks(&task.repo);
            if is_parent(&task, &all) {
                run_parent(&app, &task_id, &cancel).await;
                app.finish_task_loop(&task_id);
                return;
            }
            let parent_blocked = task
                .parent
                .as_deref()
                .and_then(|pid| all.iter().find(|t| t.id == pid))
                .is_some_and(|p| {
                    !matches!(own_waits_state(p, &all), Waits::Ready | Waits::Nothing)
                });
            if parent_blocked
                || (!task.depends_on.is_empty()
                    && !matches!(waits_state(&task, &all), Waits::Ready | Waits::Nothing))
            {
                if task.status != TaskStatus::Queued {
                    task.status = TaskStatus::Queued;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                }
                app.finish_task_loop(&task_id);
                return;
            }
        }

        // Concurrency limit: stays `queued` while waiting for a slot, and
        // `task.stop` (via `cancel`) works here too.
        let mut permit = tokio::select! {
            _ = cancel.cancelled() => {
                mark_stopped_if_not_already(&app, &task_id).await;
                app.finish_task_loop(&task_id);
                return;
            }
            permit = app.slots.clone().acquire_owned() => {
                match permit {
                    Ok(p) => Some(p),
                    Err(_) => { app.finish_task_loop(&task_id); return; }
                }
            }
        };

        if needs_planning(&task) {
            match run_plan_stage(
                &app,
                &task_id,
                &pending_answer,
                &cancel,
                &mut permit,
                auto_start_after_plan,
            )
            .await
            {
                PlanOutcome::Proceed => {
                    drop(permit);
                    continue;
                }
                PlanOutcome::StoppedForReview | PlanOutcome::Ended => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        // A child may have been attached while this task waited for a slot:
        // it must never implement the whole request itself then.
        if implement_attempt_count(&task) == 0 {
            let all = app.repo_tasks(&task.repo);
            if is_parent(&task, &all) {
                drop(permit);
                run_parent(&app, &task_id, &cancel).await;
                app.finish_task_loop(&task_id);
                return;
            }
        }

        let resume_eligible = !just_answered;
        just_answered = false;

        let status = task.status;
        if !wait_while_over_budget(
            &app,
            &task_id,
            &mut task,
            "implement",
            status,
            &pending_answer,
            &cancel,
            &mut permit,
        )
        .await
        {
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }

        // A task in a graph starts from its base branch as it is now, so a
        // dependent begins on top of the work it waited for (a subtask on
        // its parent's head, with every earlier sibling landed).
        if implement_attempt_count(&task) == 0
            && (task.parent.is_some() || !task.depends_on.is_empty())
        {
            if let Some(base_ref) = task.base_ref.clone() {
                let (wt, from, tid, r) = (
                    PathBuf::from(&task.worktree),
                    task.base_sha.clone(),
                    task_id.clone(),
                    base_ref.clone(),
                );
                let synced = tokio::task::spawn_blocking(move || {
                    git::carry_onto_moved_base(&wt, &r, &from, &tid)
                })
                .await;
                if let Ok(Ok(git::Rebase::Moved { new_sha })) = synced {
                    task.decisions.push(format!(
                        "Rebase: started from {base_ref} at {}",
                        short_sha(&new_sha)
                    ));
                    task.base_sha = new_sha;
                }
            }
        }

        let mut jev_tier_choice = None;
        let mut planner_tier_used = false;
        let mut tier_fallback_reason = None;
        if implement_attempt_count(&task) == 0 {
            match task.planned_tier.filter(|_| task.variant().planner_tier) {
                Some(tier) => {
                    task.tier = tier;
                    task.tier_fallback = None;
                    planner_tier_used = true;
                }
                None => match classify_tier(&app, &task).await {
                    TierPick::Classified { tier, choice, p } => {
                        task.tier = tier;
                        task.tier_fallback = None;
                        jev_tier_choice = Some((choice, p));
                    }
                    TierPick::Fallback { reason } => {
                        task.tier = Tier::Standard;
                        task.tier_fallback = Some(reason.clone());
                        tier_fallback_reason = Some(reason);
                    }
                },
            }
        }

        let settings = app.settings.read().unwrap().clone();
        let attempt_n = implement_attempt_count(&task) + 1;
        let (route_id, overridden) = task.variant().implement_route_id(&settings, task.tier);
        if overridden {
            let line = variant_route_line(&format!("tier {}", task.tier.as_str()), &route_id);
            if !task.decisions.contains(&line) {
                task.decisions.push(line);
            }
        }
        if let Some((choice, p)) = jev_tier_choice {
            task.decisions.push(jev_tier_line(&choice, p, &route_id));
        }
        if planner_tier_used {
            task.decisions.push(format!(
                "Planner: tier {} -> route {route_id}",
                task.tier.as_str()
            ));
        }
        if let Some(reason) = tier_fallback_reason {
            task.decisions
                .push(jev_tier_fallback_line(&reason, &route_id));
        }
        let route = settings
            .routes
            .iter()
            .find(|r| r.id == route_id)
            .cloned()
            .unwrap_or(Route {
                id: "codex".to_string(),
                label: "Codex".to_string(),
                harness: Harness::Codex,
                model: None,
                effort: None,
                profile_id: None,
            });

        // The plan attempt (if any) is never a resume candidate -- it's a
        // different route/harness shape entirely, and has nothing to do
        // with the implement route's own session history.
        let prev = task
            .attempts
            .iter()
            .rev()
            .find(|a| a.stage == Stage::Implement)
            .cloned();
        let resume_session = if resume_eligible && task.variant().retry_mode == RetryMode::Resume {
            prev.as_ref().and_then(|p| {
                let route_matches = p.route_id == route.id;
                let has_session = p.session_id.is_some();
                let verify_failure =
                    p.failure.as_ref().map(|f| f.kind) == Some(FailureKind::Verify);
                let was_interrupted = p.status == AttemptStatus::Interrupted;
                if route_matches && has_session && (verify_failure || was_interrupted) {
                    p.session_id.clone()
                } else {
                    None
                }
            })
        } else {
            None
        };

        let worktree = PathBuf::from(&task.worktree);
        let base_sha = task.base_sha.clone();
        let wt2 = worktree.clone();
        let base2 = base_sha.clone();
        let (status_short, diff_stat) = tokio::task::spawn_blocking(move || {
            (
                git::status_short(&wt2).unwrap_or_default(),
                git::diff_stat(&wt2, &base2).unwrap_or_default(),
            )
        })
        .await
        .unwrap_or_default();

        if task.variant().advisor {
            if let Some(cost) = run_advisor_before_retry(
                &app, &mut task, &settings, &route, &worktree, &base_sha, &cancel,
            )
            .await
            {
                task.cost_usd += cost;
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
                // The advisor's spend can be what reaches the budget.
                let status = task.status;
                if !wait_while_over_budget(
                    &app,
                    &task_id,
                    &mut task,
                    "implement",
                    status,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        let brief_text = match (
            &resume_session,
            prev.as_ref().and_then(|p| p.failure.as_ref()),
        ) {
            (Some(_), Some(failure)) => brief::build_resume_delta(failure),
            _ => brief::build_brief(&task, &status_short, &diff_stat),
        };
        // The messages sent to this task since its last attempt are
        // delivered here, with the attempt about to start.
        let brief_text = brief::with_block_before_report(
            &brief_text,
            &messages::brief_block_for_task(&app, &task_id),
        );
        let advice = task
            .attempts
            .iter()
            .rev()
            .find(|a| a.stage == Stage::Implement)
            .and_then(|a| a.advice.clone());
        let brief_text = match advice.as_deref() {
            Some(advice) if task.variant().advisor => {
                brief::with_block_before_report(&brief_text, &brief::advisor_block(advice))
            }
            _ => brief_text,
        };

        let reason = format!("tier {} -> route {}", task.tier.as_str(), route.id);
        let attempt = Attempt {
            n: attempt_n,
            stage: Stage::Implement,
            route_id: route.id.clone(),
            harness: route.harness,
            model: route.model.clone().unwrap_or_default(),
            reason,
            session_id: None,
            pgid: None,
            resumed: resume_session.is_some(),
            started_at: now_ms(),
            ended_at: None,
            status: AttemptStatus::Running,
            summary: None,
            handoff: None,
            changed_files: vec![],
            verify: vec![],
            gate_blocks: 0,
            prefix_tokens: None,
            review: None,
            failure: None,
            usage: None,
            cost_usd: None,
            cost_estimated: false,
            review_cost_usd: None,
            advice: None,
            advisor_cost_usd: None,
        };
        task.attempts.push(attempt);
        let idx = task.attempts.len() - 1;
        task.status = TaskStatus::Running;
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);

        let run_dir = app.store.run_dir(&task_id, attempt_n);
        let _ = std::fs::create_dir_all(&run_dir);
        let mcp_path = run_dir.join("mcp.json");
        let task_mcp_path = app.store.task_dir(&task_id).join("mcp.json");
        // The project's own servers, plus orchd's messaging bridge scoped to
        // this task.
        let messages_server = messages::task_server(&app, &task_id);
        let mut mcp_config = std::fs::read_to_string(&task_mcp_path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .filter(|v| v.get("mcpServers").is_some_and(|s| s.is_object()))
            .unwrap_or_else(|| json!({"mcpServers": {}}));
        mcp_config["mcpServers"][messages::SERVER] = messages_server.clone();
        let _ = store::write_json_atomic(&mcp_path, &mcp_config);

        let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
        let token = uuid::Uuid::new_v4().to_string();
        let settings_path = run_dir.join("settings.json");
        let key_path = run_dir.join("key");
        // Keep our own clone of the hook context alongside the one handed
        // to `hook.stop` lookups, so we can read back how many times it
        // blocked into `attempt.gateBlocks` once the run ends.
        let mut registered: Option<(String, Arc<HookContext>)> = None;
        if matches!(route.harness, Harness::Claude) {
            let profile = route
                .profile_id
                .as_ref()
                .and_then(|pid| app.secrets.read().unwrap().profiles.get(pid).cloned());
            let mut profile_obj = serde_json::Map::new();
            if let Some(p) = &profile {
                if !p.env.is_empty() {
                    profile_obj.insert(
                        "env".to_string(),
                        serde_json::to_value(&p.env).unwrap_or(serde_json::Value::Null),
                    );
                }
                if let Some(key) = &p.key {
                    if store::write_secret_file(&key_path, key).is_ok() {
                        profile_obj.insert(
                            "apiKeyHelper".to_string(),
                            serde_json::Value::String(format!(
                                "cat {}",
                                harness::shell_quote(&key_path.to_string_lossy())
                            )),
                        );
                    }
                }
            }
            let profile_value = if profile_obj.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(profile_obj))
            };

            let socket_path_str = app.socket_path.to_string_lossy().to_string();
            let stop_hook = harness::StopHook {
                orchd_path: &app.orchd_path,
                socket_path: &socket_path_str,
                token: &token,
                lean_output: task.variant().lean_output,
            };
            let mut claude_settings = harness::build_claude_settings(
                profile_value.as_ref(),
                settings.sandbox,
                &settings.allowed_domains,
                &deny_read,
                Some(stop_hook),
            );
            // A headless run answers no prompts, so the messaging tools must
            // be allowed up front to be usable at all.
            if let Some(allow) = claude_settings["permissions"]["allow"].as_array_mut() {
                for tool in crate::mcp::TASK_TOOLS {
                    allow.push(json!(format!("mcp__{}__{tool}", messages::SERVER)));
                }
            }
            let _ = store::write_json_atomic(&settings_path, &claude_settings);
            let ctx = Arc::new(HookContext {
                task_id: task_id.clone(),
                attempt_n,
                worktree: worktree.clone(),
                base_sha: base_sha.clone(),
                verify: task.verify.clone(),
                blocks: AtomicU32::new(0),
                cancel: CancelToken::new(),
                hook_running: Arc::new(AtomicBool::new(false)),
            });
            app.hook_tokens
                .write()
                .unwrap()
                .insert(token.clone(), ctx.clone());
            registered = Some((token.clone(), ctx));
        }

        let network_allowed = settings.codex_network;
        let req = harness::RunRequest {
            harness: route.harness,
            worktree: &worktree,
            model: route.model.as_deref(),
            effort: route.effort.as_deref(),
            resume: resume_session.as_deref(),
            review: false,
            mcp_config: Some(&mcp_path),
            settings_path: Some(&settings_path),
            network_allowed,
            codex_mcp: Some((messages::SERVER, &messages_server)),
            images: &[],
            repo_settings: true,
        };

        let _ = std::fs::write(run_dir.join("brief.md"), &brief_text);
        let events_path = run_dir.join("events.jsonl");
        let run_result = run_harness(
            &app,
            &task_id,
            attempt_n,
            true,
            &worktree,
            &req,
            &brief_text,
            &events_path,
            &cancel,
            (task.variant().stall_timeout_secs > 0).then(|| Stall {
                limit: Duration::from_secs(task.variant().stall_timeout_secs),
                paused: registered
                    .as_ref()
                    .map(|(_, ctx)| ctx.hook_running.clone())
                    .unwrap_or_default(),
            }),
        )
        .await;

        let gate_blocks = if let Some((tok, ctx)) = registered.take() {
            app.hook_tokens.write().unwrap().remove(&tok);
            ctx.cancel.cancel();
            ctx.blocks.load(Ordering::SeqCst)
        } else {
            0
        };
        // Delete the per-run key file the moment the run ends (spec item
        // B); recovery also deletes it for an attempt interrupted by an
        // unclean shutdown.
        let _ = std::fs::remove_file(&key_path);

        // `run_harness` may have persisted `session_id`/`pgid` mid-run;
        // reload so we don't clobber that with our stale in-memory copy --
        // then reapply `gate_blocks`, which is never itself persisted
        // mid-run and so isn't on the reloaded copy at all.
        if let Ok(Some(reloaded)) = app.store.load_task(&task_id) {
            task = reloaded;
        }
        task.attempts[idx].gate_blocks = gate_blocks;

        let outcome = match run_result {
            Ok(o) => o,
            Err(RunError::Cancelled) => {
                task.attempts[idx].status = AttemptStatus::Interrupted;
                task.attempts[idx].ended_at = Some(now_ms());
                settle_unfinished_cost(&mut task, idx, &run_dir, &settings.prices);
                task.status = TaskStatus::Stopped;
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
                drop(permit);
                app.finish_task_loop(&task_id);
                return;
            }
            Err(RunError::Io(msg)) => {
                match fail_and_continue(
                    &app,
                    &task_id,
                    &mut task,
                    idx,
                    FailureKind::Error,
                    msg,
                    &mut attempt_budget,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                {
                    LoopSignal::Continue { answered } => {
                        just_answered = answered;
                        drop(permit);
                        continue;
                    }
                    LoopSignal::Stop => {
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                }
            }
        };

        task.attempts[idx].session_id = outcome.session_id.clone();
        task.attempts[idx].usage = Some(Usage {
            input: outcome.usage_input,
            output: outcome.usage_output,
            cached: outcome.usage_cached,
        });
        // A resumed first turn also carries the whole earlier conversation.
        if route.harness == Harness::Claude && resume_session.is_none() {
            task.attempts[idx].prefix_tokens = outcome.first_turn_tokens;
        }
        let cost = outcome.cost_usd.map(|total| {
            attempt_cost(
                &task.attempts[..idx],
                &task.attempts[idx],
                total,
                outcome.cost_estimated,
            )
        });
        task.attempts[idx].cost_usd = cost;
        task.attempts[idx].cost_estimated = outcome.cost_estimated;
        if let Some(cost) = cost {
            task.cost_usd += cost;
        }

        if outcome.stalled {
            let detail = outcome.error.clone().unwrap_or_default();
            let (wt, base) = (worktree.clone(), base_sha.clone());
            task.attempts[idx].changed_files = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &base).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                FailureKind::Stall,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue { answered } => {
                    just_answered = answered;
                    drop(permit);
                    continue;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        let final_text = outcome.final_text.clone().unwrap_or_default();
        let report = brief::parse_report(&final_text);
        // Not a gate: the reviewer weighs it. A `partial` or missing report,
        // or a harness that errored after editing files, is exactly what a
        // reviewer should look at harder.
        let implementer_note = format!(
            "Implementer report: {}{}",
            match report.as_ref().map(|r| &r.outcome) {
                Some(brief::Outcome::Complete) => "complete",
                Some(brief::Outcome::Partial) => "partial",
                Some(brief::Outcome::Blocked) => "blocked",
                None => "missing (no parseable sushi-report)",
            },
            outcome
                .error
                .as_deref()
                .map(|e| format!("; the harness ended with an error: {}", tail_chars(e, 400)))
                .unwrap_or_default()
        );
        // What the implementer says it did and decided (e.g. "no lesson
        // this time", "ran the desktop smoke"): the reviewer weighs these
        // claims against the evidence instead of never hearing them.
        let implementer_note = match report.as_ref() {
            Some(r) if !r.summary.trim().is_empty() || !r.decisions.is_empty() => {
                let account = std::iter::once(r.summary.trim().to_string())
                    .chain(r.decisions.iter().map(|d| format!("- {d}")))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!(
                    "{implementer_note}\n\n{}",
                    brief::untrusted_block(
                        "The implementer's own account, a claim to check against the diff and verify results",
                        &account
                    )
                )
            }
            _ => implementer_note,
        };
        task.attempts[idx].summary = report.as_ref().map(|r| r.summary.clone());
        task.attempts[idx].handoff = report
            .as_ref()
            .map(|r| r.handoff.trim().to_string())
            .filter(|h| !h.is_empty());
        // Agent-reported decisions are trusted less than the owner's: strip
        // any leading "Owner:" the agent might have echoed back, prefix
        // with "Agent:", and never duplicate an identical entry.
        if let Some(r) = &report {
            for d in &r.decisions {
                let cleaned = ["Owner:", "Agent:"]
                    .iter()
                    .find_map(|p| d.strip_prefix(p))
                    .map(|s| s.trim_start())
                    .unwrap_or(d.as_str());
                let entry = format!("Agent: {cleaned}");
                if !task.decisions.contains(&entry) {
                    task.decisions.push(entry);
                }
            }
        }

        let wt3 = worktree.clone();
        let base3 = base_sha.clone();
        let changed = tokio::task::spawn_blocking(move || {
            git::changed_files(&wt3, &base3).unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        task.attempts[idx].changed_files = changed.clone();

        let outcome_blocked = report
            .as_ref()
            .map(|r| r.outcome == brief::Outcome::Blocked)
            .unwrap_or(false);

        if outcome_blocked {
            let question_text = report
                .as_ref()
                .map(|r| r.question.clone())
                .unwrap_or_default();
            // Routed through the same signature/budget accounting as any
            // other failure (spec item 8), so a recurring "blocked"
            // question can't loop forever even when the classifier keeps
            // saying it's answerable.
            record_failure(&mut task, idx, FailureKind::Blocked, question_text.clone());
            let should_continue = advance_after_failure(&mut task, attempt_budget);
            if !should_continue {
                // Same "attempts keep failing" escalation as
                // `fail_and_continue`'s, inlined because a
                // should_continue==true blocked report falls through to
                // `classify_answerable` below instead of looping.
                match wait_for_exhausted_answer(
                    &app,
                    &task_id,
                    &mut task,
                    idx,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                {
                    Some(_) => {
                        attempt_budget += 2;
                        just_answered = true;
                        drop(permit);
                        continue;
                    }
                    None => {
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                }
            }

            let answerable_p = classify_answerable(&app, &task, &question_text).await;
            let blocked_decision = decide_blocked_question(answerable_p);
            if let Some(p) = answerable_p {
                task.decisions.push(jev_answerable_line(
                    p,
                    blocked_decision == BlockedDecision::AnswerSelf,
                ));
            }
            match blocked_decision {
                BlockedDecision::AnswerSelf => {
                    let decision =
                        format!("Answer it yourself from the repository: {question_text}");
                    if !task.decisions.contains(&decision) {
                        task.decisions.push(decision);
                    }
                    task.status = TaskStatus::Queued;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                    drop(permit);
                    continue;
                }
                BlockedDecision::Waiting => {
                    task.question = Some(Question {
                        text: question_text,
                        options: vec![],
                    });
                    task.status = TaskStatus::Waiting;
                    task.updated_at = now_ms();
                    match wait_for_answer_with_triage(
                        &app,
                        &task_id,
                        &mut task,
                        idx,
                        &pending_answer,
                        &cancel,
                        &mut permit,
                    )
                    .await
                    {
                        Some(_) => {
                            just_answered = true;
                            drop(permit);
                            continue;
                        }
                        None => {
                            drop(permit);
                            app.finish_task_loop(&task_id);
                            return;
                        }
                    }
                }
            }
        }

        if changed.is_empty() {
            let (kind, detail) = match &outcome.error {
                Some(error) => (FailureKind::Error, error.clone()),
                None => (FailureKind::NoDeliverable, "No files changed.".to_string()),
            };
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                kind,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue { answered } => {
                    just_answered = answered;
                    drop(permit);
                    continue;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        if cancel.is_cancelled() {
            task.attempts[idx].status = AttemptStatus::Interrupted;
            task.attempts[idx].ended_at = Some(now_ms());
            task.status = TaskStatus::Stopped;
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
            drop(permit);
            app.finish_task_loop(&task_id);
            return;
        }

        // The base branch may have moved while the agent worked (another
        // task merged, the owner committed): carry the work onto it so verify
        // and review judge the tree that would actually land. Conflicts go
        // back to the agent as a failure; it resolves them, not this code.
        let mut base_sha = base_sha;
        let mut changed = changed;
        if let Some(base_ref) = task.base_ref.clone() {
            let (wt, from, tid, r) = (
                worktree.clone(),
                base_sha.clone(),
                task_id.clone(),
                base_ref.clone(),
            );
            let carried = tokio::task::spawn_blocking(move || {
                git::carry_onto_moved_base(&wt, &r, &from, &tid)
            })
            .await
            .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
            match carried {
                Ok(git::Rebase::Unchanged) => {}
                Ok(git::Rebase::Moved { new_sha }) => {
                    task.decisions.push(format!(
                        "Rebase: carried the work onto {base_ref} at {}",
                        short_sha(&new_sha)
                    ));
                    task.base_sha = new_sha.clone();
                    base_sha = new_sha;
                    let (wt, b) = (worktree.clone(), base_sha.clone());
                    changed = tokio::task::spawn_blocking(move || {
                        git::changed_files(&wt, &b).unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default();
                    task.attempts[idx].changed_files = changed.clone();
                    if changed.is_empty() {
                        // The base already holds this work: nothing left to commit.
                        task.decisions.push(format!(
                            "Rebase: {base_ref} already contains this work; nothing to commit"
                        ));
                        task.attempts[idx].status = AttemptStatus::Passed;
                        task.attempts[idx].ended_at = Some(now_ms());
                        task.status = TaskStatus::Done;
                        task.updated_at = now_ms();
                        let _ = app.store.save_task(&task);
                        app.broadcast_task(&task);
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                }
                Ok(git::Rebase::Skipped { reason }) => {
                    let note = format!("Rebase: not carried onto {base_ref} ({reason})");
                    if !task.decisions.contains(&note) {
                        task.decisions.push(note);
                    }
                }
                Ok(git::Rebase::Conflicts { new_sha, files }) => {
                    task.base_sha = new_sha.clone();
                    let detail = conflict_detail(&base_ref, &new_sha, &files, &task_id);
                    match fail_and_continue(
                        &app,
                        &task_id,
                        &mut task,
                        idx,
                        FailureKind::Verify,
                        detail,
                        &mut attempt_budget,
                        &pending_answer,
                        &cancel,
                        &mut permit,
                    )
                    .await
                    {
                        LoopSignal::Continue { answered } => {
                            just_answered = answered;
                            drop(permit);
                            continue;
                        }
                        LoopSignal::Stop => {
                            drop(permit);
                            app.finish_task_loop(&task_id);
                            return;
                        }
                    }
                }
                Err(e) => {
                    // carry_onto_moved_base restored the worktree, so the old
                    // base is still what the diff is against.
                    let note = format!(
                        "Rebase: could not carry the work onto {base_ref} ({e}); verifying on the old base"
                    );
                    if !task.decisions.contains(&note) {
                        task.decisions.push(note);
                    }
                }
            }
        }

        let verify_results = run_verify_cached(
            &app,
            &task_id,
            &worktree,
            &run_dir,
            &base_sha,
            &task.verify,
            &cancel,
        )
        .await;
        task.attempts[idx].verify = verify_results.clone();
        if let Some(failed) = verify_results.iter().find(|v| v.code != Some(0)) {
            let detail = format!(
                "{} exited {}.\n{}",
                failed.command,
                failed
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                failed.tail
            );
            match fail_and_continue(
                &app,
                &task_id,
                &mut task,
                idx,
                FailureKind::Verify,
                detail,
                &mut attempt_budget,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                LoopSignal::Continue { answered } => {
                    just_answered = answered;
                    drop(permit);
                    continue;
                }
                LoopSignal::Stop => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
            }
        }

        if let Some(path) = changed
            .iter()
            .find(|f| matches_any_protected(f, &settings.protected_paths))
        {
            task.attempts[idx].status = AttemptStatus::Blocked;
            task.attempts[idx].ended_at = Some(now_ms());
            task.question = Some(Question {
                text: format!("Change touches protected path {path}: approve or reject?"),
                options: vec!["approve".into(), "reject".into()],
            });
            task.status = TaskStatus::Waiting;
            task.updated_at = now_ms();
            match wait_for_answer(
                &app,
                &task_id,
                &mut task,
                &pending_answer,
                &cancel,
                &mut permit,
            )
            .await
            {
                None => {
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                Some(answer) => {
                    just_answered = true;
                    // Proceeds only on an exact "approve"; anything else --
                    // "reject", a typo, free text -- rejects the change.
                    if answer != "approve" {
                        match fail_and_continue(
                            &app,
                            &task_id,
                            &mut task,
                            idx,
                            FailureKind::Protected,
                            format!("Owner rejected change to protected path {path}"),
                            &mut attempt_budget,
                            &pending_answer,
                            &cancel,
                            &mut permit,
                        )
                        .await
                        {
                            LoopSignal::Continue { answered } => {
                                just_answered = just_answered || answered;
                                drop(permit);
                                continue;
                            }
                            LoopSignal::Stop => {
                                drop(permit);
                                app.finish_task_loop(&task_id);
                                return;
                            }
                        }
                    }
                    // approved: fall through to review/commit
                }
            }
        }

        let mut review_result: Option<ReviewResult> = None;
        if !settings.review.is_empty() {
            if let Some(review_route) =
                select_review_route(&settings, &route, task.variant().review_other_family)
            {
                if task.variant().review_other_family && review_route.harness == route.harness {
                    // The A/B arm says other family; record when it wasn't.
                    let note = format!(
                        "Orchestrator: no review route on another harness; reviewed by {}",
                        review_route.id
                    );
                    if !task.decisions.contains(&note) {
                        task.decisions.push(note);
                    }
                }
                if !wait_while_over_budget(
                    &app,
                    &task_id,
                    &mut task,
                    "review",
                    TaskStatus::Running,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                {
                    // Stopped at the gate: the attempt ends here too, or a
                    // restart would find it `running` and requeue the task.
                    if let Ok(Some(mut t)) = app.store.load_task(&task_id) {
                        if let Some(a) = t.attempts.get_mut(idx) {
                            a.status = AttemptStatus::Interrupted;
                            a.ended_at = Some(now_ms());
                        }
                        t.updated_at = now_ms();
                        let _ = app.store.save_task(&t);
                        app.broadcast_task(&t);
                    }
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                let mut review_cost = 0.0;
                let reviewed = run_review(
                    &app,
                    &task_id,
                    attempt_n,
                    &task,
                    &worktree,
                    &base_sha,
                    &verify_results,
                    &implementer_note,
                    review_route,
                    &deny_read,
                    &cancel,
                    &mut review_cost,
                )
                .await;
                // The task's total, and separately the attempt's
                // review_cost_usd for display: an attempt's own cost_usd is
                // what later resumes of its session subtract
                // (`attempt_cost`), so the review's cost must never join it.
                task.cost_usd += review_cost;
                task.attempts[idx].review_cost_usd =
                    Some(task.attempts[idx].review_cost_usd.unwrap_or(0.0) + review_cost);
                match reviewed {
                    Ok(r) => review_result = Some(r),
                    Err(RunError::Cancelled) => {
                        // A cancelled review is never a PASS: the attempt
                        // (and the task) is simply stopped.
                        task.attempts[idx].status = AttemptStatus::Interrupted;
                        task.attempts[idx].ended_at = Some(now_ms());
                        task.status = TaskStatus::Stopped;
                        task.updated_at = now_ms();
                        let _ = app.store.save_task(&task);
                        app.broadcast_task(&task);
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                    Err(RunError::Io(why)) => {
                        // No verdict is never a PASS: the owner decides
                        // whether to commit this attempt unreviewed.
                        task.attempts[idx].status = AttemptStatus::Blocked;
                        task.attempts[idx].ended_at = Some(now_ms());
                        task.question = Some(Question {
                            text: format!("The review gave no verdict ({why}). Commit this attempt unreviewed?"),
                            options: vec!["approve".into(), "retry".into()],
                        });
                        task.status = TaskStatus::Waiting;
                        task.updated_at = now_ms();
                        match wait_for_answer(
                            &app,
                            &task_id,
                            &mut task,
                            &pending_answer,
                            &cancel,
                            &mut permit,
                        )
                        .await
                        {
                            None => {
                                drop(permit);
                                app.finish_task_loop(&task_id);
                                return;
                            }
                            Some(answer) => {
                                if answer == "approve" {
                                    task.decisions.push(format!(
                                        "Orchestrator: attempt {attempt_n} committed without a review verdict"
                                    ));
                                } else {
                                    review_result = Some(ReviewResult {
                                        verdict: Verdict::Fail,
                                        findings: vec![format!(
                                            "Owner asked for another attempt: {answer}"
                                        )],
                                    });
                                }
                            }
                        }
                    }
                }
            } else {
                let note = format!(
                    "Orchestrator: review skipped, no route matches review \"{}\"",
                    settings.review
                );
                if !task.decisions.contains(&note) {
                    task.decisions.push(note);
                }
            }
        }
        task.attempts[idx].review = review_result.clone();

        if let Some(r) = &review_result {
            if r.verdict == Verdict::Fail {
                match fail_and_continue(
                    &app,
                    &task_id,
                    &mut task,
                    idx,
                    FailureKind::Review,
                    r.findings.join("; "),
                    &mut attempt_budget,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                {
                    LoopSignal::Continue { answered } => {
                        just_answered = answered;
                        drop(permit);
                        continue;
                    }
                    LoopSignal::Stop => {
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                }
            }
        }

        if !task.final_verify.is_empty() {
            // Not through the verify cache: it keys on the diff alone and
            // would hand back the fast checks' results.
            let final_results = run_verify_commands(
                &worktree,
                &run_dir.join("final"),
                &task.final_verify,
                settings.sandbox,
                &cancel,
            )
            .await;
            task.attempts[idx]
                .verify
                .extend(final_results.iter().cloned());
            if let Some(failed) = final_results.iter().find(|v| v.code != Some(0)) {
                let detail = format!(
                    "Final check {} exited {}.\n{}",
                    failed.command,
                    failed
                        .code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "null".to_string()),
                    failed.tail
                );
                match fail_and_continue(
                    &app,
                    &task_id,
                    &mut task,
                    idx,
                    FailureKind::Verify,
                    detail,
                    &mut attempt_budget,
                    &pending_answer,
                    &cancel,
                    &mut permit,
                )
                .await
                {
                    LoopSignal::Continue { answered } => {
                        just_answered = answered;
                        drop(permit);
                        continue;
                    }
                    LoopSignal::Stop => {
                        drop(permit);
                        app.finish_task_loop(&task_id);
                        return;
                    }
                }
            }
        }

        if let Some(parent_id) = task.parent.clone() {
            let landing = land_on_parent(
                &app, &mut task, idx, attempt_n, &worktree, &run_dir, &parent_id, &cancel,
            )
            .await;
            match landing {
                Landing::Landed => {
                    git::delete_wip_ref(&worktree, &task_id);
                    task.attempts[idx].status = AttemptStatus::Passed;
                    task.attempts[idx].ended_at = Some(now_ms());
                    task.status = TaskStatus::Done;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                Landing::Failed { kind, detail } => {
                    match fail_and_continue(
                        &app,
                        &task_id,
                        &mut task,
                        idx,
                        kind,
                        detail,
                        &mut attempt_budget,
                        &pending_answer,
                        &cancel,
                        &mut permit,
                    )
                    .await
                    {
                        LoopSignal::Continue { answered } => {
                            just_answered = answered;
                            drop(permit);
                            continue;
                        }
                        LoopSignal::Stop => {
                            drop(permit);
                            app.finish_task_loop(&task_id);
                            return;
                        }
                    }
                }
                Landing::Cancelled => {
                    task.attempts[idx].status = AttemptStatus::Interrupted;
                    task.attempts[idx].ended_at = Some(now_ms());
                    task.status = TaskStatus::Stopped;
                    task.updated_at = now_ms();
                    let _ = app.store.save_task(&task);
                    app.broadcast_task(&task);
                    drop(permit);
                    app.finish_task_loop(&task_id);
                    return;
                }
                Landing::NoParent => {}
            }
        }

        let wt4 = worktree.clone();
        let title = task.title.clone();
        let tid = task_id.clone();
        let commit_res =
            tokio::task::spawn_blocking(move || git::commit(&wt4, &title, &tid, attempt_n)).await;
        match commit_res {
            Ok(Ok(())) => {
                git::delete_wip_ref(&worktree, &task_id);
                task.attempts[idx].status = AttemptStatus::Passed;
                task.attempts[idx].ended_at = Some(now_ms());
                task.status = TaskStatus::Done;
            }
            Ok(Err(e)) => {
                record_failure(&mut task, idx, FailureKind::Error, e.to_string());
                task.status = TaskStatus::Failed;
            }
            Err(e) => {
                record_failure(
                    &mut task,
                    idx,
                    FailureKind::Error,
                    format!("commit task panicked: {e}"),
                );
                task.status = TaskStatus::Failed;
            }
        }
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);
        drop(permit);
        app.finish_task_loop(&task_id);
        return;
    }
}

async fn mark_stopped_if_not_already(app: &Arc<App>, task_id: &str) {
    if let Ok(Some(mut task)) = app.store.load_task(task_id) {
        if !matches!(
            task.status,
            TaskStatus::Stopped | TaskStatus::Done | TaskStatus::Failed
        ) {
            task.status = TaskStatus::Stopped;
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_one_verify_command_kills_the_whole_process_group_on_timeout() {
        let tmp = tempfile::tempdir().unwrap();
        let pidfile = tmp.path().join("child.pid");
        // `wait` keeps the `sh -c` process itself alive for the full sleep,
        // so the 300ms timeout fires while both it and the backgrounded
        // `sleep` are still running and share its process group (killpg's
        // target). A naive `timeout(...)` around `Command::output()` would
        // stop waiting here but leave the whole group running.
        let cmd = format!("sleep 60 & echo $! > {}; wait", pidfile.display());

        let cancel = CancelToken::new();
        let outcome = run_one_verify_command(
            tmp.path(),
            &cmd,
            Duration::from_millis(300),
            SandboxMode::Host,
            &[],
            &cancel,
        )
        .await;
        assert!(outcome.tail.contains("timed out"));

        let pid_text = std::fs::read_to_string(&pidfile).expect("child wrote its pid");
        let pid: i32 = pid_text.trim().parse().expect("valid pid");
        let start = std::time::Instant::now();
        loop {
            let alive = unsafe { libc::kill(pid, 0) == 0 };
            if !alive {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "backgrounded sleep {pid} is still alive after the verify command timed out"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn run_one_verify_command_kills_the_group_on_cancellation_too() {
        let tmp = tempfile::tempdir().unwrap();
        let pidfile = tmp.path().join("child.pid");
        let cmd = format!("sleep 60 & echo $! > {}; wait", pidfile.display());
        let cancel = CancelToken::new();
        let cancel2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancel2.cancel();
        });
        let outcome = run_one_verify_command(
            tmp.path(),
            &cmd,
            Duration::from_secs(60),
            SandboxMode::Host,
            &[],
            &cancel,
        )
        .await;
        assert_eq!(outcome.tail, "cancelled");
    }

    #[test]
    fn cancel_token_is_visible_synchronously_after_cancel() {
        let cancel = CancelToken::new();
        assert!(!cancel.is_cancelled());
        cancel.cancel();
        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_token_wakes_a_waiter_registered_after_cancel() {
        let cancel = CancelToken::new();
        cancel.cancel();
        // Must resolve immediately, not hang -- this is exactly the "check
        // between every step" use case.
        tokio::time::timeout(Duration::from_millis(200), cancel.cancelled())
            .await
            .expect("cancelled() must resolve immediately once already cancelled");
    }

    #[test]
    fn validate_task_id_rejects_path_traversal_and_non_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        assert!(validate_task_id(&store, "../../etc/passwd").is_err());
        assert!(validate_task_id(&store, "not-a-uuid").is_err());
        assert!(validate_task_id(&store, "").is_err());
        let real_id = uuid::Uuid::new_v4().to_string();
        assert!(validate_task_id(&store, &real_id).is_ok());
    }

    #[test]
    fn failure_signature_prefers_an_error_looking_line_over_the_first_line() {
        let detail = "Compiling...\nWarning: unused variable\nError: assertion failed at line 42\nmore noise";
        let sig = failure_signature(FailureKind::Verify, detail);
        assert!(sig.contains("assertion failed at line"));
        assert!(!sig.contains("Compiling"));
        // Digits and the line number are stripped so two runs at different
        // line numbers still normalize to the same signature.
        assert!(!sig.contains("42"));
    }

    #[test]
    fn failure_signature_strips_absolute_paths() {
        let detail = "Error at /Users/me/repo/src/foo.ts:10: boom";
        let sig = failure_signature(FailureKind::Verify, detail);
        assert!(!sig.contains("/Users/me/repo"));
        assert!(sig.contains("boom"));
    }

    #[test]
    fn failure_signature_falls_back_to_first_line_without_error_keywords() {
        let sig = failure_signature(FailureKind::NoDeliverable, "nothing changed\nsecond line");
        assert!(sig.contains("nothing changed"));
    }

    #[test]
    fn failure_signature_uses_kind_and_truncates_to_120_chars() {
        let long = "error: ".to_string() + &"x".repeat(200);
        let sig = failure_signature(FailureKind::NoDeliverable, &long);
        assert!(sig.starts_with("no_deliverable:"));
        assert_eq!(sig.len(), "no_deliverable:".len() + 120);
    }

    fn attempt_with_failure(n: u32, signature: &str) -> Attempt {
        Attempt {
            n,
            stage: Stage::Implement,
            route_id: "claude-sonnet".into(),
            harness: Harness::Claude,
            model: "sonnet".into(),
            reason: "r".into(),
            session_id: None,
            pgid: None,
            resumed: false,
            started_at: 0,
            ended_at: None,
            status: AttemptStatus::Failed,
            summary: None,
            handoff: None,
            changed_files: vec![],
            verify: vec![],
            gate_blocks: 0,
            prefix_tokens: None,
            review: None,
            failure: Some(Failure {
                kind: FailureKind::Verify,
                detail: "d".into(),
                signature: signature.to_string(),
            }),
            usage: None,
            cost_usd: None,
            cost_estimated: false,
            review_cost_usd: None,
            advice: None,
            advisor_cost_usd: None,
        }
    }

    #[test]
    fn a_retry_on_the_same_tier_keeps_the_fallback_note() {
        let mut task = task_with_status(TaskStatus::Running);
        task.tier_fallback = Some("no classifier key".into());
        task.attempts = vec![attempt_with_failure(1, "sig-a")];
        assert!(advance_after_failure(&mut task, 4));
        assert_eq!(task.tier, Tier::Standard);
        assert_eq!(task.tier_fallback.as_deref(), Some("no classifier key"));
        // The same failure again moves the task up a tier: the note goes.
        task.attempts.push(attempt_with_failure(2, "sig-a"));
        assert!(advance_after_failure(&mut task, 4));
        assert_eq!(task.tier, Tier::Hard);
        assert_eq!(task.tier_fallback, None);
    }

    #[test]
    fn consecutive_same_signature_counts_trailing_run_only() {
        let attempts = vec![
            attempt_with_failure(1, "sig-a"),
            attempt_with_failure(2, "sig-b"),
            attempt_with_failure(3, "sig-b"),
            attempt_with_failure(4, "sig-b"),
        ];
        assert_eq!(consecutive_same_signature(&attempts, "sig-b"), 3);
        assert_eq!(consecutive_same_signature(&attempts, "sig-a"), 0);
    }

    #[test]
    fn decide_after_failure_ties_up_on_repeat_signature() {
        let input = FailureDecisionInput {
            tier: Tier::Mechanical,
            signature: "sig-b",
            previous_signature: Some("sig-b"),
            consecutive_same: 2,
            attempt_n: 2,
            max_attempts: 4,
        };
        match decide_after_failure(&input) {
            FailureDecision::NextAttempt { tier } => assert_eq!(tier, Tier::Standard),
            _ => panic!("expected next attempt"),
        }
    }

    #[test]
    fn decide_after_failure_stays_same_tier_on_new_signature() {
        let input = FailureDecisionInput {
            tier: Tier::Standard,
            signature: "sig-c",
            previous_signature: Some("sig-b"),
            consecutive_same: 1,
            attempt_n: 2,
            max_attempts: 4,
        };
        match decide_after_failure(&input) {
            FailureDecision::NextAttempt { tier } => assert_eq!(tier, Tier::Standard),
            _ => panic!("expected next attempt"),
        }
    }

    #[test]
    fn decide_after_failure_waits_after_three_consecutive() {
        let input = FailureDecisionInput {
            tier: Tier::Standard,
            signature: "sig-b",
            previous_signature: Some("sig-b"),
            consecutive_same: 3,
            attempt_n: 3,
            max_attempts: 10,
        };
        match decide_after_failure(&input) {
            FailureDecision::Waiting { question } => assert!(question.contains("sig-b")),
            _ => panic!("expected waiting"),
        }
    }

    #[test]
    fn decide_after_failure_waits_when_attempts_exhausted() {
        let input = FailureDecisionInput {
            tier: Tier::Standard,
            signature: "sig-x",
            previous_signature: None,
            consecutive_same: 1,
            attempt_n: 4,
            max_attempts: 4,
        };
        match decide_after_failure(&input) {
            FailureDecision::Waiting { .. } => {}
            _ => panic!("expected waiting"),
        }
    }

    #[test]
    fn decide_blocked_question_answers_self_above_threshold() {
        assert_eq!(
            decide_blocked_question(Some(0.7)),
            BlockedDecision::AnswerSelf
        );
        assert_eq!(
            decide_blocked_question(Some(0.9)),
            BlockedDecision::AnswerSelf
        );
        assert_eq!(
            decide_blocked_question(Some(0.69)),
            BlockedDecision::Waiting
        );
        assert_eq!(decide_blocked_question(None), BlockedDecision::Waiting);
    }

    #[test]
    fn attempt_screenshots_lists_new_images_under_artifacts_only() {
        let tmp = tempfile::tempdir().unwrap();
        let shots = tmp.path().join("artifacts/ui");
        std::fs::create_dir_all(&shots).unwrap();
        std::fs::write(shots.join("after.png"), b"x").unwrap();
        std::fs::write(shots.join("notes.txt"), b"x").unwrap();
        std::fs::write(tmp.path().join("root.png"), b"x").unwrap();
        let found = attempt_screenshots(tmp.path(), 0);
        assert_eq!(found, vec![shots.join("after.png")]);
        assert!(attempt_screenshots(tmp.path(), i64::MAX).is_empty());
    }

    #[test]
    fn resolve_variant_lays_known_flags_over_the_defaults() {
        let defaults = Variant {
            stall_timeout_secs: 900,
            ..Variant::default()
        };
        let v = resolve_variant(&defaults, Some(&json!({"retryMode": "fresh"}))).unwrap();
        assert_eq!(v.retry_mode, RetryMode::Fresh);
        assert_eq!(v.stall_timeout_secs, 900);
        assert_eq!(resolve_variant(&defaults, None).unwrap(), defaults);
        assert!(resolve_variant(&defaults, Some(&json!({"nope": 1}))).is_err());
        assert!(resolve_variant(&defaults, Some(&json!({"retryMode": "sideways"}))).is_err());
        assert!(resolve_variant(&defaults, Some(&json!("fresh"))).is_err());
        assert!(resolve_variant(&defaults, Some(&json!({"stallTimeoutSecs": u64::MAX}))).is_err());
    }

    #[test]
    fn resolve_variant_takes_route_overrides_the_defaults_leave_out() {
        let v = resolve_variant(
            &Variant::default(),
            Some(
                &json!({"plannerRoute": "claude-sonnet", "tierRoutes": {"hard": "claude-sonnet"}}),
            ),
        )
        .unwrap();
        assert_eq!(v.planner_route.as_deref(), Some("claude-sonnet"));
        assert_eq!(v.tier_routes.get(&Tier::Hard).unwrap(), "claude-sonnet");
        let bad_tier = json!({"tierRoutes": {"extreme": "claude-sonnet"}});
        assert!(resolve_variant(&Variant::default(), Some(&bad_tier)).is_err());
    }

    #[test]
    fn variant_route_line_names_the_override() {
        assert_eq!(
            variant_route_line("tier hard", "claude-sonnet"),
            "Variant: tier hard -> route claude-sonnet (override)"
        );
    }

    #[test]
    fn jev_tier_line_formats_choice_probability_and_route() {
        assert_eq!(
            jev_tier_line("mechanical", 0.82, "codex"),
            "Jev: tier mechanical (p 0.82) -> route codex"
        );
    }

    #[test]
    fn jev_tier_fallback_line_formats_reason_and_route() {
        assert_eq!(
            jev_tier_fallback_line("no classifier key", "claude-sonnet"),
            "Jev: tier unavailable (no classifier key) -> fallback standard, route claude-sonnet"
        );
    }

    fn tier_answer(choice: &str, p: f64) -> classify::Answers {
        let mut answers = classify::Answers::new();
        answers.insert(
            "tier".to_string(),
            classify::Answer {
                choice: Some(choice.to_string()),
                probabilities: Some(HashMap::from([(choice.to_string(), p)])),
                ..Default::default()
            },
        );
        answers
    }

    #[test]
    fn pick_tier_falls_back_to_classifier_off_regardless_of_the_result() {
        let result: Result<classify::Answers, classify::ClassifyError> =
            Err(classify::ClassifyError("network is down".to_string()));
        match pick_tier(&result, true) {
            TierPick::Fallback { reason } => assert_eq!(reason, "classifier off"),
            TierPick::Classified { .. } => panic!("expected a fallback"),
        }
    }

    #[test]
    fn pick_tier_falls_back_to_no_classifier_key() {
        let result: Result<classify::Answers, classify::ClassifyError> =
            Err(classify::ClassifyError(classify::NO_KEY_REASON.to_string()));
        match pick_tier(&result, false) {
            TierPick::Fallback { reason } => assert_eq!(reason, "no classifier key"),
            TierPick::Classified { .. } => panic!("expected a fallback"),
        }
    }

    #[test]
    fn pick_tier_falls_back_to_classifier_call_failed_and_drops_the_raw_error() {
        let result: Result<classify::Answers, classify::ClassifyError> = Err(
            classify::ClassifyError("https://openrouter.ai returned 500: secret leak".to_string()),
        );
        match pick_tier(&result, false) {
            TierPick::Fallback { reason } => {
                assert_eq!(reason, "classifier call failed");
                assert!(!reason.contains("openrouter"));
                assert!(!reason.contains("secret"));
            }
            TierPick::Classified { .. } => panic!("expected a fallback"),
        }
    }

    #[test]
    fn pick_tier_falls_back_to_no_tier_answer_when_choice_is_missing() {
        let mut answers = classify::Answers::new();
        answers.insert("tier".to_string(), classify::Answer::default());
        let result: Result<classify::Answers, classify::ClassifyError> = Ok(answers);
        match pick_tier(&result, false) {
            TierPick::Fallback { reason } => assert_eq!(reason, "no tier answer"),
            TierPick::Classified { .. } => panic!("expected a fallback"),
        }
    }

    #[test]
    fn pick_tier_falls_back_to_no_tier_answer_when_the_tier_question_is_absent() {
        let result: Result<classify::Answers, classify::ClassifyError> =
            Ok(classify::Answers::new());
        match pick_tier(&result, false) {
            TierPick::Fallback { reason } => assert_eq!(reason, "no tier answer"),
            TierPick::Classified { .. } => panic!("expected a fallback"),
        }
    }

    #[test]
    fn pick_tier_falls_back_when_unsure() {
        let result: Result<classify::Answers, classify::ClassifyError> =
            Ok(tier_answer("hard", 0.45));
        match pick_tier(&result, false) {
            TierPick::Fallback { reason } => assert_eq!(reason, "unsure: hard p 0.45"),
            TierPick::Classified { .. } => panic!("expected a fallback"),
        }
    }

    #[test]
    fn pick_tier_classifies_a_confident_answer_and_is_not_a_fallback() {
        let result: Result<classify::Answers, classify::ClassifyError> =
            Ok(tier_answer("hard", 0.82));
        match pick_tier(&result, false) {
            TierPick::Classified { tier, choice, p } => {
                assert_eq!(tier, Tier::Hard);
                assert_eq!(choice, "hard");
                assert_eq!(p, 0.82);
            }
            TierPick::Fallback { reason } => panic!("expected a classified pick, got {reason}"),
        }
    }

    #[test]
    fn jev_answerable_line_names_the_outcome() {
        assert_eq!(
            jev_answerable_line(0.9, true),
            "Jev: answerable from repo (p 0.90) -> agent sent back"
        );
        assert_eq!(
            jev_answerable_line(0.4, false),
            "Jev: answerable from repo (p 0.40) -> asked owner"
        );
    }

    #[test]
    fn jev_stop_gate_line_names_the_outcome() {
        assert_eq!(
            jev_stop_gate_line(true, 0.9, 0.1),
            "Jev: premature finish (p 0.90) -> sent back"
        );
        assert_eq!(
            jev_stop_gate_line(false, 0.9, 0.85),
            "Jev: verification looks fine (p 0.85) -> allowed"
        );
    }

    #[test]
    fn jev_plan_preflight_line_reports_all_three_scores() {
        assert_eq!(
            jev_plan_preflight_line(0.9, 0.8, 0.3),
            "Jev: goal 0.90, criteria 0.80, verification 0.30"
        );
    }

    #[test]
    fn orchestrator_answer_line_includes_question_answer_and_reason() {
        assert_eq!(
            orchestrator_answer_line("Which theme?", "light", "README says so"),
            "Orchestrator: Which theme? -> light (README says so)"
        );
    }

    #[test]
    fn orchestrator_escalate_line_includes_the_reason() {
        assert_eq!(
            orchestrator_escalate_line("no default anywhere"),
            "Orchestrator: escalated (no default anywhere)"
        );
    }

    #[test]
    fn glob_match_supports_double_star_suffix() {
        assert!(glob_match("src/app/**", "src/app/SectionPage.tsx"));
        assert!(glob_match("src/app/**", "src/app/nested/Deep.tsx"));
        assert!(!glob_match("src/app/**", "src/extensions/registry.ts"));
        assert!(glob_match("*.md", "README.md"));
        assert!(!glob_match("*.md", "README.txt"));
    }

    #[test]
    fn matches_any_protected_checks_every_glob() {
        let globs = vec!["src/app/**".to_string(), "electron/main.cjs".to_string()];
        assert!(matches_any_protected("src/app/SectionPage.tsx", &globs));
        assert!(matches_any_protected("electron/main.cjs", &globs));
        assert!(!matches_any_protected("src/extensions/registry.ts", &globs));
    }

    #[test]
    fn select_review_route_auto_prefers_the_hard_tier_then_another_harness() {
        let settings = Settings::default();
        let route = |id: &str| settings.routes.iter().find(|r| r.id == id).unwrap();
        let hard = settings.tiers.get(&Tier::Hard).unwrap();
        assert_eq!(
            &select_review_route(&settings, route("claude-sonnet"), false)
                .unwrap()
                .id,
            hard
        );
        assert_eq!(
            &select_review_route(&settings, route("codex"), false)
                .unwrap()
                .id,
            hard
        );
        let for_hard = select_review_route(&settings, route(hard), false).unwrap();
        assert_ne!(for_hard.harness, route(hard).harness);
    }

    #[test]
    fn select_review_route_other_family_never_reviews_claude_with_claude() {
        let settings = Settings::default();
        let route = |id: &str| settings.routes.iter().find(|r| r.id == id).unwrap();
        let for_sonnet = select_review_route(&settings, route("claude-sonnet"), true).unwrap();
        assert_eq!(for_sonnet.harness, Harness::Codex);
        let for_codex = select_review_route(&settings, route("codex"), true).unwrap();
        assert_eq!(
            for_codex.id, "claude-opus",
            "the hard route when it is the other family"
        );

        // A hard route on Codex, listed after another Codex route, still wins.
        let mut settings = Settings::default();
        settings.routes.push(Route {
            id: "codex-strong".into(),
            label: "Codex strong".into(),
            harness: Harness::Codex,
            model: Some("gpt-5.3-codex".into()),
            effort: None,
            profile_id: None,
        });
        settings.tiers.insert(Tier::Hard, "codex-strong".into());
        let sonnet = settings
            .routes
            .iter()
            .find(|r| r.id == "claude-sonnet")
            .unwrap();
        assert_eq!(
            select_review_route(&settings, sonnet, true).unwrap().id,
            "codex-strong"
        );
    }

    #[test]
    fn select_review_route_explicit_id() {
        let settings = Settings {
            review: "claude-opus".to_string(),
            ..Settings::default()
        };
        let implementer = settings.routes.iter().find(|r| r.id == "codex").unwrap();
        let route = select_review_route(&settings, implementer, false).unwrap();
        assert_eq!(route.id, "claude-opus");
    }

    #[test]
    fn verify_tail_keeps_stdout_failures_behind_a_long_stderr() {
        let stderr = "   Compiling crate\n".repeat(500);
        let tail = verify_tail(
            b"test foo ... FAILED\nfailures:\n    foo\n",
            stderr.as_bytes(),
        );
        assert!(tail.contains("test foo ... FAILED"));
        assert!(tail.len() < 4200);
    }

    #[test]
    fn verify_tail_surfaces_failing_test_names_and_assertions() {
        let stderr = "WARNING bundler chunk too large\n".repeat(300);
        let stdout = format!(
            "test a_broken_thing ... FAILED\n\nthread 'a_broken_thing' panicked at t.rs:3:5:\nassertion `left == right` failed: boom\n  left: 1\n right: 2\n{}",
            "test ok_one ... ok\n".repeat(400)
        );
        let tail = verify_tail(stdout.as_bytes(), stderr.as_bytes());
        assert!(tail.contains("a_broken_thing ... FAILED"));
        assert!(tail.contains("assertion `left == right` failed: boom"));
        assert!(tail.len() < 4500);
        assert!(tail.trim_end().ends_with("right: 2"));
    }

    #[test]
    fn split_verify_commands_moves_prose_to_review() {
        let tmp = tempfile::tempdir().unwrap();
        let cmds: Vec<String> = [
            "true",
            "FOO=1 sh -c true",
            "(cd . && true)",
            "./scripts/created-by-the-task.sh",
            "npm run test:desktop (only if src/app/X.tsx ends up touched)",
            "ui-evidence skill: screenshot the panel's Archive section",
        ]
        .map(String::from)
        .to_vec();
        let (run, judged) = split_verify_commands(tmp.path(), &cmds);
        assert_eq!(
            run,
            vec![
                "true",
                "FOO=1 sh -c true",
                "(cd . && true)",
                "./scripts/created-by-the-task.sh"
            ]
        );
        assert_eq!(judged.len(), 2);
    }

    #[test]
    fn attempt_cost_subtracts_what_earlier_attempts_on_the_session_already_paid() {
        // Real figures from one task: fresh $2.13, resumed $2.94, resumed $3.04.
        let with = |n, resumed, cost: Option<f64>| Attempt {
            session_id: Some("s1".into()),
            resumed,
            cost_usd: cost,
            ..attempt_with_failure(n, "x")
        };
        let first = with(1, false, Some(2.13));
        let second = with(2, true, None);
        let second_cost = attempt_cost(std::slice::from_ref(&first), &second, 2.94, false);
        assert!((second_cost - 0.81).abs() < 1e-9);
        let third = with(3, true, None);
        let paid = [first, with(2, true, Some(second_cost))];
        assert!((attempt_cost(&paid, &third, 3.04, false) - 0.10).abs() < 1e-9);
        // A fresh session is never discounted.
        assert_eq!(attempt_cost(&paid, &with(4, false, None), 1.5, false), 1.5);
        // An estimate covers its own run only.
        assert_eq!(attempt_cost(&paid, &third, 0.4, true), 0.4);
        // An attempt killed mid-run and estimated on recovery is still part
        // of the session total the resume reports, so it is subtracted too:
        // the task pays 2.13 + 0.5 + 0.37 = 3.0, the session's own total.
        let killed = Attempt {
            cost_estimated: true,
            ..with(2, true, Some(0.5))
        };
        let after_kill = [with(1, false, Some(2.13)), killed];
        assert!((attempt_cost(&after_kill, &third, 3.0, false) - 0.37).abs() < 1e-9);
    }

    #[test]
    fn a_plan_attempt_settles_only_the_runs_whose_cost_is_not_saved_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = tmp.path().join("plan");
        std::fs::create_dir_all(&plan).unwrap();
        let result = |cost: f64| {
            format!(
                r#"{{"type":"result","total_cost_usd":{cost},"usage":{{"input_tokens":1,"output_tokens":1}},"result":"x"}}"#
            )
        };
        std::fs::write(plan.join("events.jsonl"), result(0.25)).unwrap();
        std::fs::write(plan.join("events-retry.jsonl"), result(0.5)).unwrap();
        let prices = Settings::default().prices;
        let settle = |saved: Option<f64>| {
            let mut task = task_with_status(TaskStatus::Drafting);
            task.cost_usd = saved.unwrap_or(0.0);
            task.attempts = vec![Attempt {
                stage: Stage::Plan,
                cost_usd: saved,
                ..attempt_with_failure(1, "x")
            }];
            settle_unfinished_cost(&mut task, 0, tmp.path(), &prices);
            (task.attempts[0].cost_usd, task.cost_usd)
        };
        // No cost saved yet: every plan run's file counts.
        assert_eq!(settle(None), (Some(0.75), 0.75));
        // Died in the retry: the first run's cost was saved before it.
        assert_eq!(settle(Some(0.25)), (Some(0.75), 0.75));
    }

    #[test]
    fn diff_hash_changes_when_an_untracked_file_is_edited() {
        let tmp = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(tmp.path())
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        git(&[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ]);
        std::fs::write(tmp.path().join("new.rs"), "broken").unwrap();
        let before = diff_hash(tmp.path(), "HEAD");
        std::fs::write(tmp.path().join("new.rs"), "fixed").unwrap();
        assert_ne!(before, diff_hash(tmp.path(), "HEAD"));
        // A moved base is a different tree to verify, even when the task's
        // own diff reads the same.
        git(&[
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "base moves",
        ]);
        assert_ne!(
            diff_hash(tmp.path(), "HEAD~1"),
            diff_hash(tmp.path(), "HEAD")
        );
        // Never followed: reading /dev/zero through it would never finish.
        std::os::unix::fs::symlink("/dev/zero", tmp.path().join("zero")).unwrap();
        diff_hash(tmp.path(), "HEAD");
    }

    #[test]
    fn simple_hash_is_stable_and_sensitive_to_content() {
        assert_eq!(simple_hash("abc"), simple_hash("abc"));
        assert_ne!(simple_hash("abc"), simple_hash("abd"));
    }

    // -- task.archive / task.unarchive --------------------------------------
    //
    // These drive `App::dispatch` directly (no real socket, no real harness
    // process): the validate-then-mutate logic under test only ever looks at
    // a task's stored `status`, so a task written straight into the store
    // with the status under test is equivalent to -- and far faster than --
    // getting a real attempt loop into that same state.

    /// The `TempDir` must stay alive for as long as the `App` does (dropping
    /// it deletes the directory `App` reads and writes) -- callers keep the
    /// tuple bound for the whole test, not just the `Arc<App>`.
    fn test_app() -> (Arc<App>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            dir.path().join("data"),
            dir.path().join("orchd.sock"),
            "orchd".to_string(),
        )
        .unwrap();
        (app, dir)
    }

    fn task_with_status(status: TaskStatus) -> Task {
        Task {
            id: uuid::Uuid::new_v4().to_string(),
            title: "Do thing".into(),
            goal: "Do the thing".into(),
            criteria: vec![],
            verify: vec![],
            final_verify: vec![],
            request: None,
            repo: "/repo".into(),
            worktree: "/repo-task".into(),
            branch: "task/do-thing".into(),
            base_sha: "deadbeef".into(),
            base_ref: None,
            depends_on: vec![],
            parent: None,
            status,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            budget_raises: 0,
            archived: false,
            planned_tier: None,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn graph_task(id: &str, status: TaskStatus, depends_on: &[&str], parent: Option<&str>) -> Task {
        let mut t = task_with_status(status);
        t.id = id.to_string();
        t.depends_on = depends_on.iter().map(|d| d.to_string()).collect();
        t.parent = parent.map(str::to_string);
        t
    }

    #[test]
    fn a_parent_waiting_for_a_child_that_depends_on_it_is_a_cycle() {
        let tasks = vec![
            graph_task("p", TaskStatus::Queued, &[], None),
            graph_task("c", TaskStatus::Queued, &["p"], Some("p")),
        ];
        assert!(has_cycle(&wait_edges(&tasks)));
        let tasks = vec![
            graph_task("p", TaskStatus::Queued, &[], None),
            graph_task("a", TaskStatus::Queued, &[], Some("p")),
            graph_task("b", TaskStatus::Queued, &["a"], Some("p")),
        ];
        assert!(!has_cycle(&wait_edges(&tasks)));
    }

    #[test]
    fn waits_state_reads_dependencies_and_children() {
        let mut tasks = vec![
            graph_task("p", TaskStatus::Running, &[], None),
            graph_task("a", TaskStatus::Done, &[], Some("p")),
            graph_task("b", TaskStatus::Queued, &["a", "gone"], Some("p")),
        ];
        assert_eq!(waits_state(&tasks[2], &tasks), Waits::Ready);
        assert_eq!(waits_state(&tasks[0], &tasks), Waits::Pending);
        assert_eq!(waits_state(&tasks[1], &tasks), Waits::Nothing);
        tasks[2].status = TaskStatus::Failed;
        assert_eq!(
            waits_state(&tasks[0], &tasks),
            Waits::Ended(vec!["b".into()])
        );
        // An archived child no longer holds its parent back.
        tasks[2].archived = true;
        assert_eq!(waits_state(&tasks[0], &tasks), Waits::Ready);
    }

    #[test]
    fn check_subtasks_accepts_a_serial_pair_and_rejects_bad_graphs() {
        let part = |key: &str, deps: &[&str]| brief::PlanSubtask {
            key: key.into(),
            title: key.to_uppercase(),
            request: format!("do {key}"),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
        };
        assert!(check_subtasks(&[part("a", &[]), part("b", &["a"])]).is_ok());
        assert!(check_subtasks(&[part("a", &[])]).is_err());
        assert!(check_subtasks(&[part("a", &["b"]), part("b", &["a"])])
            .unwrap_err()
            .contains("cycle"));
        assert!(check_subtasks(&[part("a", &[]), part("b", &["z"])])
            .unwrap_err()
            .contains("unknown key"));
        assert!(check_subtasks(&[part("a", &[]), part("a", &[])])
            .unwrap_err()
            .contains("duplicate"));
        let many: Vec<_> = (0..=MAX_SUBTASKS)
            .map(|i| part(&i.to_string(), &[]))
            .collect();
        assert!(check_subtasks(&many).is_err());
    }

    #[test]
    fn a_subtask_request_carries_the_whole_and_what_was_decided() {
        let mut parent = task_with_status(TaskStatus::Running);
        parent.decisions = vec!["Owner: Which default? -> dark".into(), "Jev: x".into()];
        let text = subtask_request(&parent, "  write the toggle ");
        assert!(
            text.starts_with("write the toggle\n\nThis is one part of a larger task, \"Do thing\"")
        );
        assert!(text.contains("- Owner: Which default? -> dark"));
        assert!(!text.contains("Jev: x"));
    }

    #[tokio::test]
    async fn task_archive_refuses_running_drafting_and_waiting_tasks() {
        let (app, _dir) = test_app();
        for status in [
            TaskStatus::Running,
            TaskStatus::Drafting,
            TaskStatus::Waiting,
        ] {
            let task = task_with_status(status);
            app.store.save_task(&task).unwrap();
            let err = app
                .dispatch("task.archive", json!({"id": task.id}))
                .await
                .unwrap_err();
            assert!(
                err.contains("running, drafting, or waiting"),
                "status {status:?}: unexpected error {err}"
            );
            let reloaded = app.store.load_task(&task.id).unwrap().unwrap();
            assert!(!reloaded.archived, "status {status:?} must not be archived");
        }
    }

    #[tokio::test]
    async fn task_archive_succeeds_on_queued_stopped_failed_and_done_tasks() {
        let (app, _dir) = test_app();
        for status in [
            TaskStatus::Queued,
            TaskStatus::Stopped,
            TaskStatus::Failed,
            TaskStatus::Done,
        ] {
            let task = task_with_status(status);
            app.store.save_task(&task).unwrap();
            let result = app
                .dispatch("task.archive", json!({"id": task.id}))
                .await
                .unwrap();
            assert_eq!(result["archived"], true, "status {status:?}");
            let reloaded = app.store.load_task(&task.id).unwrap().unwrap();
            assert!(
                reloaded.archived,
                "status {status:?} should now be archived"
            );
        }
    }

    #[tokio::test]
    async fn task_list_hides_archived_tasks_unless_include_archived_is_set() {
        let (app, _dir) = test_app();
        let task = task_with_status(TaskStatus::Done);
        app.store.save_task(&task).unwrap();
        app.dispatch("task.archive", json!({"id": task.id}))
            .await
            .unwrap();

        let default_list = app.dispatch("task.list", json!({})).await.unwrap();
        assert!(default_list.as_array().unwrap().is_empty());

        let with_archived = app
            .dispatch("task.list", json!({"includeArchived": true}))
            .await
            .unwrap();
        let listed = with_archived.as_array().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], task.id);
        assert_eq!(listed[0]["archived"], true);
    }

    #[tokio::test]
    async fn task_unarchive_makes_a_task_reappear_in_the_default_task_list() {
        let (app, _dir) = test_app();
        let task = task_with_status(TaskStatus::Done);
        app.store.save_task(&task).unwrap();
        app.dispatch("task.archive", json!({"id": task.id}))
            .await
            .unwrap();
        assert!(app
            .dispatch("task.list", json!({}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());

        let result = app
            .dispatch("task.unarchive", json!({"id": task.id}))
            .await
            .unwrap();
        assert_eq!(result["archived"], false);

        let default_list = app.dispatch("task.list", json!({})).await.unwrap();
        let listed = default_list.as_array().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], task.id);
    }

    /// Like `test_app`, but writes `parallel: 1` before `App::new` reads
    /// settings -- `parallel_limit`/`slots` are fixed at construction time
    /// (a live `settings.set` only takes effect on the next restart), so
    /// this is the only way to get a single-slot app for a "second task
    /// holds the only slot" test.
    fn test_app_parallel_one() -> (Arc<App>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let store = Store::new(&data_dir).unwrap();
        let mut settings = store.load_settings().unwrap();
        settings.parallel = 1;
        store.save_settings(&settings).unwrap();
        let app = App::new(data_dir, dir.path().join("orchd.sock"), "orchd".to_string()).unwrap();
        (app, dir)
    }

    #[tokio::test]
    async fn task_archive_refuses_a_queued_task_with_a_live_loop() {
        let (app, _dir) = test_app_parallel_one();

        // A second task holds the app's only slot, so the task under test
        // stays parked at the top of `run_task_loop` (permit acquired
        // before anything else) instead of ever reaching the harness.
        let holder = task_with_status(TaskStatus::Running);
        app.store.save_task(&holder).unwrap();
        let held_permit = app.slots.clone().try_acquire_owned().unwrap();

        let task = task_with_status(TaskStatus::Queued);
        app.store.save_task(&task).unwrap();
        app.spawn_task_loop(task.id.clone(), true);
        assert!(
            app.controls.lock().unwrap().contains_key(&task.id),
            "spawn_task_loop registers its control entry synchronously"
        );

        let err = app
            .dispatch("task.archive", json!({"id": task.id}))
            .await
            .unwrap_err();
        assert!(err.contains("live loop"), "unexpected error: {err}");
        let reloaded = app.store.load_task(&task.id).unwrap().unwrap();
        assert!(!reloaded.archived);

        drop(held_permit);
    }

    #[tokio::test]
    async fn recover_on_start_skips_archived_tasks() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Queued);
        task.archived = true;
        app.store.save_task(&task).unwrap();

        app.recover_on_start().unwrap();

        assert!(
            !app.controls.lock().unwrap().contains_key(&task.id),
            "an archived task must not get a loop started on recovery"
        );
    }

    #[tokio::test]
    async fn task_start_refuses_an_archived_task() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Stopped);
        task.archived = true;
        app.store.save_task(&task).unwrap();

        let err = app
            .dispatch("task.start", json!({"id": task.id}))
            .await
            .unwrap_err();
        assert!(
            err.contains("archived") && err.contains("unarchive"),
            "unexpected error: {err}"
        );
        assert!(!app.controls.lock().unwrap().contains_key(&task.id));
    }
}
