use super::*;

// -- task.archive / task.unarchive --------------------------------------
//
// These drive `App::dispatch` directly (no real socket, no real harness
// process): the validate-then-mutate logic under test only ever looks at
// a task's stored `status`, so a task written straight into the store
// with the status under test is equivalent to -- and far faster than --
// getting a real attempt loop into that same state.

/// The `TempDir` must stay alive for as long as the `App` does (dropping
/// it deletes the directory `App` reads and writes) -- callers keep the
/// tuple bound for the whole test, not just the `Arc<App>`.
pub(super) fn test_app() -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::new(
        dir.path().join("data"),
        dir.path().join("orchd.sock"),
        "orchd".to_string(),
    )
    .unwrap();
    (app, dir)
}

pub(super) fn task_with_status(status: TaskStatus) -> Task {
    Task {
        id: uuid::Uuid::new_v4().to_string(),
        title: "Do thing".into(),
        goal: "Do the thing".into(),
        criteria: vec![],
        verify: vec![],
        final_verify: vec![],
        checks: vec![],
        held_out: None,
        request: None,
        repo: "/repo".into(),
        worktree: "/repo-task".into(),
        worktree_removed: false,
        visual_criteria: vec![],
        landed_sha: None,
        report: None,
        report_at: None,
        lead_touch: None,
        follow_up_of: None,
        follow_ups: vec![],
        branch: "task/do-thing".into(),
        base_sha: "deadbeef".into(),
        base_ref: None,
        depends_on: vec![],
        paths: vec![],
        parent: None,
        status,
        tier: Tier::Standard,
        question: None,
        decisions: vec![],
        attempts: vec![],
        cost_usd: 0.0,
        budget_raises: 0,
        assumptions: vec![],
        judged_findings: vec![],
        archived: false,
        planned_tier: None,
        tier_fallback: None,
        variant: Default::default(),
        eval_set: None,
        eval_name: None,
        eval_check_cmd: None,
        eval_check: None,
        brief_check: Default::default(),
        queue: Default::default(),
        created_at: 1,
        updated_at: 1,
    }
}

/// Like `test_app`, but writes `parallel: 1` before `App::new` reads
/// settings -- `parallel_limit`/`slots` are fixed at construction time
/// (a live `settings.set` only takes effect on the next restart), so
/// this is the only way to get a single-slot app for a "second task
/// holds the only slot" test.
pub(super) fn test_app_parallel_one() -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let store = Store::new(&data_dir).unwrap();
    let mut settings = store.load_settings().unwrap();
    settings.parallel = 1;
    store.save_settings(&settings).unwrap();
    let app = App::new(data_dir, dir.path().join("orchd.sock"), "orchd".to_string()).unwrap();
    (app, dir)
}

pub(super) fn attempt_with_failure(n: u32, signature: &str) -> Attempt {
    Attempt {
        n,
        stage: Stage::Implement,
        route_id: "claude-sonnet".into(),
        harness: Harness::Claude,
        model: "sonnet".into(),
        reason: "r".into(),
        session_id: None,
        pgid: None,
        started_at: 0,
        ended_at: None,
        status: AttemptStatus::Failed,
        summary: None,
        handoff: None,
        disputes: vec![],
        changed_files: vec![],
        verify: vec![],
        gate_blocks: 0,
        prefix_tokens: None,
        review: None,
        failure: Some(Failure {
            kind: FailureKind::Verify,
            detail: "d".into(),
            signature: signature.to_string(),
        }),
        usage: None,
        cost_usd: None,
        cost_estimated: false,
        review_cost_usd: None,
        evidence: vec![],
        evidence_tree: None,
        evidence_from: None,
        advice: None,
        advisor_cost_usd: None,
        fingerprint: None,
        review_fingerprint: None,
        advisor_fingerprint: None,
        candidates: vec![],
    }
}
