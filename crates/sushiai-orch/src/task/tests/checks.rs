use super::task;
use crate::task::amendment::{Amendment, PendingAmendment};
use crate::task::checks::authority::{CheckAuthority, Origin};
use crate::task::checks::baseline::{self, BaselineObservation, BaselineTicket};
use crate::task::checks::criteria::drop_criterion;
use crate::task::checks::snapshot::{CheckSnapshot, CriterionEdit};
use crate::task::{Baseline, Check, CheckSource, PlannedTest, SourcedCheck};

fn check(index: usize, run: &str) -> Check {
    Check {
        criterion: index,
        run: run.into(),
        baseline: None,
    }
}

#[test]
fn captured_drop_replay_does_not_drop_the_next_criterion_but_a_fresh_request_does() {
    let mut task = task();
    task.fixture_data().criteria = vec!["first".into(), "second".into(), "third".into()];
    let authority = CheckAuthority::default();
    let first = CriterionEdit::capture(&task, 0).unwrap();
    drop_criterion(&authority, &mut task, &first, Origin::Owner).unwrap();
    assert!(drop_criterion(&authority, &mut task, &first, Origin::Owner).is_err());
    assert_eq!(task.criteria, ["second", "third"]);
    let next = CriterionEdit::capture(&task, 0).unwrap();
    drop_criterion(&authority, &mut task, &next, Origin::Owner).unwrap();
    assert_eq!(task.criteria, ["third"]);
}

#[test]
fn non_owner_drop_preserves_unknown_repo_owner_and_settings_commands() {
    for source in [
        None,
        Some(CheckSource::Repo),
        Some(CheckSource::Owner),
        Some(CheckSource::Planner),
    ] {
        let mut task = task();
        task.fixture_data().criteria = vec!["criterion -- check: custom-check".into()];
        task.fixture_data().verify = vec!["custom-check".into()];
        task.fixture_data().final_verify = task.verify.clone();
        task.fixture_data().checks = vec![check(0, "custom-check")];
        task.fixture_data().held_out = Some(check(0, "custom-check"));
        if let Some(source) = source {
            task.fixture_data()
                .planner_tests
                .sources
                .push(SourcedCheck {
                    command: "custom-check".into(),
                    source,
                });
        }
        let authority = CheckAuthority {
            named: if source == Some(CheckSource::Planner) {
                vec!["custom-check".into()]
            } else {
                vec![]
            },
        };
        let before = serde_json::to_value(&task).unwrap();
        let edit = CriterionEdit::capture(&task, 0).unwrap();
        assert!(drop_criterion(&authority, &mut task, &edit, Origin::Other).is_err());
        let mut after = serde_json::to_value(&task).unwrap();
        after["decisions"] = before["decisions"].clone();
        assert_eq!(after, before);
        drop_criterion(&authority, &mut task, &edit, Origin::Owner).unwrap();
        assert!(task.criteria.is_empty() && task.verify.is_empty() && task.final_verify.is_empty());
        assert!(task.checks.is_empty() && task.held_out.is_none());
    }
}

#[test]
fn baseline_completion_requires_the_full_plan_base_and_planner_file_identity() {
    let mut original = task();
    original.fixture_data().criteria = vec!["works".into()];
    original.fixture_data().checks = vec![check(0, "custom-check")];
    original.fixture_data().planner_tests.files = vec![PlannedTest {
        path: "test.rs".into(),
        content: "before".into(),
    }];
    let ticket = BaselineTicket::capture(&original);
    let results = [BaselineObservation {
        baseline: Baseline::Fail,
        code: Some(1),
    }];
    assert!(!baseline::record(&mut original.clone(), &ticket, &[], 4));
    let mut changed = original.clone();
    changed.fixture_data().base_sha = "other".into();
    assert!(!baseline::record(&mut changed, &ticket, &results, 4));
    let mut changed = original.clone();
    changed.fixture_data().planner_tests.files[0].content = "after".into();
    assert!(!baseline::record(&mut changed, &ticket, &results, 4));
    let mut changed = original.clone();
    changed.fixture_data().criteria[0] = "another meaning".into();
    assert!(!baseline::record(&mut changed, &ticket, &results, 4));
    assert!(baseline::record(&mut original, &ticket, &results, 4));
    assert_eq!(original.checks[0].baseline, Some(Baseline::Fail));
    assert_eq!(original.updated_at, 4);
}

#[test]
fn rebuilding_a_check_uses_current_authority_and_resets_only_the_replaced_baseline() {
    let mut task = task();
    task.fixture_data().criteria = vec!["works -- check: custom-old".into(), "kept".into()];
    task.fixture_data().checks = vec![check(0, "custom-old"), check(1, "keep")];
    task.fixture_data().checks[0].baseline = Some(Baseline::Fail);
    task.fixture_data().checks[1].baseline = Some(Baseline::Pass);
    task.fixture_data()
        .planner_tests
        .sources
        .push(SourcedCheck {
            command: "custom-old".into(),
            source: CheckSource::Owner,
        });
    let amendment = Amendment {
        criteria: Some(vec![
            "works -- check: node scripts/new.mjs".into(),
            "kept".into(),
        ]),
        ..Default::default()
    };
    let authority = CheckAuthority::default();
    assert!(amendment
        .clone()
        .apply_authorized(&mut task, Origin::Other, &authority, 8)
        .is_err());
    amendment
        .apply_authorized(&mut task, Origin::Owner, &authority, 8)
        .unwrap();
    assert_eq!(task.checks[0].run, "node scripts/new.mjs");
    assert_eq!(task.checks[0].baseline, None);
    assert_eq!(task.checks[1].baseline, Some(Baseline::Pass));
}

