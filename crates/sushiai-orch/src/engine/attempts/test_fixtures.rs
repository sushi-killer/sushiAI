use crate::policy::routing::{Harness, Tier};
use crate::task::attempt::{Attempt, AttemptStatus, Failure, FailureKind, Stage};
use crate::task::checks::types::{Baseline, Check};
use crate::task::state::{Task, TaskStatus};

pub(crate) fn sample_task() -> Task {
    crate::task::test_support::fixture(crate::task::TaskData {
        id: "t1".into(),
        title: "Add a button".into(),
        goal: "Add a Save button to the settings dialog".into(),
        criteria: vec!["Button visible".into(), "Click saves settings".into()],
        verify: vec!["npm test".into()],
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
        branch: "task/add-a-button".into(),
        base_sha: "abc123".into(),
        base_ref: None,
        fresh_base: None,
        depends_on: vec![],
        paths: vec![],
        parent: None,
        status: TaskStatus::Running,
        tier: Tier::Standard,
        question: None,
        pending_amendment: None,
        delivery_checks: vec![],
        decisions: vec!["Owner: use the primary button style".into()],
        attempts: vec![],
        cost_usd: 0.0,
        budget_raises: 0,
        daily_budget_ok_day: None,
        assumptions: vec![],
        question_history: vec![],
        findings: vec![],
        archived: false,
        pr_url: None,
        pr_branch: None,
        pr_pending: false,
        pr_plan: None,
        overseer_woken: false,
        unreviewed: false,
        failed_count: 0,
        loop_state: Default::default(),
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
    })
}

pub(crate) fn failed_implement_attempt(detail: String) -> Attempt {
    Attempt {
        n: 1,
        stage: Stage::Implement,
        route_id: "claude-sonnet".into(),
        harness: Harness::Claude,
        model: "sonnet".into(),
        reason: "tier default".into(),
        session_id: None,
        pgid: None,
        started_at: 1,
        ended_at: Some(2),
        status: AttemptStatus::Failed,
        summary: None,
        handoff: None,
        declines: vec![],
        changed_files: vec![],
        verify: vec![],
        gate_blocks: 0,
        prefix_tokens: None,
        first_edit_at: None,
        review: None,
        failure: Some(Failure {
            kind: FailureKind::Verify,
            detail,
            signature: "verify:xxxx".into(),
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
        cause: None,
        diff_stat: None,
    }
}

pub(crate) fn grounded_task() -> Task {
    let mut task = sample_task();
    task.fixture_data().checks = vec![
        Check {
            criterion: 0,
            run: "gated-cmd".into(),
            baseline: Some(Baseline::Fail),
        },
        Check {
            criterion: 1,
            run: "vacuous-cmd".into(),
            baseline: Some(Baseline::Pass),
        },
        Check {
            criterion: 1,
            run: "env-cmd".into(),
            baseline: Some(Baseline::Env),
        },
    ];
    task.fixture_data().held_out = Some(Check {
        criterion: 1,
        run: "SECRET-held-out-cmd".into(),
        baseline: Some(Baseline::Fail),
    });
    task
}

pub(crate) fn done_dependency(id: &str, ended_at: i64) -> Task {
    let mut dep = sample_task();
    dep.fixture_data().id = id.into();
    dep.fixture_data().title = format!("Dependency {id}");
    dep.fixture_data().status = TaskStatus::Done;
    dep.fixture_data().decisions.clear();
    let mut attempt = Attempt {
        declines: vec![],
        n: 1,
        stage: Stage::Implement,
        route_id: "claude-sonnet".into(),
        harness: Harness::Claude,
        model: "sonnet".into(),
        reason: "tier default".into(),
        session_id: None,
        pgid: None,
        started_at: 1,
        ended_at: Some(ended_at),
        status: AttemptStatus::Passed,
        summary: Some("DEP_SUMMARY".into()),
        handoff: Some("DEP_HANDOFF".into()),
        changed_files: vec![],
        verify: vec![],
        gate_blocks: 0,
        prefix_tokens: None,
        first_edit_at: None,
        review: None,
        failure: None,
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
        cause: None,
        diff_stat: None,
    };
    attempt.changed_files = vec!["a.txt".into()];
    dep.fixture_data().attempts.push(attempt);
    dep
}

pub(crate) fn dependent_of(ids: &[&str]) -> Task {
    let mut task = sample_task();
    task.fixture_data().id = "child".into();
    task.fixture_data().depends_on = ids.iter().map(|s| s.to_string()).collect();
    task
}
