use crate::delivery::pr::checkout::TempWorktree;
use crate::delivery::pr::reply::schema_reply;
use crate::delivery::pr::reply::BodyCheck;
use crate::delivery::pr::reply::Draft;
use crate::delivery::pr::reply::PrTurn;
use crate::engine::delivery::pr::helper::run_helper;
use crate::engine::delivery::pr::helper::HelperRun;
use crate::engine::state::App;
use crate::harness;
use crate::task::state::Task;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// Longest diff given to the drafter, in characters.
const MAX_DIFF_CHARS: usize = 20_000;

/// Runs a helper turn in the task's worktree, else in a temporary detached
/// worktree of its branch. `make_brief` gets the diff against the base.
/// Returns the route id and the run's outcome; `cost` gets the spend.
async fn helper_turn(
    app: &Arc<App>,
    task: &Task,
    turn: PrTurn,
    make_brief: impl FnOnce(&str) -> String,
    cost: &mut f64,
) -> Result<(String, harness::RunOutcome), String> {
    let name = turn.name();
    let schema = turn.schema();
    let settings = app.settings.read().unwrap().clone();
    let route = settings
        .pr_draft_route()
        .cloned()
        .ok_or("no route to draft with")?;
    let (route, _) = app
        .usable_route(&settings, route)
        .ok_or("no agent harness is available")?;
    let repo = PathBuf::from(&task.repo);
    let temp = if Path::new(&task.worktree).join(".git").exists() {
        None
    } else {
        let path = crate::git::worktree::worktree_path(
            &repo,
            &app.home,
            "",
            &format!("pr-draft-{}", task.id),
        );
        Some(
            TempWorktree::checkout(
                repo.clone(),
                path,
                format!("refs/heads/{}", task.branch),
                &task.branch,
            )
            .await?,
        )
    };
    let worktree = temp
        .as_ref()
        .map_or_else(|| PathBuf::from(&task.worktree), |t| t.path.clone());
    let (wt, base) = (worktree.clone(), task.base_sha.clone());
    let diff = tokio::task::spawn_blocking(move || {
        crate::git::changes::diff_full(&wt, &base, MAX_DIFF_CHARS).unwrap_or_default()
    })
    .await
    .unwrap_or_default();
    let brief = make_brief(&diff);
    let run = HelperRun {
        name,
        schema,
        route: &route,
        settings: &settings,
        worktree: &worktree,
        brief: &brief,
    };
    let outcome = run_helper(app, task, run, cost).await;
    drop(temp);
    outcome.map(|outcome| (route.id, outcome))
}

/// Drafts the branch, title and body of the pull request; `cost` gets the spend.
pub(super) async fn draft_pr(
    app: &Arc<App>,
    task: &Task,
    cost: &mut f64,
) -> Result<(String, Draft), String> {
    let (route, outcome) = helper_turn(
        app,
        task,
        PrTurn::Draft,
        |diff| crate::delivery::brief::build_pr_draft_brief(task, diff),
        cost,
    )
    .await?;
    let draft = schema_reply(&outcome, "draft")?;
    Ok((route, draft))
}

/// Checks the drafted body against the final diff and the check results; the
/// reply is the verdict. `cost` gets the spend.
pub(super) async fn check_body(
    app: &Arc<App>,
    task: &Task,
    body: &str,
    cost: &mut f64,
) -> Result<BodyCheck, String> {
    let (_, outcome) = helper_turn(
        app,
        task,
        PrTurn::BodyCheck,
        |diff| crate::delivery::brief::build_pr_truth_brief(task, body, diff),
        cost,
    )
    .await?;
    schema_reply(&outcome, "body check")
}
