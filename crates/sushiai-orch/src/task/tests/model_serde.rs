use crate::policy::routing::Tier;
use crate::policy::variant::Variant;
use crate::task::attempt::{Attempt, FailureKind};
use crate::task::checks::commands::is_command_check;
use crate::task::checks::types::{valid_check, valid_checks, Baseline, Check};
use crate::task::questions::{AnsweredBy, AnsweredQuestion, AskedBy, Question, QuestionKind};
use crate::task::state::{Task, TaskStatus};

fn minimal_task_json() -> serde_json::Value {
    serde_json::json!({
        "id": "t", "title": "", "goal": "", "criteria": [], "verify": [], "repo": "",
        "worktree": "", "branch": "", "baseSha": "", "status": "waiting", "tier": "standard",
        "createdAt": 0, "updatedAt": 0, "variant": {}
    })
}

#[test]
fn a_task_json_from_before_question_history_still_loads_and_serializes_without_the_keys() {
    let mut v = minimal_task_json();
    v["question"] = serde_json::json!({"text": "Ok?", "options": ["a"], "kind": "budget"});
    let task: Task = serde_json::from_value(v).unwrap();
    let q = task.question.as_ref().unwrap();
    assert_eq!((q.asked_by, q.asked_at), (None, None));
    assert!(task.question_history.is_empty());
    let out = serde_json::to_value(&task).unwrap();
    assert!(out.get("questionHistory").is_none(), "{out}");
    assert!(out["question"].get("askedBy").is_none(), "{out}");
    assert!(out["question"].get("askedAt").is_none(), "{out}");
}

#[test]
fn a_stamped_question_and_its_history_entry_serialize_lowercase() {
    let q = Question::new(
        "Ok?",
        vec!["a".into()],
        QuestionKind::Budget,
        AskedBy::Review,
        crate::clock::now_ms(),
    );
    assert!(q.asked_at.is_some());
    let mut task: Task = serde_json::from_value(minimal_task_json()).unwrap();
    task.fixture_data().question_history.push(AnsweredQuestion {
        question: q.text.clone(),
        options: q.options.clone(),
        kind: q.kind,
        asked_by: q.asked_by,
        asked_at: q.asked_at,
        answer: "a".into(),
        answered_at: 5,
        answered_by: AnsweredBy::Orchestrator,
        failure_signature: None,
        commands: Vec::new(),
    });
    task.fixture_data().question = Some(q);
    let v = serde_json::to_value(&task).unwrap();
    assert_eq!(v["question"]["askedBy"], "review");
    assert!(v["question"]["askedAt"].is_i64());
    let e = &v["questionHistory"][0];
    assert_eq!(e["askedBy"], "review");
    assert_eq!(e["answeredBy"], "orchestrator");
    assert_eq!(e["answeredAt"], 5);
    assert_eq!(e["kind"], "budget");
    let back: Task = serde_json::from_value(v).unwrap();
    assert_eq!(back.question_history, task.question_history);
}

#[test]
fn task_request_field_is_camel_case_and_omitted_when_absent() {
    let task = crate::task::test_support::fixture(crate::task::TaskData {
        id: "t1".into(),
        title: "Fix the thing".into(),
        goal: String::new(),
        criteria: vec![],
        verify: vec![],
        final_verify: vec![],
        checks: vec![],
        held_out: None,
        planner_tests: Default::default(),
        request: Some("fix the thing that's broken".into()),
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
        branch: "task/x".into(),
        base_sha: "abc".into(),
        base_ref: None,
        fresh_base: None,
        depends_on: vec![],
        paths: vec![],
        parent: None,
        status: TaskStatus::Drafting,
        tier: Tier::Standard,
        question: None,
        decisions: vec![],
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
    });
    let v = serde_json::to_value(&task).unwrap();
    assert_eq!(v["request"], "fix the thing that's broken");
    assert_eq!(v["status"], "drafting");

    let mut without_request = task;
    without_request.fixture_data().request = None;
    let v2 = serde_json::to_value(&without_request).unwrap();
    assert!(v2.get("request").is_none());
}