#[test]
fn queued_amendments_recheck_provenance_and_reject_a_changed_check_plan() {
    let mut task = task();
    task.fixture_data().criteria = vec!["works".into()];
    task.fixture_data().verify = vec!["custom-check".into()];
    task.fixture_data()
        .planner_tests
        .sources
        .push(SourcedCheck {
            command: "custom-check".into(),
            source: CheckSource::Planner,
        });
    let edit = Amendment {
        verify: Some(vec![]),
        ..Default::default()
    };
    let pending = PendingAmendment::capture(&task, edit, Origin::Other);
    let authority = CheckAuthority::default();
    assert!(pending
        .preview(&task, &authority, 5)
        .unwrap()
        .verify
        .is_empty());
    task.fixture_data().planner_tests.sources[0].source = CheckSource::Owner;
    assert!(pending.preview(&task, &authority, 5).is_err());
    assert_eq!(task.verify, ["custom-check"]);
}

#[test]
fn source_recording_never_downgrades_owner_or_repo_authority() {
    let mut task = task();
    task.record_check_sources(vec![SourcedCheck {
        command: "custom-check".into(),
        source: CheckSource::Owner,
    }]);
    task.record_check_sources(vec![SourcedCheck {
        command: "custom-check".into(),
        source: CheckSource::Planner,
    }]);
    assert_eq!(task.planner_tests.sources[0].source, CheckSource::Owner);
    let before = CheckSnapshot::capture(&task);
    task.fixture_data().updated_at += 1;
    assert!(before.matches(&task));
    task.fixture_data().base_sha = "changed-base".into();
    assert!(!before.matches(&task));
}

#[test]
fn pending_owner_amendment_follows_a_carry_without_changing_its_contract() {
    let mut before = task();
    before.fixture_data().criteria = vec!["works".into()];
    before.fixture_data().checks = vec![check(0, "old-check")];
    before.fixture_data().planner_tests.files = vec![PlannedTest {
        path: "test.rs".into(),
        content: "same source".into(),
    }];
    let pending = PendingAmendment::capture(
        &before,
        Amendment {
            verify: Some(vec!["owner-check".into()]),
            ..Default::default()
        },
        Origin::Owner,
    );
    let mut after = before.clone();
    crate::task::transitions::move_base(&mut after, "new-base".into());
    after.fixture_data().checks[0].baseline = Some(Baseline::Pass);
    after.fixture_data().planner_tests.protected = Some(std::collections::BTreeMap::from([(
        "test.rs".into(),
        "new-hash".into(),
    )]));
    let carried = pending.carry_forward(&before, &after).unwrap();
    assert!(pending
        .preview(&after, &CheckAuthority::default(), 10)
        .is_err());
    let preview = carried
        .preview(&after, &CheckAuthority::default(), 10)
        .unwrap();
    assert_eq!(preview.base_sha, "new-base");
    assert_eq!(preview.verify, ["owner-check"]);
    assert_eq!(preview.planner_tests.files, before.planner_tests.files);
}

#[test]
fn pending_carry_rejects_changed_criteria_and_planner_source() {
    let mut before = task();
    before.fixture_data().criteria = vec!["original".into()];
    before.fixture_data().planner_tests.files = vec![PlannedTest {
        path: "test.rs".into(),
        content: "original".into(),
    }];
    let pending = PendingAmendment::capture(
        &before,
        Amendment {
            verify: Some(vec!["owner-check".into()]),
            ..Default::default()
        },
        Origin::Owner,
    );
    let mut criteria = before.clone();
    crate::task::transitions::move_base(&mut criteria, "new-base".into());
    criteria.fixture_data().criteria[0] = "replacement".into();
    assert!(pending.carry_forward(&before, &criteria).is_err());
    let mut source = before.clone();
    crate::task::transitions::move_base(&mut source, "new-base".into());
    source.fixture_data().planner_tests.files[0].content = "replacement".into();
    assert!(pending.carry_forward(&before, &source).is_err());
    assert!(pending.carry_forward(&criteria, &source).is_err());
}

#[test]
fn async_plan_identity_covers_screenshot_targeted_and_checklist_commands() {
    let original = task();
    let captured = CheckSnapshot::capture(&original);
    let mut screenshot = original.clone();
    screenshot.fixture_data().brief_check.screenshot = Some("owner screenshot".into());
    assert!(!captured.matches(&screenshot));
    let mut targeted = original.clone();
    targeted.fixture_data().brief_check.check = Some("owner targeted check".into());
    assert!(!captured.matches(&targeted));
    let mut checklist = original.clone();
    checklist
        .fixture_data()
        .checklist
        .push(crate::task::ChecklistItem {
            text: "owner item".into(),
            check: "owner command".into(),
            passes: false,
        });
    assert!(!captured.matches(&checklist));
    let mut gates = original.clone();
    gates
        .fixture_data()
        .brief_check
        .non_gating
        .push("owner waived command".into());
    assert!(!captured.matches(&gates));
}
