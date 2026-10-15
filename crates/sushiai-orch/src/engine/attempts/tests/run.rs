use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::run_harness;
use crate::engine::attempts::run::{RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::review::native::review_request;
use crate::engine::test_support::slow_helper_task;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::Guard;
use std::path::PathBuf;
use std::time::Duration;

#[tokio::test]
async fn helper_timed_out_while_stdin_is_full() {
    let (app, _dir, task, route) = slow_helper_task();
    let worktree = PathBuf::from(&task.worktree);
    std::fs::write(worktree.join(".overseer-no-stdin"), "").unwrap();
    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, "test-helper", None)
        .unwrap();
    let mcp = run_dir.join("mcp.json");
    let settings = run_dir.join("settings.json");
    let req = review_request(&route, &worktree, &mcp, &settings, &[]);
    let prompt = "x".repeat(1024 * 1024);
    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        run_harness(
            &app,
            RunIdentity {
                id: &task.id,
                attempt_n: 0,
                track: RunTrack::No,
                tag: CostTag::task_without_attempt("brief_check", &route.id),
            },
            RunInvocation {
                request: &req,
                brief: &prompt,
                events: &run_dir.join("events.jsonl"),
                schema: None,
            },
            &CancelToken::new(),
            Guard::helper("test-helper", app.helper_timeout()),
        ),
    )
    .await
    .expect("the helper deadline must cover blocked stdin")
    .ok()
    .expect("the helper must start");
    assert!(outcome.timed_out);
    assert_eq!(
        outcome.error.as_deref(),
        Some("test-helper timed out after 0.25s")
    );
}