#[test]
fn an_attempt_without_a_fingerprint_loads_and_round_trips_without_one() {
    let a: Attempt = serde_json::from_value(serde_json::json!({
        "n": 1, "stage": "implement", "routeId": "r", "harness": "claude", "model": "sonnet",
        "reason": "x", "resumed": false, "startedAt": 0, "status": "passed"
    }))
    .unwrap();
    assert!(a.fingerprint.is_none());
    assert!(serde_json::to_value(&a)
        .unwrap()
        .get("fingerprint")
        .is_none());
}

#[test]
fn attempt_prefix_tokens_is_optional_on_disk_and_a_removed_skills_field_still_loads() {
    let old = serde_json::json!({
        "n": 1, "stage": "implement", "routeId": "r", "harness": "claude", "model": "m",
        "reason": "x", "resumed": false, "startedAt": 0, "status": "passed",
        "skills": ["deslop"]
    });
    let mut a: Attempt = serde_json::from_value(old).unwrap();
    assert_eq!(a.prefix_tokens, None);
    let v = serde_json::to_value(&a).unwrap();
    assert!(v.get("prefixTokens").is_none() && v.get("skills").is_none());
    a.prefix_tokens = Some(19_629);
    let v = serde_json::to_value(&a).unwrap();
    assert_eq!(v["prefixTokens"], 19_629);
}

#[test]
fn max_cost_usd_is_off_by_default_and_each_raise_adds_it_once_more() {
    let v: Variant = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(v.max_cost_usd, 0.0);
    let mut task: Task = serde_json::from_value(serde_json::json!({
        "id": "t", "title": "", "goal": "", "criteria": [], "verify": [], "repo": "",
        "worktree": "", "branch": "", "baseSha": "", "status": "queued", "tier": "standard",
        "createdAt": 0, "updatedAt": 0, "variant": {}
    }))
    .unwrap();
    assert_eq!(task.cost_budget(), None);
    task.fixture_data().variant = Some(Variant {
        max_cost_usd: 2.5,
        ..Variant::default()
    });
    assert_eq!(task.cost_budget(), Some(2.5));
    task.fixture_data().budget_raises = 2;
    assert_eq!(task.cost_budget(), Some(7.5));
    assert_eq!(serde_json::to_value(&task).unwrap()["budgetRaises"], 2);
    assert_eq!(
        serde_json::to_value(task.variant()).unwrap()["maxCostUsd"],
        2.5
    );
    let negative = Variant {
        max_cost_usd: -1.0,
        ..Variant::default()
    };
    assert!(negative.check().is_err());
}

#[test]
fn task_field_names_are_camel_case() {
    let task = crate::task::test_support::fixture(crate::task::TaskData {
        id: "t1".into(),
        title: "Title".into(),
        goal: "Goal".into(),
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
        branch: "task/x".into(),
        base_sha: "abc".into(),
        base_ref: None,
        fresh_base: None,
        depends_on: vec![],
        paths: vec![],
        parent: None,
        status: TaskStatus::Queued,
        tier: Tier::Standard,
        question: None,
        decisions: vec![],
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
    });
    let v = serde_json::to_value(&task).unwrap();
    assert_eq!(v["baseSha"], "abc");
    assert_eq!(v["createdAt"], 1);
    assert!(v.get("base_sha").is_none());
}

fn task_with_criteria(criteria: &[&str], flagged: &[&str]) -> Task {
    crate::task::test_support::fixture(crate::task::TaskData {
        id: "t1".into(),
        title: "Title".into(),
        goal: "Goal".into(),
        criteria: criteria.iter().map(|c| c.to_string()).collect(),
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
        visual_criteria: flagged.iter().map(|c| c.to_string()).collect(),
        landed_sha: None,
        landed_at: None,
        unlanded: None,
        diff_stat: None,
        report: None,
        report_at: None,
        lead_touch: None,
        follow_up_of: None,
        follow_ups: vec![],
        branch: "task/x".into(),
        base_sha: "abc".into(),
        base_ref: None,
        fresh_base: None,
        depends_on: vec![],
        paths: vec![],
        parent: None,
        status: TaskStatus::Done,
        tier: Tier::Standard,
        question: None,
        decisions: vec![],
        attempts: vec![],
        cost_usd: 0.0,
        budget_raises: 0,
        daily_budget_ok_day: None,
        assumptions: vec![],
        question_history: vec![],
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
        eval_check: None,
        source: None,
        findings: vec![],
        brief_check: Default::default(),
        plan_gate: Default::default(),
        metrics: None,
        checklist: vec![],
        queue: Default::default(),
        created_at: 1,
        updated_at: 1,
    })
}

