use super::*;
use crate::engine::persistence::contract::tests::{
    assert_owner_applies, pending_once, queue_owner,
};
use crate::engine::test_support::{task_with_status, test_app_fitting_sandbox};
use crate::task::{
    FindingDetail, FindingRecord, FindingScenario as Scenario, FindingStatus, TaskStatus, Verdict,
};

#[tokio::test]
async fn an_owner_edit_during_a_scenario_keeps_the_finding_out_of_the_contract() {
    let (app, dir) = test_app_fitting_sandbox();
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().worktree = dir.path().to_string_lossy().into_owned();
    task.fixture_data().findings.push(FindingRecord {
        id: "F-1".into(),
        text: "P1 failure".into(),
        status: FindingStatus::Open,
        severity: None,
        attempt: 1,
        round: 1,
        repeats: 1,
        scenarios: vec![],
        handled: false,
    });
    app.store.save_task(&task).unwrap();
    let mut review = review_of("F-1", "sh -c 'exit 1'");
    crate::task::findings::register_findings(&mut task, 2, 1, &mut review);
    assert_eq!(task.findings[0].repeats, 2);
    let observed = task.clone();
    let cancel = CancelToken::new();
    {
        let future = handle_repeated_findings(&app, &mut task, 1, &mut review, &cancel);
        tokio::pin!(future);
        pending_once(future.as_mut()).await;
        queue_owner(&app, &observed);
        assert_eq!(future.await, Err(WaitEnd::Changed));
    }
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(saved.criteria.is_empty());
    assert!(saved.checks.is_empty());
    assert!(!saved.findings[0].handled);
    assert!(!task.findings[0].handled);
    assert_eq!(review.repeated, ["P1 failure [F-1]"]);
    assert_eq!(saved.findings[0].repeats, 1);
    assert_eq!(task.findings[0].repeats, 2);
    assert_owner_applies(&app, &task);
}

fn review_of(id: &str, scenario: &str) -> ReviewResult {
    ReviewResult {
        verdict: Verdict::Fail,
        findings: vec!["P1 failure".into()],
        repeated: Vec::new(),
        severities: vec![Some(1)],
        criteria: Vec::new(),
        details: vec![FindingDetail {
            id: Some(id.into()),
            scenario: Some(Scenario {
                run: scenario.into(),
                fails: "red".into(),
            }),
            ..Default::default()
        }],
    }
}

#[tokio::test]
async fn an_unsaved_registration_publishes_its_actual_repeat_and_scenario() {
    use crate::engine::test_support::{attempt_with_failure, test_app};
    use crate::task::attempts::{record_review_view, ReviewView};
    use crate::task::findings::{apply_declines, register_findings};
    use crate::task::{Decline, DeclineReason, DeclineRuling, ReviewScope};
    let (app, dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().worktree = dir.path().to_string_lossy().into_owned();
    task.fixture_data()
        .attempts
        .push(attempt_with_failure(2, "failure"));
    let mut first = review_of("F-1", "false");
    register_findings(&mut task, 1, 1, &mut first);
    let mut second = review_of("F-1", "false");
    register_findings(&mut task, 2, 1, &mut second);
    let mut other = task.findings[0].clone();
    other.id = "F-2".into();
    other.text = "P1 declined".into();
    task.fixture_data().findings.push(other);
    app.store.save_task(&task).unwrap();

    record_review_view(
        &mut task,
        0,
        Some(ReviewView {
            tree: Some("review tree".into()),
            base: "review base".into(),
            scope: ReviewScope::Full,
        }),
    );
    let mut review = review_of("F-1", "Fix the parser; touch ran-marker");
    apply_declines(
        &mut task,
        3,
        &mut review,
        &[Decline {
            id: "F-2".into(),
            reason: DeclineReason::Wrong,
            evidence: vec!["already covered".into()],
        }],
        &[DeclineRuling {
            id: "F-2".into(),
            upheld: true,
        }],
    );
    register_findings(&mut task, 3, 2, &mut review);
    assert_eq!(task.findings[0].repeats, 2);
    assert_eq!(
        app.store.load_task(&task.id).unwrap().unwrap().findings[0].repeats,
        1
    );
    handle_repeated_findings(&app, &mut task, 3, &mut review, &CancelToken::new())
        .await
        .unwrap();

    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.findings[0], task.findings[0]);
    assert_eq!(saved.findings[0].attempt, 3);
    assert_eq!(saved.findings[0].round, 2);
    assert_eq!(saved.findings[0].repeats, 2);
    assert!(saved.findings[0].handled);
    assert!(saved.findings[0]
        .scenarios
        .contains(&"Fix the parser; touch ran-marker".into()));
    assert_eq!(saved.criteria, task.criteria);
    assert_eq!(saved.checks.len(), task.checks.len());
    assert!(!dir.path().join("ran-marker").exists());
    assert_eq!(task.findings[1].status, FindingStatus::Resolved);
    assert_eq!(task.assumptions.len(), 1);
    assert_eq!(task.attempts[0].review_tree.as_deref(), Some("review tree"));
    app.save_task_logged(&task);
    let final_saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(final_saved.findings, task.findings);
    assert_eq!(final_saved.assumptions.len(), 1);
    assert_eq!(
        final_saved.attempts[0].review_base.as_deref(),
        Some("review base")
    );
}

#[tokio::test]
async fn an_entirely_unsaved_finding_is_adopted_with_its_handled_receipt() {
    let (app, dir) = test_app_fitting_sandbox();
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().worktree = dir.path().to_string_lossy().into_owned();
    app.store.save_task(&task).unwrap();
    let mut review = review_of("F-1", "Fix the parser");
    for attempt in 1..=3 {
        crate::task::findings::register_findings(&mut task, attempt, 1, &mut review);
    }
    assert_eq!(task.findings[0].repeats, 2);
    assert!(app
        .store
        .load_task(&task.id)
        .unwrap()
        .unwrap()
        .findings
        .is_empty());
    handle_repeated_findings(&app, &mut task, 3, &mut review, &CancelToken::new())
        .await
        .unwrap();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.findings, task.findings);
    assert!(saved.findings[0].handled);
    assert_eq!(saved.criteria, task.criteria);
    assert_eq!(saved.checks.len(), task.checks.len());
    assert!(review.repeated.is_empty());
}
