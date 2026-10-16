//! The watch of an open pull request's checks. Code only polls, reruns the
//! failed jobs once and collects logs; certain cases are fast paths with no
//! agent turn:
//!
//! - all green, no checks, or a gh that cannot list them ends the watch quietly;
//! - every failing test also fails in the base branch's latest run: not the
//!   task's, the owner is told once;
//! - the failed jobs pass on their one rerun: flaky, the owner is told once.
//!
//! Anything else is one attribution turn (`engine/attribution.rs`). The task's
//! own failure goes to its agent for one repair step through the pull request
//! check repair, and the fix is pushed to the same pull request, whose
//! publication starts the watch of the new head. A failure that is not the
//! task's leaves the task as it is and is told to the owner once per pull
//! request and cause. The state is the task's typed `PrCiWatch`, so a restart
//! resumes an unfinished watch.

mod gh;
mod red;
#[cfg(test)]
mod tests;

use self::gh::{Bucket, Gh, GhCli};
use self::red::read_red;
use crate::clock::now_ms;
use crate::engine::delivery::pr::pending::notify_owner;
use crate::engine::state::App;
use crate::task::delivery::{CiRerun, PrCiWatch};
use crate::task::{Task, TaskStatus};
use crate::verify::comparison::{compare, Compared};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Between two polls, unless `ORCHD_PR_CI_POLL_MS` sets it (tests).
const POLL: Duration = Duration::from_secs(60);
/// How long one head is watched at most.
const BOUND: Duration = Duration::from_secs(4 * 60 * 60);

#[derive(Clone, Copy)]
struct Timing {
    poll: Duration,
    bound: Duration,
}

impl Timing {
    fn from_env() -> Self {
        let poll = std::env::var("ORCHD_PR_CI_POLL_MS")
            .ok()
            .and_then(|ms| ms.trim().parse().ok())
            .map_or(POLL, Duration::from_millis);
        Self { poll, bound: BOUND }
    }
}

/// Which watch a loop runs: a new head or a new start ends an older loop.
#[derive(Clone, PartialEq, Eq)]
struct Mine {
    head: String,
    started_at: i64,
}

impl Mine {
    fn of(watch: &PrCiWatch) -> Self {
        Self {
            head: watch.head.clone(),
            started_at: watch.started_at,
        }
    }
}

/// The watch, while a done task's open pull request is watched and no check
/// repair of it is in flight.
fn watchable(task: &Task) -> Option<&PrCiWatch> {
    (task.status == TaskStatus::Done
        && !task.archived
        && task.landed_sha.is_none()
        && task.queue.pr_check_repair.is_none())
    .then(|| task.open_pr_ci_watch())
    .flatten()
}

/// Starts or resumes the watch of `task`'s open pull request in the
/// background: after its publication, and again after a restart.
pub(in crate::engine) fn resume(app: &App, task: &Task) {
    let Some(watch) = watchable(task) else {
        return;
    };
    let (app, id, mine) = (app.arc(), task.id.clone(), Mine::of(watch));
    let gh = GhCli {
        repo: PathBuf::from(&task.repo),
    };
    tokio::spawn(async move { watch_checks(&app, &gh, &id, &mine, Timing::from_env()).await });
}

/// What one poll came to.
#[derive(Debug, PartialEq, Eq)]
enum Poll {
    Again,
    Settled,
}

async fn watch_checks<G: Gh>(app: &Arc<App>, gh: &G, id: &str, mine: &Mine, timing: Timing) {
    loop {
        // Checks appear a little after a push: poll after the first wait.
        tokio::time::sleep(timing.poll).await;
        if app.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let Some(task) = app.store.load_task(id).ok().flatten() else {
            return;
        };
        let Some(watch) = watchable(&task).filter(|w| Mine::of(w) == *mine).cloned() else {
            return;
        };
        let elapsed = now_ms().saturating_sub(watch.started_at);
        if elapsed > i64::try_from(timing.bound.as_millis()).unwrap_or(i64::MAX) {
            let line = format!(
                "PR: the checks of {} were still unfinished after {} minutes; orchd stopped watching them",
                watch.url,
                timing.bound.as_secs() / 60
            );
            settle(app, id, mine, |t| t.record_decision(line));
            return;
        }
        if poll(app, gh, &task, &watch).await == Poll::Settled {
            return;
        }
    }
}