#[test]
fn a_criterion_checked_by_a_command_is_never_visual() {
    for check in [
        "x -- check: cargo test --bin orchd",
        "x -- check: npm run test:orchd",
        "x -- check: node --test tests/a.cjs",
        "x -- check: pytest tests/",
    ] {
        let task = task_with_criteria(&[check], &[check]);
        assert!(is_command_check(check), "{check}");
        assert!(task.visual_criteria_texts().is_empty(), "{check}");
    }
}

#[test]
fn a_check_that_mentions_an_image_is_still_visual() {
    for check in [
        "x -- check: screenshot under artifacts/",
        "x -- check: npm run screenshot -- artifacts/x.png",
    ] {
        let task = task_with_criteria(&[check], &[check]);
        assert!(!is_command_check(check), "{check}");
        assert_eq!(task.visual_criteria_texts(), vec![check], "{check}");
    }
    assert!(!is_command_check("cargo test"));
    assert!(is_command_check("x -- check: go test ./..."));
    assert!(!is_command_check("x -- check: read the makefile"));
}

#[test]
fn task_archived_field_is_camel_case_and_defaults_to_false_when_absent() {
    let mut task = crate::task::test_support::fixture(crate::task::TaskData {
        id: "t1".into(),
        title: "Title".into(),
        goal: "Goal".into(),
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
        branch: "task/x".into(),
        base_sha: "abc".into(),
        base_ref: None,
        fresh_base: None,
        depends_on: vec![],
        paths: vec![],
        parent: None,
        status: TaskStatus::Done,
        tier: Tier::Standard,
        question: None,
        decisions: vec![],
        attempts: vec![],
        cost_usd: 0.0,
        budget_raises: 0,
        daily_budget_ok_day: None,
        assumptions: vec![],
        question_history: vec![],
        findings: vec![],
        archived: true,
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
    });
    let v = serde_json::to_value(&task).unwrap();
    assert_eq!(v["archived"], true);

    // A pre-existing task.json written before this field existed must
    // still load, defaulting to `archived: false`.
    let mut without_archived = v.clone();
    without_archived.as_object_mut().unwrap().remove("archived");
    let loaded: Task = serde_json::from_value(without_archived).unwrap();
    assert!(!loaded.archived);

    task.fixture_data().archived = false;
    let v2 = serde_json::to_value(&task).unwrap();
    assert_eq!(v2["archived"], false);
}

fn check(criterion: usize, run: &str) -> Check {
    Check {
        criterion,
        run: run.into(),
        baseline: None,
    }
}

#[test]
fn valid_checks_drop_out_of_range_and_blank_entries() {
    let kept = valid_checks(
        vec![
            check(0, "a"),
            check(2, "out of range"),
            check(1, "  \t"),
            check(1, " b "),
        ],
        2,
    );
    assert_eq!(kept, vec![check(0, "a"), check(1, " b ")]);
    assert_eq!(valid_check(Some(check(3, "x")), 3), None);
    assert_eq!(valid_check(Some(check(2, "x")), 3), Some(check(2, "x")));
    assert_eq!(valid_check(None, 3), None);
}

#[test]
fn valid_checks_reset_a_supplied_baseline() {
    let mut c = check(0, "a");
    c.baseline = Some(Baseline::Fail);
    assert_eq!(valid_checks(vec![c.clone()], 1), vec![check(0, "a")]);
    assert_eq!(valid_check(Some(c), 1), Some(check(0, "a")));
}

#[test]
fn a_check_serializes_its_baseline_only_once_known() {
    assert_eq!(
        serde_json::to_string(&check(0, "x")).unwrap(),
        r#"{"criterion":0,"run":"x"}"#
    );
    let with = Check {
        baseline: Some(Baseline::Env),
        ..check(1, "y")
    };
    assert_eq!(
        serde_json::to_string(&with).unwrap(),
        r#"{"criterion":1,"run":"y","baseline":"env"}"#
    );
    assert_eq!(
        serde_json::to_string(&FailureKind::Heldout).unwrap(),
        r#""heldout""#
    );
    assert_eq!(FailureKind::Heldout.as_str(), "heldout");
}
