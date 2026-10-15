use super::{criterion_text_refusal, shared_check_amend_refusal, CheckAuthority, Origin};
use crate::task::amendment::Amendment;
use crate::task::checks::types::{CheckSource, SourcedCheck};
use crate::task::state::Task;
use serde_json::json;

fn criterion_task(source: Option<CheckSource>) -> Task {
    let mut task: Task = serde_json::from_value(json!({
        "id": "criterion-authority", "title": "Criterion authority", "goal": "Keep the check",
        "criteria": ["Works -- check: ./scripts/gate.sh"], "verify": [],
        "repo": "/synthetic/repo", "worktree": "/synthetic/worktree",
        "branch": "task/synthetic", "baseSha": "base", "status": "queued",
        "tier": "standard", "createdAt": 1, "updatedAt": 1
    }))
    .unwrap();
    if let Some(source) = source {
        task.fixture_data()
            .planner_tests
            .sources
            .push(SourcedCheck {
                command: "./scripts/gate.sh".into(),
                source,
            });
    }
    task
}

#[test]
fn a_criterion_command_must_remain_a_complete_check_instead_of_prose_or_a_prefix() {
    let authority = CheckAuthority::default();
    for source in [Some(CheckSource::Owner), Some(CheckSource::Repo), None] {
        let task = criterion_task(source);
        for replacement in [
            "Works; ./scripts/gate.sh was mentioned",
            "Works -- check: ./scripts/gate.sh-extra",
            "Works; ./scripts/gate.sh -- check: true",
        ] {
            let amendment = Amendment {
                criteria: Some(vec![replacement.into()]),
                ..Default::default()
            };
            assert!(
                criterion_text_refusal(&authority, &task, 0, replacement).is_some(),
                "{source:?}: {replacement}"
            );
            assert!(
                shared_check_amend_refusal(&authority, &task, &amendment).is_some(),
                "{source:?}: {replacement}"
            );
            let mut owned = task.clone();
            amendment
                .apply_authorized(&mut owned, Origin::Owner, &authority, 2)
                .unwrap();
            assert_eq!(owned.criteria, vec![replacement]);
        }
    }
}

#[test]
fn an_exact_criterion_check_survives_rewording_and_reordering() {
    let task = criterion_task(Some(CheckSource::Owner));
    let authority = CheckAuthority::default();
    let replacement = "Works better -- check:   ./scripts/gate.sh  ";
    assert!(criterion_text_refusal(&authority, &task, 0, replacement).is_none());
    let amendment = Amendment {
        criteria: Some(vec!["Another criterion".into(), replacement.into()]),
        ..Default::default()
    };
    assert!(shared_check_amend_refusal(&authority, &task, &amendment).is_none());
}

#[test]
fn a_planner_criterion_check_may_be_replaced_without_owner_authority() {
    let mut task = criterion_task(Some(CheckSource::Planner));
    let authority = CheckAuthority::default();
    let replacement = "Works; ./scripts/gate.sh was mentioned";
    assert!(criterion_text_refusal(&authority, &task, 0, replacement).is_none());
    let amendment = Amendment {
        criteria: Some(vec![replacement.into()]),
        ..Default::default()
    };
    amendment
        .apply_authorized(&mut task, Origin::Other, &authority, 2)
        .unwrap();
    assert_eq!(task.criteria, vec![replacement]);
}
