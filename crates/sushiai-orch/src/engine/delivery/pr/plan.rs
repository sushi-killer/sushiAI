use crate::delivery::pr::body::final_body;
use crate::delivery::pr::body::redact_pr_text;
use crate::delivery::pr::branch::choose_branch;
use crate::delivery::pr::reply::Draft;
use crate::engine::attempts::secrets::run_secrets;
use crate::engine::delivery::pr::draft::check_body;
use crate::engine::delivery::pr::draft::draft_pr;
use crate::engine::delivery::pr::pending::owed;
use crate::engine::delivery::pr::pending::record;
use crate::engine::state::App;
use crate::task::delivery::BranchFrom;
use crate::task::delivery::PrPlan;
use crate::task::state::Task;
use std::sync::Arc;

struct DraftedPr {
    draft: Draft,
    branch: String,
    branch_from: BranchFrom,
    cost: f64,
    lines: Vec<String>,
}
async fn draft_content(app: &Arc<App>, task: &Task) -> DraftedPr {
    let mut cost = 0.0;
    let mut drafted_ok = true;
    let (mut lines, draft) = match draft_pr(app, task, &mut cost).await {
        Ok((route, draft)) => (vec![format!("PR: drafted by {route}")], draft),
        Err(reason) => {
            drafted_ok = false;
            (
                vec![format!(
                    "PR: draft failed ({reason}); opened with the task's own title and body"
                )],
                Draft::default(),
            )
        }
    };
    let (branch, branch_from) = match choose_branch(task, &draft.branch).await {
        Ok(name) => (name, BranchFrom::Drafted),
        Err(why) => {
            if drafted_ok {
                lines.push(format!(
                    "PR: branch: {why}; the task's own branch {} is used",
                    task.branch
                ));
            }
            (task.branch.clone(), BranchFrom::Task)
        }
    };
    DraftedPr {
        draft,
        branch,
        branch_from,
        cost,
        lines,
    }
}
pub(super) async fn draft_plan(app: &Arc<App>, task: &Task) -> Option<PrPlan> {
    let id = task.id.as_str();
    if let Some(plan) = task.pr_plan.clone().filter(PrPlan::is_reusable) {
        return Some(plan);
    }
    let DraftedPr {
        draft,
        branch,
        branch_from,
        mut cost,
        mut lines,
    } = draft_content(app, task).await;
    // Landed, or opened by the overseer or the owner, while the draft ran.
    let Some(task) = owed(app, id) else {
        if let Ok(Some(now)) = app.store.load_task(id) {
            if let Some(url) = now.pr_url.clone() {
                lines.push(format!("PR: already open {url}, draft not used"));
            }
        }
        record(app, id, lines, cost, true);
        return None;
    };
    let secrets = match run_secrets(app, id) {
        Ok(secrets) => secrets.redaction,
        Err(e) => {
            lines.push(format!("PR: not opened: {e}"));
            record(app, id, lines, cost, true);
            return None;
        }
    };
    let home = std::env::var("HOME").ok();
    let title = redact_pr_text(&draft.title, &secrets, home.as_deref());
    // The task's own body when the draft has none: it is published too.
    let drafted = if draft.body.trim().is_empty() {
        crate::delivery::pr::body::pr_body(&task)
    } else {
        draft.body.clone()
    };
    // One agent turn checks the body against the final diff and the
    // check results before anything is pushed. A failed turn keeps the
    // drafted body; the owner reads the line either way.
    let checked = match check_body(app, &task, &drafted, &mut cost).await {
        Ok(check) if check.ok => drafted,
        Ok(check) => {
            lines.push(format!("PR: body check: {}", check.corrections.join("; ")));
            check
                .body
                .filter(|b| !b.trim().is_empty())
                .unwrap_or(drafted)
        }
        Err(reason) => {
            lines.push(format!(
                "PR: body check failed ({reason}); opened as drafted"
            ));
            drafted
        }
    };
    let commits = super::validation::recorded_commits(&task).await;
    let body = final_body(&task, checked, &secrets, home.as_deref(), &commits);
    let plan = PrPlan {
        branch,
        title,
        body,
        branch_from,
    };
    let saved = plan.clone();
    let _ = app.update_task(id, move |t| {
        t.record_decisions(lines);
        t.charge(cost);
        t.record_pr_plan(saved);
    });
    Some(plan)
}
