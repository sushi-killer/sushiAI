use crate::engine::attempts::best_of::candidate::EvaluatedCandidates;
use crate::engine::attempts::best_of::run::BestOfRun;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::run_harness;
use crate::engine::attempts::run::{RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::review::native::review_request;
use crate::engine::review::native::write_readonly_claude_settings_with_profile;
use crate::engine::review::routing::select_review_route_with;
use crate::engine::state::App;
use crate::git::own_diff::OwnDiff;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::helper_timeout_message;
use crate::harness::guard::Guard;
use crate::harness::native::error::RunError;
use crate::policy::routing::Harness;
use std::path::Path;
use std::sync::Arc;

/// One read-only run that reads both diffs and answers with a `sushi-pick`
/// block; `Some((b_wins, why))`, or `None` when the reply had none.
pub(super) async fn run_pick(
    app: &Arc<App>,
    run: &BestOfRun<'_>,
    candidates: &EvaluatedCandidates,
    decisions: &mut Vec<String>,
    cancel: &CancelToken,
) -> Result<(Option<(bool, String)>, f64), RunError> {
    let primary = &run.primary;
    let task = primary.task;
    let task_id = task.id.as_str();
    let attempt_n = primary.attempt_n;
    let settings = primary.settings;
    let a_route = &candidates.a.run.route;
    let b_route = &candidates.b.run.route;
    let worktree = candidates.a.run.worktree.as_path();
    let wt_b = candidates.b.run.worktree.as_path();
    let (eval_a, eval_b) = (&candidates.a.checks, &candidates.b.checks);
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
    let (diff_a, diff_b) = candidate_diffs(worktree, wt_b, &task.own_diff()).await;
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
            settings,
            run.deny_read,
            &settings_path,
        );
    }
    let events = dir.join("events.jsonl");
    let req = review_request(&route, worktree, &mcp_path, &settings_path, &[]);
    let limit = app.helper_timeout();
    let res = run_harness(
        app,
        RunIdentity {
            id: task_id,
            attempt_n,
            track: RunTrack::No,
            tag: CostTag::task("pick", &route.id),
        },
        RunInvocation {
            request: &req,
            brief: &brief_text,
            events: &events,
            schema: None,
        },
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
