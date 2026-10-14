use super::*;

#[path = "../../tests/support/native_sandbox.rs"]
mod native_sandbox;

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
        dir.path().join("home"),
        "orchd".to_string(),
    )
    .unwrap();
    app.assume_harnesses_installed();
    // Tests run on a synthetic clock: quiet hours would depend on the machine's zone.
    app.settings.write().unwrap().quiet_hours = String::new();
    (app, dir)
}

/// Test fixture for behavioral cases that start task checks. When Seatbelt
/// cannot be applied from this process, keep the assertions running with a
/// host policy instead of trying to nest a native profile.
pub(super) fn test_app_fitting_sandbox() -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let store = Store::new(&data_dir).unwrap();
    let mut settings = serde_json::to_value(store.load_settings().unwrap()).unwrap();
    native_sandbox::fit_sandbox(&mut settings);
    store
        .save_settings(&serde_json::from_value(settings).unwrap())
        .unwrap();
    let app = App::new(data_dir, dir.path().join("home"), "orchd".to_string()).unwrap();
    app.assume_harnesses_installed();
    (app, dir)
}

pub(crate) fn task_with_status(status: TaskStatus) -> Task {
    Task {
        id: uuid::Uuid::new_v4().to_string(),
        title: "Do thing".into(),
        goal: "Do the thing".into(),
        criteria: vec![],
        verify: vec![],
        final_verify: vec![],
        checks: vec![],
        held_out: None,
        planner_tests: Default::default(),
        request: None,
        repo: "/repo".into(),
        project_id: None,
        worktree: "/repo-task".into(),
        worktree_removed: false,
        deliverables: vec![],
        visual_criteria: vec![],
        landed_sha: None,
        landed_at: None,
        unlanded: None,
        diff_stat: None,
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
        daily_budget_ok_day: None,
        assumptions: vec![],
        question_history: vec![],
        judged_findings: vec![],
        findings: vec![],
        archived: false,
        pr_url: None,
        pr_branch: None,
        pr_pending: false,
        pr_plan: None,
        overseer_woken: false,
        planned_tier: None,
        planned_by_parent: false,
        tier_fallback: None,
        variant: Default::default(),
        eval_set: None,
        eval_name: None,
        eval_check_cmd: None,
        source: None,
        eval_check: None,
        brief_check: Default::default(),
        plan_gate: Default::default(),
        metrics: None,
        checklist: vec![],
        queue: Default::default(),
        created_at: 1,
        updated_at: 1,
    }
}

/// Like `test_app`, with a single task-loop slot from construction.
/// Later settings changes resize that limit live.
pub(super) fn test_app_parallel_one() -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    let store = Store::new(&data_dir).unwrap();
    let mut settings = store.load_settings().unwrap();
    settings.parallel = 1;
    store.save_settings(&settings).unwrap();
    let app = App::new(data_dir, dir.path().join("home"), "orchd".to_string()).unwrap();
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
        first_edit_at: None,
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
        review_tree: None,
        review_base: None,
        review_scope: None,
        evidence_base: None,
        evidence_from: None,
        advice: None,
        advisor_cost_usd: None,
        fingerprint: None,
        review_fingerprint: None,
        advisor_fingerprint: None,
        candidates: vec![],
        criteria_results: vec![],
        diff_stat: None,
    }
}

/// Runs `git` in `dir`, panicking on failure; returns its stdout.
pub(super) fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A git repo in `dir` with one commit.
pub(super) fn init_git_repo(dir: &std::path::Path) {
    git_in(dir, &["init", "-q"]);
    git_in(dir, &["config", "user.email", "test@example.com"]);
    git_in(dir, &["config", "user.name", "Test"]);
    std::fs::write(dir.join("README.md"), "hello\n").unwrap();
    git_in(dir, &["add", "."]);
    git_in(dir, &["commit", "-q", "-m", "init"]);
}

/// A task worktree whose shared fake harness waits for the helper guard.
pub(super) fn slow_helper_task() -> (Arc<App>, tempfile::TempDir, Task, Route) {
    chat::fake_overseer_harness();
    let (app, dir) = test_app();
    app.helper_timeout_ms.store(250, Ordering::SeqCst);
    let worktree = dir.path().join("repo");
    std::fs::create_dir(&worktree).unwrap();
    init_git_repo(&worktree);
    std::fs::write(worktree.join(".overseer-slow"), "").unwrap();
    let mut task = task_with_status(TaskStatus::Running);
    task.repo = worktree.display().to_string();
    task.worktree = task.repo.clone();
    task.base_sha = git_in(&worktree, &["rev-parse", "HEAD"]);
    task.criteria = vec!["Keep the API".into()];
    task.attempts = vec![attempt_with_failure(1, "failure")];
    let route = {
        let mut settings = app.settings.write().unwrap();
        settings.planner = "claude-sonnet".into();
        settings.brief_check_route = "claude-sonnet".into();
        settings.orchestrator = "claude-sonnet".into();
        settings.auto_answer = true;
        settings
            .routes
            .iter()
            .find(|r| r.id == "claude-sonnet")
            .unwrap()
            .clone()
    };
    app.store.save_task(&task).unwrap();
    (app, dir, task, route)
}

pub(super) fn assert_helper_timeout(task: &Task, helper: &str) {
    let line = helper_timeout_message(helper, Duration::from_millis(250));
    assert_eq!(
        task.decisions.iter().filter(|d| d.contains(&line)).count(),
        1,
        "{:?}",
        task.decisions
    );
    // Under load the deadline can stop the fake before it records its call;
    // what matters is that a timed-out helper is never started again.
    assert!(chat::fake_overseer_calls(Path::new(&task.worktree)) <= 1);
}
