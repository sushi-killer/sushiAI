use crate::engine::delivery::pr::carry::carry_for_pr;
use crate::engine::delivery::pr::carry::PrCarry;
use crate::engine::delivery::pr::pending::notify_owner;
use crate::engine::delivery::pr::pending::owed;
use crate::engine::delivery::pr::pending::record;
use crate::engine::state::App;
use crate::task::state::Task;
use std::sync::Arc;
use std::time::Duration;

pub(super) async fn carry_before_draft(app: &Arc<App>, task: &Task) -> bool {
    let id = task.id.as_str();
    // Never push from a stale base: carry onto the newest one first.
    // A manual `task.pr` carries in the same checkout: one at a time.
    let guard = loop {
        match crate::delivery::pr::in_flight::InFlight::take(id) {
            Ok(guard) => break guard,
            Err(_) if owed(app, id).is_some() => {
                tokio::time::sleep(Duration::from_millis(200)).await
            }
            Err(_) => {
                record(app, id, Vec::new(), 0.0, true);
                return false;
            }
        }
    };
    let carried = carry_for_pr(app, task, None).await;
    drop(guard);
    match carried {
        Ok(PrCarry::Ready(_)) => {}
        // One decision line was saved; there is no pull request to open.
        Ok(PrCarry::Contained) => {
            record(app, id, Vec::new(), 0.0, true);
            return false;
        }
        // The task is back in its loop and opens the pull request when done.
        Ok(
            PrCarry::Resolving(_) | PrCarry::ChecksPending | PrCarry::Failed(_) | PrCarry::Deferred,
        ) => return false,
        // setup_failed: the environment, not the work. The branch is
        // untouched; the owner hears once and opens it with task_pr.
        Ok(PrCarry::Environment(e)) => {
            record(
                app,
                id,
                vec![format!(
                    "PR: not opened: the setup of the carry worktree failed ({e}); this is the environment, the work is untouched and nothing was pushed"
                )],
                0.0,
                true,
            );
            notify_owner(
                app,
                &task.repo,
                format!(
                    "Task \"{}\" is done but its pull request was not opened: the repo's worktree setup failed while carrying onto the newest base ({e}). The work is on branch {}. Fix the setup, then open it with task_pr.",
                    task.title, task.branch
                ),
            );
            return false;
        }
        Err(e) => {
            record(
                app,
                id,
                vec![format!(
                    "PR: not opened: the work could not be carried onto the newest base ({e}); nothing was pushed"
                )],
                0.0,
                true,
            );
            notify_owner(
                app,
                &task.repo,
                format!(
                    "Task \"{}\" is done but its pull request was not opened: the work could not be carried onto the newest base ({e}). Nothing was pushed; the work is on branch {}. Carry it by hand, then open it with task_pr.",
                    task.title, task.branch
                ),
            );
            return false;
        }
    }
    true
}
