use crate::clock::now_ms;
use crate::engine::graph::landing::Landing;
use crate::engine::state::App;
use crate::task::{Task, TaskStatus};
use std::path::Path;
use std::sync::{atomic::Ordering, Arc};
use std::time::Duration;

const LANDING_RETRY: Duration = Duration::from_secs(120);
const DIRTY_LIMIT: Duration = Duration::from_secs(3600);
fn dirty_limit_ms() -> i64 {
    std::env::var("ORCHD_LANDING_DIRTY_LIMIT_MS")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(DIRTY_LIMIT.as_millis() as i64)
}

pub(super) fn dirty(app: &Arc<App>, task: &mut Task, branch: &str, checkout: &Path) -> Landing {
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
    Landing::Waiting
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
    pub(in crate::engine) fn spawn_landing_retries(&self) {
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
