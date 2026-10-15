use crate::engine::attempts::best_of::evaluate::Eval;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::run_harness;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::review::review_request;
use crate::engine::review::select_review_route_with;
use crate::engine::review::write_readonly_claude_settings_with_profile;
use crate::engine::state::App;
use crate::git::own_diff::OwnDiff;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::helper_timeout_message;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::task::Task;
use std::path::Path;
use std::sync::Arc;

/// One read-only run that reads both diffs and answers with a `sushi-pick`
/// block; `Some((b_wins, why))`, or `None` when the reply had none.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_pick(
    app: &Arc<App>,
    task: &Task,
    task_id: &str,
    attempt_n: u32,
    settings: &Settings,
    a_route: &Route,
    b_route: &Route,
    worktree: &Path,
    wt_b: &Path,
    own: &OwnDiff,
    decisions: &mut Vec<String>,
    eval_a: &Eval,
    eval_b: &Eval,
    deny_read: &[String],
    cancel: &CancelToken,
) -> Result<(Option<(bool, String)>, f64), RunError> {
    // The reviewer on the other family; without a review route, whichever
    // family candidate a was not.
    let route = select_review_route_with(
        settings,
        task.variant().review_route_id(settings),
        a_route,
        task.tier,
    )
    .map(|(r, _)| r)
    .filter(|r| r.harness != a_route.harness)
    .unwrap_or(b_route)
    .clone();
    let (diff_a, diff_b) = candidate_diffs(worktree, wt_b, own).await;
    let brief_text = crate::engine::attempts::pick::build_pick_brief(
        task,
        &crate::engine::attempts::pick::PickCandidate {
            verify: &eval_a.verify,
            diff: &diff_a,
        },
        &crate::engine::attempts::pick::PickCandidate {
            verify: &eval_b.verify,
            diff: &diff_b,
        },
    );
    let Ok(dir) = app
        .store
        .claim_helper_run_dir(task_id, "pick", Some(attempt_n))
    else {
        return Ok((None, 0.0));
    };
    let mcp_path = dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let _ = std::fs::write(dir.join("brief.md"), &brief_text);
    let settings_path = dir.join("settings.json");
    let key_path = dir.join("key");
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            &route,
            app,
            &key_path,
            settings,
            deny_read,
            &settings_path,
        );
    }
    let events = dir.join("events.jsonl");
    let req = review_request(&route, worktree, &mcp_path, &settings_path, &[]);
    let limit = app.helper_timeout();
    let res = run_harness(
        app,
        task_id,
        attempt_n,
        RunTrack::No,
        worktree,
        &req,
        CostTag::task("pick", &route.id),
        &brief_text,
        &events,
        cancel,
        Guard::helper("best-of pick", limit),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    match res {
        Ok(outcome) => {
            let cost = outcome.cost_usd.unwrap_or(0.0);
            if outcome.timed_out {
                decisions.push(helper_timeout_message("best-of pick", limit));
                return Ok((None, cost));
            }
            let text = outcome.final_text.unwrap_or_default();
            Ok((crate::engine::attempts::pick::parse_pick(&text), cost))
        }
        Err(RunError::Cancelled) => Err(RunError::Cancelled),
        Err(RunError::Io(_) | RunError::NotFound(_)) => Ok((None, 0.0)),
    }
}

async fn candidate_diffs(worktree: &Path, sibling: &Path, own: &OwnDiff) -> (String, String) {
    let (wa, wb, own) = (worktree.to_path_buf(), sibling.to_path_buf(), own.clone());
    tokio::task::spawn_blocking(move || {
        (
            own.diff(&wa, 30_000).unwrap_or_default(),
            own.diff(&wb, 30_000).unwrap_or_default(),
        )
    })
    .await
    .unwrap_or_default()
}
