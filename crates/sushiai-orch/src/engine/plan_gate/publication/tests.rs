use super::*;
use crate::task::{Check, TaskStatus};
fn remove_criteria(task: &mut Task, remove: &[usize]) {
    let snapshot = crate::task::checks::snapshot::CheckSnapshot::capture(task);
    crate::task::checks::criteria::remove_report_only(
        &crate::task::checks::authority::CheckAuthority::default(),
        task,
        &snapshot,
        remove,
        Origin::Owner,
    )
    .unwrap();
}

#[test]
fn plan_gate_removes_a_criterion_and_reindexes_its_checks() {
    let mut task = crate::engine::test_support::task_with_status(TaskStatus::Drafting);
    task.fixture_data().criteria = vec!["r".into(), "keep one".into(), "keep two".into()];
    task.fixture_data().visual_criteria = vec!["r".into(), "keep two".into()];
    task.fixture_data().checks = vec![
        Check {
            criterion: 0,
            run: "a".into(),
            baseline: None,
        },
        Check {
            criterion: 2,
            run: "c".into(),
            baseline: None,
        },
    ];
    remove_criteria(&mut task, &[0]);
    assert_eq!(task.criteria, vec!["keep one", "keep two"]);
    assert_eq!(task.visual_criteria, vec!["keep two"]);
    assert_eq!(task.checks.len(), 1);
    assert_eq!(task.checks[0].criterion, 1);
}

fn proposal(task: &Task) -> GateProposal {
    GateProposal {
        input: GateInput::capture(task),
        review: PlanReview {
            first_pass: true,
            removals: vec![0],
            findings: vec![crate::task::PlanFinding {
                id: "review-0".into(),
                severity: crate::task::Severity::Critical,
                kind: "wrong-approach".into(),
                text: "Use the requested approach".into(),
            }],
            decisions: vec!["Plan gate: reviewed draft".into()],
        },
    }
}

fn draft_task() -> Task {
    let mut task = crate::engine::test_support::task_with_status(TaskStatus::Drafting);
    task.fixture_data().criteria = vec!["Report completion".into(), "Keep acceptance".into()];
    task
}

#[test]
fn a_changed_owner_contract_rejects_the_entire_review_publication() {
    let (app, _dir) = crate::engine::test_support::test_app();
    let task = draft_task();
    app.store.save_task(&task).unwrap();
    let proposal = proposal(&task);
    let owner = app
        .update_task(&task.id, |fresh| {
            fresh.fixture_data().criteria = vec!["Owner's replacement".into()];
            fresh.record_decision("Owner changed the contract".into());
        })
        .unwrap();
    assert!(matches!(
        proposal
            .publish(&app, &task.id, &CancelToken::new())
            .unwrap(),
        GateAction::Changed
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(saved).unwrap(),
        serde_json::to_value(owner).unwrap()
    );
}

#[test]
fn a_durable_pending_owner_edit_defers_the_gate_without_a_ram_control() {
    use crate::task::amendment::Amendment;
    use crate::task::checks::authority::CheckAuthority;
    let (app, _dir) = crate::engine::test_support::test_app();
    let mut task = draft_task();
    app.store.save_task(&task).unwrap();
    let proposal = proposal(&task);
    task.queue_pending_amendment(
        Amendment {
            criteria: Some(vec!["Owner's pending acceptance".into()]),
            ..Default::default()
        },
        Origin::Owner,
        &CheckAuthority::default(),
        2,
    )
    .unwrap();
    app.store.save_task_with_pending(&task, None).unwrap();
    let before = serde_json::to_value(&task).unwrap();
    assert!(matches!(
        proposal
            .publish(&app, &task.id, &CancelToken::new())
            .unwrap(),
        GateAction::Changed
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(serde_json::to_value(saved).unwrap(), before);
    assert!(app.controls.lock().unwrap().is_empty());
}

#[test]
fn a_stop_or_cancel_prevents_report_removal_and_replan() {
    for stop in [false, true] {
        let (app, _dir) = crate::engine::test_support::test_app();
        let mut task = draft_task();
        app.store.save_task(&task).unwrap();
        let proposal = proposal(&task);
        let cancel = CancelToken::new();
        if stop {
            task.resume_phase(TaskStatus::Stopped, 2);
            app.store.save_task(&task).unwrap();
        } else {
            cancel.cancel();
        }
        let before = serde_json::to_value(&task).unwrap();
        assert!(matches!(
            proposal.publish(&app, &task.id, &cancel).unwrap(),
            GateAction::Cancelled
        ));
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(serde_json::to_value(saved).unwrap(), before);
    }
}

#[test]
fn one_fresh_publication_applies_the_review_and_duplicate_input_cannot_reapply_it() {
    let (app, _dir) = crate::engine::test_support::test_app();
    let task = draft_task();
    app.store.save_task(&task).unwrap();
    let first = proposal(&task);
    let duplicate = proposal(&task);
    app.update_task(&task.id, |fresh| fresh.charge(0.4))
        .unwrap();
    assert!(matches!(
        first.publish(&app, &task.id, &CancelToken::new()).unwrap(),
        GateAction::Replan
    ));
    let accepted = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(accepted.criteria, ["Keep acceptance"]);
    assert!(accepted.plan_gate.reviewed && accepted.plan_gate.replanned);
    assert_eq!(accepted.cost_usd, 0.4);
    assert_eq!(accepted.plan_gate.findings.len(), 1);
    assert!(matches!(
        duplicate
            .publish(&app, &task.id, &CancelToken::new())
            .unwrap(),
        GateAction::Changed
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(saved).unwrap(),
        serde_json::to_value(accepted).unwrap()
    );
}

#[tokio::test]
async fn a_stopped_or_pending_gate_starts_no_model_turn() {
    use crate::engine::planning::draft::parse_plan;
    use crate::task::amendment::Amendment;
    use crate::task::checks::authority::CheckAuthority;
    for stopped in [true, false] {
        let (app, _dir) = crate::engine::test_support::test_app();
        let mut task = draft_task();
        app.store.save_task(&task).unwrap();
        if stopped {
            task.resume_phase(TaskStatus::Stopped, 2);
            app.store.save_task(&task).unwrap();
        } else {
            task.queue_pending_amendment(
                Amendment {
                    criteria: Some(vec!["Owner's pending acceptance".into()]),
                    ..Default::default()
                },
                Origin::Owner,
                &CheckAuthority::default(),
                2,
            )
            .unwrap();
            app.store.save_task_with_pending(&task, None).unwrap();
        }
        let before = serde_json::to_value(&task).unwrap();
        let draft = parse_plan("```sushi-plan\n{\"title\":\"Plan\"}\n```").unwrap();
        let action = crate::engine::plan_gate::run(&app, &task.id, 1, &draft, &CancelToken::new())
            .await
            .unwrap();
        assert!(matches!(
            (stopped, action),
            (true, GateAction::Cancelled) | (false, GateAction::Changed)
        ));
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(serde_json::to_value(saved).unwrap(), before);
        assert!(!app.store.task_dir(&task.id).join("runs").exists());
    }
}
