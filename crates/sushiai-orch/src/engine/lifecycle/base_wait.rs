use crate::clock::now_ms;
use crate::engine::lifecycle::carry::carry_onto_newest_base;
use crate::engine::lifecycle::carry::BaseCarry;
use crate::engine::questions::parking::wait_for_answer;
use crate::engine::questions::reply::Reply;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::git::paths::short_sha;
use crate::harness::cancel::CancelToken;
use crate::task::is_option;
use crate::task::AskedBy;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::Task;
use crate::task::TaskStatus;
use crate::task::Wait;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::oneshot;

/// How often a task that waits for its base branch to move looks at it.
const BASE_MOVE_POLL: Duration = Duration::from_secs(1);

/// How often that wait fetches the base from `origin`: the base often moves
/// only by a merge on the remote, which the local branch never sees itself.
const BASE_FETCH_EVERY: Duration = Duration::from_secs(60);

/// How many times a failed carry is retried before the agent gets it as a
/// conflict to fix.
const CARRY_RETRIES: u32 = 3;

/// The pause before a failed carry is tried again; `ORCHD_CARRY_RETRY_MS`
/// shortens it for tests.
fn carry_retry_pause() -> Duration {
    let ms = std::env::var("ORCHD_CARRY_RETRY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15_000);
    Duration::from_millis(ms)
}

/// The longest a task waits for its base to move, in seconds; after it the
/// baseline rule judges the check against the base as it is. A test run may
/// shorten it with `ORCHD_BASE_WAIT_SECS`.
const BASE_WAIT_MAX_SECS: u64 = 2 * 3600;

/// Starts the `wait` title of a task that waits for its base to move (the UI
/// shows a dependency wait as "after <title>").
pub(in crate::engine) const BASE_WAIT_TITLE: &str = "a new commit on ";

fn base_wait_max() -> Duration {
    let secs = std::env::var("ORCHD_BASE_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(BASE_WAIT_MAX_SECS);
    Duration::from_secs(secs)
}

/// Whether `task` sits in the bounded wait for its base to move: it can move
/// nothing itself, so it is no reason for another task to wait.
pub(in crate::engine) fn waits_for_base_move(task: &Task) -> bool {
    matches!(
        &task.queue.wait,
        Some(Wait::Dependency { titles }) if titles.iter().any(|t| t.starts_with(BASE_WAIT_TITLE))
    )
}

/// Ends the wait on `base_sha` for good; the decision line is a log.
fn record_base_waited_out(task: &mut Task, base_sha: &str) {
    task.record_waited_out_base(base_sha.to_string());
    task.record_decision(format!(
        "Orchestrator: waited out base {base_sha}; not waiting for it to move again"
    ));
}

/// Whether the wait on `base_sha` already ran out its bound.
pub(in crate::engine) fn base_waited_out(task: &Task, base_sha: &str) -> bool {
    task.loop_state
        .waited_out_bases
        .iter()
        .any(|b| b == base_sha)
}

/// Fetches `origin/<base_ref>` and fast-forwards the local branch to it (only
/// in a clean checkout, never a diverged branch). Best effort: offline or no
/// `origin` leaves the branch as it is.
async fn follow_origin(repo: &Path, base_ref: &str) {
    if crate::delivery::pr::command::run_cmd(
        "git",
        &["fetch", "--quiet", "origin", base_ref],
        repo,
        Duration::from_secs(60),
    )
    .await
    .is_err()
    {
        return;
    }
    let (r, b) = (repo.to_path_buf(), base_ref.to_string());
    let _ = tokio::task::spawn_blocking(move || {
        let sha = crate::git::history::resolve_commit(&r, &format!("refs/remotes/origin/{b}"))?;
        crate::git::branches::move_branch(&r, &b, &sha)
    })
    .await;
}

pub(in crate::engine) enum BaseRetry {
    Ready,
    Conflict(String),
    Stopped,
}

/// Waits for the base to move and carries onto it. A failed carry stays on
/// an owner question until it can be retried, rather than implementing on
/// the old base. Conflicts are left for a conflict-only implement attempt.
pub(in crate::engine) async fn carry_after_base_moves(
    app: &Arc<App>,
    task: &mut Task,
    worktree: &Path,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> BaseRetry {
    let mut failed_carries = 0;
    loop {
        if let Some(base_ref) = task.base_ref.clone() {
            let mut noted = false;
            let mut fetched_at: Option<tokio::time::Instant> = None;
            let waited_from = tokio::time::Instant::now();
            let resume_status = task.status;
            let mut timed_out = false;
            loop {
                if fetched_at.is_none_or(|at| at.elapsed() >= BASE_FETCH_EVERY) {
                    fetched_at = Some(tokio::time::Instant::now());
                    follow_origin(Path::new(&task.repo), &base_ref).await;
                }
                let (wt, r) = (worktree.to_path_buf(), base_ref.clone());
                let now = tokio::task::spawn_blocking(move || {
                    crate::git::history::resolve_commit(&wt, &r)
                })
                .await
                .ok()
                .and_then(|r| r.ok());
                if now.as_deref().is_none_or(|sha| sha != task.base_sha) {
                    break;
                }
                if waited_from.elapsed() >= base_wait_max() {
                    timed_out = true;
                    break;
                }
                if !noted {
                    noted = true;
                    permit.take();
                    // Shown as waiting, with its reason; the claim on files
                    // goes, so a task that waits blocks no one.
                    app.leases.lock().unwrap().remove(&task.id);
                    task.resume_phase(TaskStatus::Queued, task.updated_at);
                    task.set_wait(Some(Wait::Dependency {
                        titles: vec![format!(
                            "{BASE_WAIT_TITLE}{base_ref} (retrying; waits at most {} min)",
                            base_wait_max().as_secs() / 60
                        )],
                    }));
                    task.record_decision(format!(
                        "Orchestrator: waiting for {base_ref} to move past {} before the retry",
                        short_sha(&task.base_sha)
                    ));
                    task.touch(now_ms());
                    app.save_task_logged(task);
                    app.broadcast_task(task);
                    app.advance_queue(&task.repo);
                }
                tokio::select! {
                    _ = cancel.cancelled() => return BaseRetry::Stopped,
                    _ = tokio::time::sleep(BASE_MOVE_POLL) => {}
                }
            }
            if noted {
                task.set_wait(None);
                task.resume_phase(resume_status, task.updated_at);
            }
            if timed_out {
                task.record_decision(format!(
                    "Orchestrator: {base_ref} did not move past {} in {} min; the check is judged against the base as it is",
                    short_sha(&task.base_sha),
                    base_wait_max().as_secs() / 60
                ));
                record_base_waited_out(task, &task.base_sha.clone());
            }
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
        }
        if cancel.is_cancelled() {
            return BaseRetry::Stopped;
        }
        match carry_onto_newest_base(app, task, worktree, cancel).await {
            BaseCarry::Ready => return BaseRetry::Ready,
            BaseCarry::Stale => continue,
            BaseCarry::Stopped => return BaseRetry::Stopped,
            BaseCarry::Conflict(detail) => return BaseRetry::Conflict(detail),
            BaseCarry::Skipped(reason) => {
                task.ask(
                    Question::new(
                        format!("{reason}. Fix the carry problem, then retry, or stop?"),
                        vec!["retry".to_string(), "stop".to_string()],
                        QuestionKind::AgentQuestion,
                        AskedBy::Verify,
                        crate::clock::now_ms(),
                    ),
                    task.updated_at,
                );
                task.resume_phase(TaskStatus::Waiting, task.updated_at);
                task.touch(now_ms());
                let _task_id = task.id.clone();
                let Some(answer) = wait_for_answer(app, task, pending_answer, cancel, permit).await
                else {
                    return BaseRetry::Stopped;
                };
                if is_option(&answer, "stop") {
                    task.resume_phase(TaskStatus::Stopped, task.updated_at);
                    task.touch(now_ms());
                    app.save_task_logged(task);
                    app.broadcast_task(task);
                    return BaseRetry::Stopped;
                }
            }
            BaseCarry::Failed(reason) => {
                failed_carries += 1;
                if failed_carries > CARRY_RETRIES {
                    return BaseRetry::Conflict(format!(
                        "Carrying the work onto the newest base failed {failed_carries} times: {reason}. Bring the work up to date with the base yourself."
                    ));
                }
                task.record_decision(format!(
                    "Orchestrator: the carry failed ({failed_carries} of {CARRY_RETRIES} retries), trying again after the next fetch"
                ));
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
                tokio::select! {
                    _ = cancel.cancelled() => return BaseRetry::Stopped,
                    _ = tokio::time::sleep(carry_retry_pause()) => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod follow_origin_tests {
    use super::*;
    use crate::engine::test_support::{git_in, init_git_repo};

    #[tokio::test]
    async fn a_base_merged_on_origin_moves_the_local_branch_only_by_fast_forward() {
        let dir = tempfile::tempdir().unwrap();
        let seed = dir.path().join("seed");
        std::fs::create_dir(&seed).unwrap();
        init_git_repo(&seed);
        git_in(&seed, &["branch", "-M", "main"]);
        let origin = dir.path().join("origin.git");
        git_in(
            dir.path(),
            &[
                "clone",
                "-q",
                "--bare",
                seed.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        let repo = dir.path().join("repo");
        git_in(
            dir.path(),
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                repo.to_str().unwrap(),
            ],
        );
        git_in(&repo, &["config", "user.email", "test@example.com"]);
        git_in(&repo, &["config", "user.name", "Test"]);
        // The owner merges a fix on the remote; the local main never sees it.
        std::fs::write(seed.join("fix.txt"), "fixed\n").unwrap();
        git_in(&seed, &["add", "."]);
        git_in(&seed, &["commit", "-q", "-m", "fix the base"]);
        git_in(&seed, &["push", "-q", origin.to_str().unwrap(), "main"]);
        let merged = git_in(&seed, &["rev-parse", "HEAD"]);
        assert_ne!(git_in(&repo, &["rev-parse", "main"]), merged);

        follow_origin(&repo, "main").await;
        assert_eq!(git_in(&repo, &["rev-parse", "main"]), merged);
        assert!(
            repo.join("fix.txt").exists(),
            "the clean checkout moved too"
        );

        // A local main with its own commit is diverged: left alone.
        std::fs::write(repo.join("local.txt"), "mine\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "local"]);
        let local = git_in(&repo, &["rev-parse", "main"]);
        std::fs::write(seed.join("next.txt"), "next\n").unwrap();
        git_in(&seed, &["add", "."]);
        git_in(&seed, &["commit", "-q", "-m", "next"]);
        git_in(&seed, &["push", "-q", origin.to_str().unwrap(), "main"]);
        follow_origin(&repo, "main").await;
        assert_eq!(git_in(&repo, &["rev-parse", "main"]), local);
    }
}

#[cfg(test)]
mod carry_retry_tests {
    use super::*;
    use crate::engine::test_support::{task_with_status, test_app};

    #[tokio::test(start_paused = true)]
    async fn a_carry_that_keeps_failing_becomes_a_conflict_attempt_and_asks_nobody() {
        let (app, dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = dir.path().to_string_lossy().into_owned();
        task.fixture_data().base_ref = Some("main".into());
        task.fixture_data().worktree = dir.path().join("missing").to_string_lossy().into_owned();
        app.store.save_task(&task).unwrap();
        let worktree = PathBuf::from(&task.worktree);
        let pending = Arc::new(StdMutex::new(None));
        let result = carry_after_base_moves(
            &app,
            &mut task,
            &worktree,
            &pending,
            &CancelToken::new(),
            &mut None,
        )
        .await;
        match result {
            BaseRetry::Conflict(detail) => assert!(detail.contains("failed 4 times"), "{detail}"),
            _ => panic!("expected a conflict attempt"),
        }
        assert!(task.question.is_none());
        assert_eq!(
            task.decisions
                .iter()
                .filter(|d| d.contains("trying again after the next fetch"))
                .count(),
            CARRY_RETRIES as usize
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_carry_left_out_on_purpose_is_not_retried_and_asks_the_owner() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().base_ref = None;
        app.store.save_task(&task).unwrap();
        let worktree = PathBuf::from(&task.worktree);
        let pending = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
        let waiting = tokio::time::timeout(
            Duration::from_secs(5),
            carry_after_base_moves(&app, &mut task, &worktree, &pending, &cancel, &mut None),
        )
        .await;
        assert!(waiting.is_err(), "the owner is asked");
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.status, TaskStatus::Waiting);
        assert_eq!(
            saved.question.clone().unwrap().options,
            vec!["retry".to_string(), "stop".to_string()]
        );
        assert!(!saved
            .decisions
            .iter()
            .any(|d| d.contains("trying again after the next fetch")));
    }
}
