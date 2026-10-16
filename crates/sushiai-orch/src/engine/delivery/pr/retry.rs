use crate::delivery::pr::error::{PrError, PrRefusal};
use crate::delivery::pr::failure::failure_of;
use crate::delivery::pr::failure::retry_delays;
use crate::delivery::pr::failure::PrFailure;
use crate::engine::delivery::pr::pending::notify_owner;
use crate::engine::delivery::pr::pending::owed;
use crate::engine::delivery::pr::pending::record;
use crate::engine::state::App;
use crate::task::delivery::PrPlan;
use crate::task::state::Task;
use std::sync::atomic::Ordering;
use std::sync::Arc;

pub(super) async fn publish_plan(app: &Arc<App>, task: &Task, plan: &PrPlan) {
    let id = task.id.as_str();
    let (mut tries, delays) = (0, retry_delays());
    loop {
        if owed(app, id).is_none() {
            return record(app, id, Vec::new(), 0.0, true);
        }
        let params = crate::delivery::pr::request::PrParams {
            id: id.to_string(),
            title: Some(plan.title.clone()),
            branch: Some(plan.branch.clone()),
            body: Some(plan.body.clone()),
            source: None,
        };
        let e = match app.open_pr(params, true).await {
            Ok(_) => return record(app, id, Vec::new(), 0.0, true),
            Err(PrError::Refused(PrRefusal::TaskChanged)) => return,
            Err(error) => error,
        };
        // Opened or landed meanwhile: nothing to report.
        if owed(app, id).is_none() {
            return record(app, id, Vec::new(), 0.0, true);
        }
        let (permanent, hint) = match failure_of(&e) {
            PrFailure::NoGithubHost => {
                return record(app, id, vec![format!("PR: not opened: {e}")], 0.0, true);
            }
            PrFailure::Permanent(hint) => (true, hint),
            PrFailure::Transient => (false, ""),
        };
        if permanent || tries >= delays.len() {
            let tried = if permanent {
                String::new()
            } else {
                format!(" after {} tries", tries + 1)
            };
            record(
                app,
                id,
                vec![format!("PR: not opened{tried}: {e}.{hint}")],
                0.0,
                true,
            );
            notify_owner(
                app,
                &task.repo,
                format!(
                    "Task \"{}\" is done but its pull request was not opened{tried}: {e}.{hint} The work is on branch {}. Fix that, then open it with task_pr.",
                    task.title, task.branch
                ),
            );
            return;
        }
        let wait = delays[tries];
        tries += 1;
        let line = format!(
            "PR: try {tries} failed ({e}); trying again in {}s",
            wait.as_secs()
        );
        record(app, id, vec![line], 0.0, false);
        tokio::time::sleep(wait).await;
        // A shutdown keeps `pr_pending`: the next start resumes the flow.
        if app.shutting_down.load(Ordering::SeqCst) {
            return;
        }
    }
}
