use super::*;
use crate::engine::evolution::test_support::*;

#[test]
fn evolution_adopted_proposal_with_no_drop_after_ten_tasks_suggests_a_revert() {
    let (mut p, tasks, signals) = measured(10, 10);
    assert!(measure(
        &mut p,
        &tasks,
        &signals,
        &EvolutionSettings::default()
    ));
    assert_eq!(p.status, ProposalStatus::RevertSuggested);
    assert_eq!(
        p.before,
        Some(ProposalCounts {
            tasks: 4,
            signals: 4
        })
    );
    assert_eq!(
        p.after,
        Some(ProposalCounts {
            tasks: 10,
            signals: 10
        })
    );
    assert!(p.reason.as_deref().unwrap().contains("no drop"));
}

#[test]
fn evolution_adopted_proposal_with_a_drop_or_too_few_tasks_stays_adopted() {
    let (mut dropped, tasks, signals) = measured(10, 2);
    measure(
        &mut dropped,
        &tasks,
        &signals,
        &EvolutionSettings::default(),
    );
    assert_eq!(dropped.status, ProposalStatus::Adopted);
    assert_eq!(
        dropped.after,
        Some(ProposalCounts {
            tasks: 10,
            signals: 2
        })
    );
    let (mut young, tasks, signals) = measured(9, 9);
    measure(&mut young, &tasks, &signals, &EvolutionSettings::default());
    assert_eq!(young.status, ProposalStatus::Adopted);
    assert_eq!(young.after.unwrap().tasks, 9);
}

#[test]
fn evolution_approved_repo_proposal_is_adopted_once_its_task_is_done() {
    let mut p = proposal(ProposalStatus::Approved, "loop:x");
    p.task_id = Some("own".into());
    let mut running = finished("own", 500);
    running.fixture_data().status = TaskStatus::Running;
    assert!(!measure(
        &mut p,
        &[running],
        &[],
        &EvolutionSettings::default()
    ));
    assert_eq!(p.status, ProposalStatus::Approved);
    assert!(measure(
        &mut p,
        &[finished("own", 500)],
        &[],
        &EvolutionSettings::default()
    ));
    assert_eq!(p.status, ProposalStatus::Adopted);
    assert_eq!(p.adopted_at, Some(500));
}
