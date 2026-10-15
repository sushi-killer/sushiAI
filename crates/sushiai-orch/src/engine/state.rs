use crate::api::events::Event;
use crate::engine::attribution::Attribution;
use crate::engine::attribution::AttributionKey;
use crate::engine::fresh_base;
use crate::engine::leases::Lease;
use crate::engine::messages;
use crate::engine::questions::reply::Reply;
use crate::engine::run_control::HookContext;
use crate::engine::run_control::TaskControl;
use crate::engine::secrets::Secrets;
use crate::engine::slots::ResizableSlots;
use crate::engine::verification::cache::CachedVerification;
use crate::engine::verification::healing::preflight::PreflightJob;
use crate::harness::cancel::CancelToken;
use crate::policy::routing::Harness;
use crate::policy::settings::Settings;
use crate::store::Store;
use crate::task::messages::Message;
use crate::task::AnsweredBy;
use crate::task::VerifyOutcome;
use crate::verify::command::set_verify_timeout_secs;
use crate::verify::passes::PassedChecks;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use std::sync::RwLock;
use tokio::sync::broadcast;
use tokio::sync::oneshot;
use tokio::sync::watch;
pub(crate) struct App {
    pub store: Store,
    pub data_dir: PathBuf,
    /// The sushiAI home (`SUSHIAI_HOME`): where agents' `sushiai mcp` and
    /// `sushiai orch hook` find the daemon.
    pub home: PathBuf,
    /// The `sushiai` executable that agents run as `mcp` and `orch hook`.
    pub exe_path: String,
    pub(super) settings: RwLock<Settings>,
    pub(super) secrets: RwLock<Secrets>,
    pub(super) events_tx: broadcast::Sender<Event>,
    pub(super) controls: std::sync::Mutex<HashMap<String, TaskControl>>,
    /// Keyed by repo: the orchestrator chat turn running for it, if any.
    pub(super) chat_turns: std::sync::Mutex<HashMap<String, CancelToken>>,
    /// How long an overseer turn may run before it is cancelled (ms).
    pub(super) overseer_turn_timeout_ms: std::sync::atomic::AtomicU64,
    /// How long any task helper may run (ms).
    pub(super) helper_timeout_ms: std::sync::atomic::AtomicU64,
    /// Test override of the planner limit in ms; `0` = use the setting.
    pub(super) plan_timeout_override_ms: std::sync::atomic::AtomicU64,
    /// How long fetching a task's base branch may take (ms).
    pub(super) base_fetch_timeout_ms: std::sync::atomic::AtomicU64,
    /// Keyed by audit id: the `repo.audit` runs still in flight.
    pub(super) audits: std::sync::Mutex<HashMap<String, CancelToken>>,
    /// Every agent-to-agent message, mirrored to `<data>/messages.json`.
    pub(super) messages: StdMutex<Vec<Message>>,
    pub(super) hook_tokens: RwLock<HashMap<String, Arc<HookContext>>>,
    /// Keyed by task id: the held tool call a `task.answer` resolves.
    pub(super) permission_waits: StdMutex<HashMap<String, oneshot::Sender<Reply>>>,
    /// Callers who just answered a task's parked question; the loop consumes
    /// the provenance after recording the answer.
    pub(super) agent_answers: StdMutex<HashMap<String, AnsweredBy>>,
    pub(super) cancellations:
        StdMutex<HashMap<String, crate::task::queue::cancellation::Cancellation>>,
    /// Keyed by repo: overseer wakes skipped while its chat was busy; each
    /// fires when that repo's turn ends.
    pub(super) deferred_wakes:
        StdMutex<HashMap<String, Vec<crate::engine::chat::wake::WakeRequest>>>,
    /// Keyed by task id: the files each live task holds (`leases.rs`).
    pub(super) leases: StdMutex<HashMap<String, Lease>>,
    /// Held while a graph start counts a parent's running children and
    /// spawns one more, so `childParallel` is never exceeded by a race.
    pub(super) child_admission: StdMutex<()>,
    /// Held while the autopilot counts live loops and starts backlog tasks,
    /// so `parallel` is never exceeded by a race.
    pub(super) autopilot_admission: StdMutex<()>,
    /// Keyed by task id: the diff+untracked-list hash a verify run was last
    /// computed for, and its completed results, shared by `hook.stop` and
    /// the post-session gate. Interrupted result sets are never stored.
    pub(super) verify_cache: StdMutex<HashMap<String, CachedVerification>>,
    /// Keyed by (base sha, command): what a command did on that commit, so an
    /// unchanged base is never checked twice for the same command. Kept on
    /// disk (`base-results.json`), so a restart does not run them again; an
    /// interrupted, cancelled or environment-failed run is never stored.
    pub(super) base_runs: std::sync::Mutex<HashMap<(String, String), VerifyOutcome>>,
    /// The attribution turn's verdict per (base sha, command, run key, base
    /// run key); a failed turn is kept as `None`. Memory only.
    pub(super) attributions: StdMutex<HashMap<AttributionKey, Option<Attribution>>>,
    /// Exit-0 checks on exact worktree trees, in insertion order, memory only.
    pub(super) gate_passes: StdMutex<PassedChecks>,
    /// Retry decisions from background base runs, keyed by (base sha, command).
    pub(super) base_run_decisions: StdMutex<HashMap<(String, String), Vec<String>>>,
    /// Keyed by (base sha, command): background base runs in flight. The
    /// receiver flips to `true` when the job is over; its result, if any, is
    /// then in `base_runs`. Concurrent lookups share the same job.
    pub(super) base_jobs: StdMutex<HashMap<(String, String), watch::Receiver<bool>>>,
    /// Final-check job receipts for each live task, including jobs that
    /// finished without a base result. A final lookup waits for its original
    /// job instead of starting another one just because nothing was cached.
    pub(super) final_base_jobs:
        StdMutex<HashMap<String, crate::engine::verification::base::jobs::BaseJobReceivers>>,
    /// Pairs whose first base run failed and whose rerun passed (load-flaky).
    pub(super) base_flaky: StdMutex<std::collections::HashSet<(String, String)>>,
    /// Cancelled by `shutdown_for` only: a task stop must not end a job
    /// another task may be waiting on.
    pub(super) base_cancel: CancelToken,
    /// Keyed by task id: the task's base preflight, a background job that
    /// holds no slot (`healing.rs`). The loop applies its result.
    pub(super) base_preflights: StdMutex<HashMap<String, Arc<PreflightJob>>>,
    /// Which harness CLIs the last probe found (`availability.rs`).
    pub(super) harness_avail: StdMutex<HashMap<Harness, bool>>,
    /// Tasks the timer already resumed from a missing harness; a spawn that
    /// fails again asks the owner instead of resuming every tick.
    pub(super) harness_resumed: StdMutex<std::collections::HashSet<String>>,
    /// Live task-loop limit. Shrinking retires free permits first, then
    /// held permits as their owners finish.
    pub(super) slots: Arc<ResizableSlots>,
    /// Shared by all check runners on this daemon, independent of agent slots.
    pub(super) check_slots: Arc<ResizableSlots>,
    pub(super) check_load: RwLock<fn() -> (f64, u32)>,
    /// Hook and background check lines awaiting the loop's own Task copy.
    pub(super) check_decisions: StdMutex<HashMap<String, Vec<String>>>,
    /// Keyed by parent task id: held while one child lands on the parent's
    /// branch, so landings onto one branch happen one at a time.
    pub(super) landing_locks: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pub(super) task_locks: StdMutex<HashMap<String, Arc<StdMutex<()>>>>,
    pub(super) recovery_timers_started: AtomicBool,
    /// Keyed by repo: one base-commit check runs at a time per repo, so a
    /// rerun of a failed base check never competes with another one.
    pub(super) base_check_locks: StdMutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Set by `shutdown()`: the loops it cancels end `stopped`, and that
    /// must not read as a dependency ending.
    pub(super) shutting_down: AtomicBool,
    /// Set with `shutting_down` when the module is switched off: no agent run outlives it.
    pub(super) disabling: AtomicBool,
    /// Held across the check-and-append of evolution signal detection so one
    /// task is never recorded twice.
    pub(super) evolution_lock: tokio::sync::Mutex<()>,
    /// Keyed by cluster key: the `evolution.run` proposer runs still in
    /// flight.
    pub(super) proposals: std::sync::Mutex<HashMap<String, CancelToken>>,
    /// Held across a read-modify-write of a stored proposal.
    pub(super) proposal_lock: tokio::sync::Mutex<()>,
    /// Held across a read-modify-write of `repo-notes.json`.
    pub(super) notes_lock: tokio::sync::Mutex<()>,
    pub(super) self_ref: OnceLock<std::sync::Weak<App>>,
}

impl App {
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
            plan_timeout_override_ms: std::sync::atomic::AtomicU64::new(0),
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
            base_runs: std::sync::Mutex::new(crate::verify::base_cache::load_base_runs(
                &base_runs_dir,
            )),
            attributions: StdMutex::new(HashMap::new()),
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
            check_load: RwLock::new(crate::verify::load::machine_load),
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
}

impl App {
    pub(super) fn arc(&self) -> Arc<App> {
        self.self_ref
            .get()
            .and_then(|w| w.upgrade())
            .expect("App is always constructed behind an Arc")
    }
}
