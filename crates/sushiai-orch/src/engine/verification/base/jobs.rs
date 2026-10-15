use crate::engine::state::App;
use crate::engine::verification::base::checks::flaky_base_check_line;
use crate::engine::verification::base::run::run_on_base;
use crate::engine::verification::context::CheckContext;
use crate::harness::cancel::CancelToken;
use crate::task::attempt::VerifyOutcome;
use crate::verify::base_cache::save_base_runs;
use crate::verify::passes::is_interrupted;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone)]
pub(in crate::engine) struct BaseCheckJob {
    pub(super) task_id: Option<String>,
    pub(super) repo: String,
    pub(super) base_sha: String,
    pub(super) run_dir: PathBuf,
    pub(super) background: bool,
}

impl BaseCheckJob {
    pub(in crate::engine) fn new(
        task_id: Option<&str>,
        repo: &str,
        base_sha: &str,
        run_dir: &Path,
        background: bool,
    ) -> Self {
        Self {
            task_id: task_id.map(str::to_string),
            repo: repo.to_string(),
            base_sha: base_sha.to_string(),
            run_dir: run_dir.to_path_buf(),
            background,
        }
    }
}

/// Waits for the turn of `repo`'s base-check pairs; `None` when cancelled.
/// Two lanes per repo: blocking runs (a parent's `verify`, a carried
/// attempt's lookups) and background `finalVerify` jobs. Each lane runs one
/// pair at a time; a slow final never queues a blocking run behind it.
async fn lock_base_checks(
    app: &App,
    repo: &str,
    background: bool,
    cancel: &CancelToken,
) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    let key = if background {
        format!("{repo}\u{0}background")
    } else {
        repo.to_string()
    };
    let lock = app
        .base_check_locks
        .lock()
        .unwrap()
        .entry(key)
        .or_default()
        .clone();
    tokio::select! {
        guard = lock.lock_owned() => Some(guard),
        _ = cancel.cancelled() => None,
    }
}

pub(in crate::engine) type BaseJobReceivers = HashMap<(String, String), watch::Receiver<bool>>;

/// Starts one background job for the pairs of `commands` on `base_sha` that
/// are neither cached nor in flight; their run folders go under `run_dir`
/// (`preflight-base`, `preflight-base-retry`). The job is the run plus the
/// rerun of a command that exits non-zero, past the cache and with no other
/// base-check pair of its repo beside it, so a load-flaky test does not
/// count. It runs under the app's own `base_cancel` (a task stop does not
/// end it: another task may wait on the same job), writes its results into
/// `base_runs` and leaves `base_jobs` when done. A cancelled job caches
/// nothing. `background` jobs (finals) take the repo's background lane,
/// the others the blocking lane. Returns receipts for new and in-flight jobs.
pub(in crate::engine) fn start_base_jobs(
    app: &Arc<App>,
    job: BaseCheckJob,
    commands: &[String],
) -> BaseJobReceivers {
    let base_sha = &job.base_sha;
    let mut todo: Vec<String> = Vec::new();
    let mut receipts = HashMap::new();
    let (tx, rx) = watch::channel(false);
    {
        // Jobs first, then the cache: a job caches before it leaves the
        // registry, so a pair is never missed in between.
        let mut jobs = app.base_jobs.lock().unwrap();
        let cache = app.base_runs.lock().unwrap();
        for c in commands {
            let key = (base_sha.to_string(), c.clone());
            if let Some(job) = jobs.get(&key) {
                receipts.insert(key, job.clone());
                continue;
            }
            if cache.contains_key(&key) {
                continue;
            }
            receipts.insert(key.clone(), rx.clone());
            jobs.insert(key, rx.clone());
            todo.push(c.clone());
        }
    }
    if todo.is_empty() {
        return receipts;
    }
    let app = app.clone();
    // Leaves the registry and wakes the waiters even when the job panics.
    struct Leave {
        app: Arc<App>,
        base_sha: String,
        todo: Vec<String>,
        tx: watch::Sender<bool>,
    }
    impl Drop for Leave {
        fn drop(&mut self) {
            if self.app.base_cancel.is_cancelled() {
                let mut cache = self.app.base_runs.lock().unwrap();
                for c in &self.todo {
                    cache.remove(&(self.base_sha.clone(), c.clone()));
                }
            }
            let mut jobs = self.app.base_jobs.lock().unwrap();
            for c in &self.todo {
                jobs.remove(&(self.base_sha.clone(), c.clone()));
            }
            drop(jobs);
            let _ = self.tx.send(true);
        }
    }
    tokio::spawn(async move {
        let _leave = Leave {
            app: app.clone(),
            base_sha: job.base_sha.clone(),
            todo: todo.clone(),
            tx,
        };
        run_base_job(&app, job, &todo).await;
    });
    receipts
}

