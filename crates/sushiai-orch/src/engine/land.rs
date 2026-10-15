use crate::clock::now_ms;
use crate::engine::graph::landing::Landing;
use crate::engine::reports::record_landing;
use crate::engine::state::App;
use crate::engine::verification::scoping::split_deferred;
use crate::git::paths::short_sha;
use crate::harness::cancel::CancelToken;
use crate::policy::settings::AfterLand;
use crate::task::FailureKind;
use crate::task::Task;
use crate::task::TaskStatus;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// How often a task waiting `landing` is tried again.
const LANDING_RETRY: Duration = Duration::from_secs(120);
const AFTER_LAND_TIMEOUT: Duration = Duration::from_secs(600);
/// How many times a landing carries its work onto a branch that moved under
/// it before the attempt fails.
const MAX_RECARRIES: u32 = 3;
/// How long a landing waits for a dirty base checkout before it gives up and
/// leaves the work on the task branch (a pull request follows).
const DIRTY_LIMIT: Duration = Duration::from_secs(3600);

/// [`DIRTY_LIMIT`] in ms; `ORCHD_LANDING_DIRTY_LIMIT_MS` shortens it for tests.
fn dirty_limit_ms() -> i64 {
    std::env::var("ORCHD_LANDING_DIRTY_LIMIT_MS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DIRTY_LIMIT.as_millis() as i64)
}

pub(super) struct LandingRedirect {
    pub(super) branch: String,
    pub(super) done_tasks: Vec<Task>,
    pub(super) decisions: Vec<String>,
}

pub(super) fn resolve_landing_branch(
    branch: &str,
    tasks: &[Task],
) -> Result<LandingRedirect, String> {
    let mut branch = branch.to_string();
    let mut seen = std::collections::HashSet::new();
    let mut done_tasks = Vec::new();
    let mut decisions = Vec::new();
    loop {
        if !seen.insert(branch.clone()) {
            return Err(format!("Land: refused, a cycle was found while following done task branches from {branch}; the work stays on its own branch"));
        }
        let Some(done) = tasks
            .iter()
            .find(|t| t.status == TaskStatus::Done && t.branch == branch)
        else {
            break;
        };
        let Some(base) = done.base_ref.clone() else {
            return Err(format!("Land: refused, done task {} has no base branch to redirect to; the work stays on its own branch", done.title));
        };
        decisions.push(format!(
            "Land: redirected from done task {} ({})'s branch {branch} to {base}",
            done.title, done.id
        ));
        done_tasks.push(done.clone());
        branch = base;
    }
    Ok(LandingRedirect {
        branch,
        done_tasks,
        decisions,
    })
}

async fn mark_redirected_landed(
    app: &Arc<App>,
    task: &Task,
    branch: &str,
    sha: &str,
    redirects: &[Task],
) {
    for redirected in redirects.iter().filter(|done| done.landed_sha.is_none()) {
        let Ok(Some(mut done)) = app.store.load_task(&redirected.id) else {
            continue;
        };
        done.record_base_ref(Some(branch.to_string()));
        done.record_landed(Some(sha.to_string()));
        done.stamp_landing(now_ms());
        done.record_unlanded(None);
        done.record_decision(format!(
            "Land: task {} ({}) landed this work at {}",
            task.title,
            task.id,
            short_sha(sha)
        ));
        super::reports::refresh_diff_stat(&mut done).await;
        app.write_report(&mut done).await;
        done.touch(now_ms());
        let _ = app.store.save_task(&done);
        app.broadcast_task(&done);
    }
}

/// Lands a finished top-level task with `variant.land` on its base branch,
/// one task per (repo, branch) at a time: its work is carried onto the
/// branch's current head, squashed to one commit, checked again on exactly
/// that tree when the head had moved, and the branch moves to the commit.
/// `Landing::NoParent` and `Landing::DefaultBranchRefused` mean landing was
/// refused: the task commits on its own branch instead (and, being top level,
/// gets a pull request). A base checkout that stays dirty for an hour ends
/// the same way.
pub(super) async fn land_on_base(
    app: &Arc<App>,
    task: &mut Task,
    idx: Option<usize>,
    attempt_n: u32,
    worktree: &Path,
    run_dir: &Path,
    cancel: &CancelToken,
) -> Landing {
    let (land_on_default, after_land) = {
        let s = app.settings.read().unwrap();
        (s.may_land_on_default(&task.repo), s.after_land.clone())
    };
    let Some(original_branch) = task.base_ref.clone() else {
        task.record_decision(
            "Land: refused, the task was not started from a branch; the work stays on its own branch"
                .to_string(),
        );
        return Landing::NoParent;
    };
    let all = app.repo_tasks(&task.repo);
    let redirect = match resolve_landing_branch(&original_branch, &all) {
        Ok(redirect) => redirect,
        Err(reason) => {
            task.record_decision(reason);
            return Landing::NoParent;
        }
    };
    let branch = redirect.branch.clone();
    task.record_decisions(redirect.decisions.clone());
    let repo = PathBuf::from(&task.repo);
    if !land_on_default {
        let r = repo.clone();
        let default = tokio::task::spawn_blocking(move || crate::git::branches::default_branch(&r))
            .await
            .unwrap_or(None);
        if default.as_deref() == Some(branch.as_str()) {
            task.record_decision(format!(
                "Land: refused, {} The work stays on {}",
                crate::policy::delivery::land_on_default_refusal(&branch, &task.repo),
                task.branch
            ));
            return Landing::DefaultBranchRefused;
        }
    }

    if !redirect.done_tasks.is_empty() {
        if redirect
            .done_tasks
            .iter()
            .any(|done| done.landed_sha.is_none())
        {
            let (r, start, target) = (repo.clone(), task.base_sha.clone(), branch.clone());
            match tokio::task::spawn_blocking(move || {
                crate::git::history::merge_base_of(&r, &start, &target)
            })
            .await
            .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())))
            {
                Ok(base) => crate::task::transitions::move_base(task, base),
                Err(e) => {
                    task.record_decision(format!(
                        "Land: refused, could not find the merge base for redirected branch {branch}: {e}; the work stays on its own branch"
                    ));
                    return Landing::NoParent;
                }
            }
        }
        task.record_base_ref(Some(branch.clone()));
    }

    let lock = app.landing_lock(&format!("base\0{}\0{branch}", task.repo));
    let _landing = tokio::select! {
        _ = cancel.cancelled() => return Landing::Cancelled,
        guard = lock.lock_owned() => guard,
    };

    // The same split the final run used: a command deferred to a parent is
    // not run at landing either.
    let finals = {
        let settings = app.settings.read().unwrap();
        split_deferred(app, &settings, task).0
    };
    // The branch can move between the carry (and its checks) and the move
    // itself: someone else committed to it meanwhile. That is not the
    // agent's failure, so carry onto the new head and try again in place,
    // running the checks again on the new tree; only a conflict or a failing
    // check after a re-carry becomes a failed attempt.
    let mut recarries = 0;
    loop {
        let first = if recarries == 0 {
            "Another task"
        } else {
            "Another commit"
        };
        match crate::engine::graph::carry::CarryRun::new(app, task, idx, run_dir, cancel)
            .in_worktree(worktree)
            .onto(&branch, first, &finals)
            .await
        {
            Ok(true) => {
                let (r, b) = (repo.clone(), branch.clone());
                let landed = tokio::task::spawn_blocking(move || {
                    crate::git::history::resolve_commit(&r, &b).ok()
                })
                .await
                .unwrap_or(None);
                task.record_landed(landed);
                task.record_unlanded(None);
                if let Some(sha) = task.landed_sha.as_deref() {
                    mark_redirected_landed(app, task, &branch, sha, &redirect.done_tasks).await;
                }
                record_landing(task).await;
                return Landing::Landed;
            }
            Ok(false) => {}
            Err(landing) => return landing,
        }

        let (wt, r, b, title, tid, base) = (
            worktree.to_path_buf(),
            repo.clone(),
            branch.clone(),
            task.title.clone(),
            task.id.clone(),
            task.base_sha.clone(),
        );
        let moved = tokio::task::spawn_blocking(move || {
            crate::delivery::git::commit::commit(&wt, &title, &tid, attempt_n)?;
            let sha = crate::git::history::head_sha(&wt)?;
            let moved = crate::git::branches::move_branch(&r, &b, &sha);
            if !matches!(moved, Ok(crate::git::branches::BranchMove::Done { .. })) {
                // Keep the work, uncommitted, for the next try to carry.
                let _ = crate::delivery::git::carry::uncommit_to(&wt, &base);
            }
            moved.map(|m| (sha, m))
        })
        .await
        .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())));
        if !matches!(
            moved,
            Ok((_, crate::git::branches::BranchMove::Dirty { .. }))
        ) {
            task.end_dirty_wait();
        }
        match moved {
            Ok((sha, crate::git::branches::BranchMove::Done { checkout })) => {
                task.record_landed(Some(sha.clone()));
                task.record_unlanded(None);
                task.record_decision(format!("Land: landed on {branch} at {}", short_sha(&sha)));
                mark_redirected_landed(app, task, &branch, &sha, &redirect.done_tasks).await;
                record_landing(task).await;
                run_after_land(task, &checkout, &after_land).await;
                return Landing::Landed;
            }
            Ok((_, crate::git::branches::BranchMove::Dirty { checkout })) => {
                let first_wait = task.queue.dirty_since.is_none();
                let since = task.begin_dirty_wait(now_ms());
                if now_ms() - since >= dirty_limit_ms() {
                    task.end_dirty_wait();
                    let line = format!(
                        "Land: gave up, {} still has uncommitted changes; the work stays on {}",
                        checkout.display(),
                        task.branch
                    );
                    task.record_decision(line.clone());
                    let pr = app.settings.read().unwrap().may_open_pr_on_done(&task.repo);
                    let next = if pr {
                        "A pull request is opened from the task branch instead."
                    } else {
                        "Land it or open a pull request from the task branch."
                    };
                    crate::engine::delivery::pr::pending::notify_owner(
                        app,
                        &task.repo,
                        format!(
                            "Task \"{}\" could not land on {branch}: {line}. {next}",
                            task.title
                        ),
                    );
                    return Landing::NoParent;
                }
                if first_wait {
                    task.record_decision(format!(
                        "Land: waiting, {} has uncommitted changes; retrying every 2 minutes and on task.start",
                        checkout.display()
                    ));
                }
                return Landing::Waiting;
            }
            Ok((_, crate::git::branches::BranchMove::Diverged)) if recarries < MAX_RECARRIES => {
                recarries += 1;
                task.record_decision(format!(
                    "Land: {branch} moved while landing; carrying the work onto it again (try {recarries} of {MAX_RECARRIES})"
                ));
            }
            Ok((_, crate::git::branches::BranchMove::Diverged)) => {
                return Landing::Failed {
                    kind: FailureKind::Error,
                    detail: format!(
                        "{branch} moved while landing, {MAX_RECARRIES} times in a row; carry the work onto it again."
                    ),
                };
            }
            Err(e) => {
                return Landing::Failed {
                    kind: FailureKind::Error,
                    detail: format!("Could not land on {branch}: {e}"),
                };
            }
        }
    }
}

