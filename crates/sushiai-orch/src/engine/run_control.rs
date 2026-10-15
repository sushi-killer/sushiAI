use crate::engine::questions::reply::Reply;
use crate::engine::state::App;
use crate::git::own_diff::OwnDiff;
use crate::harness::cancel::CancelToken;
use crate::harness::run_files::RunFiles;
use crate::harness::watch::HookClock;
use crate::store;
use crate::task::amendment::PendingAmendment;
use crate::task::Task;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::oneshot;
#[derive(Clone)]
pub(super) struct TaskSignals {
    pub(super) cancel: CancelToken,
    pub(super) resume: Arc<tokio::sync::Notify>,
    /// One-shot per question: created fresh by `wait_for_answer` each time
    /// the loop actually waits, taken (and consumed) by `task.answer`. No
    /// long-lived queue -- an answer that arrives when nobody is waiting is
    /// simply not delivered (`task.answer` rejects it before it gets here).
    pub(super) pending_answer: Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    /// Mirror of the task's durable pending amendment, consumed at a safe boundary.
    pub(super) pending_amend: Arc<StdMutex<Option<PendingAmendment>>>,
}

impl TaskSignals {
    pub(super) fn new() -> Self {
        Self {
            cancel: CancelToken::new(),
            resume: Arc::new(tokio::sync::Notify::new()),
            pending_answer: Arc::new(StdMutex::new(None)),
            pending_amend: Arc::new(StdMutex::new(None)),
        }
    }
}

pub(super) struct TaskControl {
    pub(super) signals: TaskSignals,
    pub(super) handle: tokio::task::JoinHandle<()>,
}

pub(super) struct HookContext {
    pub(super) task_id: String,
    pub(super) attempt_n: u32,
    pub(super) repo: String,
    pub(super) worktree: PathBuf,
    pub(super) base_sha: String,
    /// The agent's own work: the base without the planner's files.
    pub(super) own: OwnDiff,
    pub(super) verify: Vec<String>,
    pub(super) blocks: AtomicU32,
    /// Cancelled when the attempt's run ends, so a Stop-hook verify still
    /// running for a killed (stalled) agent stops before the next attempt.
    pub(super) cancel: CancelToken,
    /// Set while `hook.stop` runs verify for this attempt: the harness is
    /// silent then by design, so the stall clock waits.
    pub(super) hook_running: Arc<HookClock>,
    /// Rules the owner allowed for this run only (`permissions.rs`).
    pub(super) run_grants: StdMutex<std::collections::HashSet<String>>,
    /// Rules the owner denied during this run.
    pub(super) run_denied: StdMutex<std::collections::HashSet<String>>,
    /// Tools and paths the hook decided with the owner, so a refusal of the
    /// same one after the run is not asked about again.
    pub(super) handled: StdMutex<std::collections::BTreeSet<String>>,
    /// Protected paths the owner allowed: what the agent staged there is
    /// copied to the real path.
    pub(super) staged: StdMutex<std::collections::BTreeSet<String>>,
    /// One permission question at a time per run.
    pub(super) ask_lock: tokio::sync::Mutex<()>,
}

impl HookContext {
    pub(super) fn new(task: &Task, attempt_n: u32, worktree: &Path, base_sha: &str) -> Arc<Self> {
        Arc::new(HookContext {
            task_id: task.id.clone(),
            attempt_n,
            repo: task.repo.clone(),
            worktree: worktree.to_path_buf(),
            base_sha: base_sha.to_string(),
            own: task.own_diff(),
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

impl App {
    /// Registers `token` for a run's Stop hook and keeps it in the run's token
    /// file, so a restarted daemon can register it again for a run it re-adopts.
    pub(super) fn register_hook(&self, token: &str, ctx: &Arc<HookContext>, run_dir: &Path) {
        let file = RunFiles::of(&run_dir.join("events.jsonl")).token;
        let _ = store::files::write_secret_file(&file, token);
        self.hook_tokens
            .write()
            .unwrap()
            .insert(token.to_string(), ctx.clone());
    }
}

impl App {
    /// Forgets a token in memory only; its file stays for the next daemon.
    pub(super) fn unload_hook(&self, token: &str) {
        self.hook_tokens.write().unwrap().remove(token);
    }
}

impl App {
    /// Forgets the token of a run that ended, in memory and on disk.
    pub(super) fn forget_hook(&self, token: &str, run_dir: &Path) {
        self.unload_hook(token);
        let _ = std::fs::remove_file(RunFiles::of(&run_dir.join("events.jsonl")).token);
    }
}

impl App {
    /// The hook token registered for attempt `n` of a task, if its run has one.
    pub(super) fn hook_token_of(&self, task_id: &str, n: u32) -> Option<String> {
        let tokens = self.hook_tokens.read().unwrap();
        tokens
            .iter()
            .find(|(_, ctx)| ctx.task_id == task_id && ctx.attempt_n == n)
            .map(|(token, _)| token.clone())
    }
}