async fn run_base_job(app: &Arc<App>, job: BaseCheckJob, commands: &[String]) {
    let task_id = job.task_id.as_deref();
    let repo = job.repo.as_str();
    let base_sha = job.base_sha.as_str();
    let dir = job.run_dir.as_path();
    let background = job.background;
    let cancel = app.base_cancel.clone();
    let Some(_alone) = lock_base_checks(app, repo, background, &cancel).await else {
        return;
    };
    let fails = |r: &VerifyOutcome| r.code.is_some_and(|c| c != 0) && !is_interrupted(r);
    let Ok(first) = run_on_base(
        app,
        crate::engine::verification::base::jobs::BaseCheckJob::new(
            task_id,
            repo,
            base_sha,
            &dir.join("preflight-base"),
            false,
        ),
        commands,
        &cancel,
    )
    .await
    else {
        return;
    };
    let failed: Vec<String> = first
        .iter()
        .filter(|r| fails(r))
        .map(|r| r.command.clone())
        .collect();
    if failed.is_empty() || cancel.is_cancelled() {
        return;
    }
    {
        let mut cache = app.base_runs.lock().unwrap();
        for c in &failed {
            cache.remove(&(base_sha.to_string(), c.clone()));
        }
        save_base_runs(&app.data_dir, &cache);
    }
    let Ok(second) = run_on_base(
        app,
        crate::engine::verification::base::jobs::BaseCheckJob::new(
            task_id,
            repo,
            base_sha,
            &dir.join("preflight-base-retry"),
            false,
        ),
        &failed,
        &cancel,
    )
    .await
    else {
        return;
    };
    for c in &failed {
        if second
            .iter()
            .any(|r| &r.command == c && r.code == Some(0) && !is_interrupted(r))
        {
            app.base_flaky
                .lock()
                .unwrap()
                .insert((base_sha.to_string(), c.clone()));
            let line = flaky_base_check_line(c, base_sha);
            app.base_run_decisions
                .lock()
                .unwrap()
                .entry((base_sha.to_string(), c.clone()))
                .or_default()
                .push(line.clone());
            CheckContext::new(app, task_id).decision(line, &mut Vec::new());
        }
    }
}

/// Waits for the in-flight job of the pair (if any), then reads the cache.
/// `None`: the job was cancelled, hit an environment failure or could not
/// check out the base, or `cancel` fired while waiting.
pub(in crate::engine) async fn wait_for_base(
    app: &Arc<App>,
    base_sha: &str,
    command: &str,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    let key = (base_sha.to_string(), command.to_string());
    let job = app.base_jobs.lock().unwrap().get(&key).cloned();
    wait_for_base_job(app, base_sha, command, job, cancel).await
}

pub(in crate::engine) async fn wait_for_base_job(
    app: &Arc<App>,
    base_sha: &str,
    command: &str,
    job: Option<watch::Receiver<bool>>,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    if let Some(mut rx) = job {
        tokio::select! {
            // An `Err` is a dropped sender: the job is over either way.
            _ = rx.wait_for(|done| *done) => {}
            _ = cancel.cancelled() => return None,
        }
    }
    app.base_runs
        .lock()
        .unwrap()
        .get(&(base_sha.to_string(), command.to_string()))
        .cloned()
}

#[cfg(test)]
mod tests;