/// Runs the `settings.afterLand` commands for the task's repo in its main
/// checkout. Each exit code is a decision line; a failure never un-lands.
async fn run_after_land(task: &mut Task, checkout: &Path, entries: &[AfterLand]) {
    let same_repo = |entry: &Path| {
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(entry) == canon(Path::new(&task.repo))
    };
    let entries: Vec<&AfterLand> = entries
        .iter()
        .filter(|e| same_repo(Path::new(&e.repo)))
        .collect();
    for entry in entries {
        let run = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&entry.run)
            .current_dir(checkout)
            .kill_on_drop(true)
            .output();
        let line = match tokio::time::timeout(AFTER_LAND_TIMEOUT, run).await {
            Ok(Ok(out)) => {
                let code = out.status.code();
                let code = code.map_or_else(|| "a signal".to_string(), |c| c.to_string());
                let mut line = format!("After land: `{}` exited {code}", entry.run);
                if !out.status.success() {
                    let err = String::from_utf8_lossy(&out.stderr);
                    let tail = err.trim();
                    let start = tail.len().saturating_sub(300);
                    let start = (start..=tail.len())
                        .find(|i| tail.is_char_boundary(*i))
                        .unwrap_or(tail.len());
                    if !tail[start..].is_empty() {
                        line.push_str(&format!(": {}", &tail[start..]));
                    }
                }
                line
            }
            Ok(Err(e)) => format!("After land: `{}` could not start ({e})", entry.run),
            Err(_) => format!(
                "After land: `{}` timed out after {} seconds",
                entry.run,
                AFTER_LAND_TIMEOUT.as_secs()
            ),
        };
        task.record_decision(line);
    }
}

impl App {
    /// Starts the loop of every `landing` task that has none: called every
    /// [`LANDING_RETRY`], on daemon start and by `task.start`.
    pub(super) fn retry_landings(&self) {
        for t in self.store.list_tasks().unwrap_or_default() {
            if t.status == TaskStatus::Landing && !t.archived {
                self.spawn_task_loop(t.id.clone(), true);
            }
        }
    }

    /// The 2-minute retry of tasks waiting `landing`; ends with the daemon.
    pub(super) fn spawn_landing_retries(&self) {
        let app = self.arc();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(LANDING_RETRY);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if app.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                app.retry_landings();
            }
        });
    }
}