async fn poll<G: Gh>(app: &Arc<App>, gh: &G, task: &Task, watch: &PrCiWatch) -> Poll {
    let mine = Mine::of(watch);
    // No checks, or none gh can list: nothing to watch.
    let Some(checks) = gh.checks(&watch.url).await.filter(|c| !c.is_empty()) else {
        settle(app, &task.id, &mine, |_| {});
        return Poll::Settled;
    };
    if checks.iter().any(|check| check.bucket == Bucket::Pending) {
        return Poll::Again;
    }
    let failing: Vec<&gh::Check> = checks
        .iter()
        .filter(|check| check.bucket == Bucket::Fail)
        .collect();
    if failing.is_empty() {
        match &watch.rerun {
            Some(rerun) => tell(app, task, &mine, Note::Rerun(&rerun.failed)),
            None => settle(app, &task.id, &mine, |_| {}),
        }
        return Poll::Settled;
    }
    let red = read_red(gh, task, &watch.head, &failing).await;
    if let (Compared::Covered(names), Some(base)) = (compare(&red.failed, &red.base), &red.branch) {
        tell(
            app,
            task,
            &mine,
            Note::Base {
                names: &names,
                base,
            },
        );
        return Poll::Settled;
    }
    if watch.rerun.is_none() && !red.runs.is_empty() {
        let rerun = CiRerun {
            runs: red.runs.clone(),
            failed: red.names.clone(),
        };
        // Saved first: a restart never reruns the same head twice.
        if !update_watch(app, &task.id, &mine, |t| t.record_pr_ci_rerun(rerun)) {
            return Poll::Settled;
        }
        for run in &red.runs {
            if let Err(error) = gh.rerun_failed(*run).await {
                eprintln!("orch: rerunning the failed jobs of run {run} failed: {error}");
            }
        }
        return Poll::Again;
    }
    red::judge(app, task, watch, red).await;
    Poll::Settled
}

/// Why a red check is not the task's, as the owner reads it.
enum Note<'a> {
    Rerun(&'a [String]),
    Base {
        names: &'a [String],
        base: &'a str,
    },
    Verdict {
        names: &'a [String],
        cause: &'a str,
        evidence: &'a str,
    },
    Repaired(&'a [String]),
}

impl Note<'_> {
    /// One note per pull request and cause.
    fn cause(&self) -> String {
        match self {
            Self::Rerun(names) => format!("rerun:{}", names.join(",")),
            Self::Base { names, base } => format!("base:{base}:{}", names.join(",")),
            Self::Verdict { names, cause, .. } => format!("{cause}:{}", names.join(",")),
            Self::Repaired(names) => format!("repaired:{}", names.join(",")),
        }
    }

    fn text(&self, task: &Task, url: &str) -> String {
        let head = format!("Task \"{}\": a check of its pull request {url}", task.title);
        let tail = "The task is left as it is.";
        match self {
            Self::Rerun(names) => format!(
                "{head} failed ({}) and passed on a rerun of the failed jobs: it is flaky, not the task's change. {tail}",
                names.join(", ")
            ),
            Self::Base { names, base } => format!(
                "{head} fails ({}), and the latest run on the base branch {base} fails the same tests: it is not the task's change. {tail}",
                names.join(", ")
            ),
            Self::Verdict { names, cause, evidence } => format!(
                "{head} fails ({}); the attribution turn reads it as {cause}, not the task's change: {evidence} {tail}",
                names.join(", ")
            ),
            Self::Repaired(names) => format!(
                "{head} still fails ({}) after the task's repair step was pushed to it. It needs a look.",
                names.join(", ")
            ),
        }
    }
}

/// Ends the watch and tells the owner once per pull request and cause.
fn tell(app: &Arc<App>, task: &Task, mine: &Mine, note: Note<'_>) {
    let cause = note.cause();
    let mut first = false;
    settle(app, &task.id, mine, |t| first = t.record_pr_ci_told(&cause));
    if first {
        let url = task.pr_url.as_deref().unwrap_or_default();
        notify_owner(app, &task.repo, note.text(task, url));
    }
}

/// Ends the watch with `change`, while it is still this loop's watch.
fn settle(app: &App, id: &str, mine: &Mine, change: impl FnOnce(&mut Task)) {
    update_watch(app, id, mine, |t| {
        change(t);
        t.finish_pr_ci_watch();
    });
}

/// Applies `change` while the watch is still this loop's; whether it was.
fn update_watch(app: &App, id: &str, mine: &Mine, change: impl FnOnce(&mut Task)) -> bool {
    let mut current = false;
    let _ = app.update_task(id, |t| {
        current = watchable(t).is_some_and(|w| Mine::of(w) == *mine);
        if current {
            change(t);
        }
    });
    current
}
