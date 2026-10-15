use super::*;
use crate::policy::routing::{Harness, Route};
use crate::task::attempts::{
    record_best_of, record_run_completion, BestOfSelection, RunCompletion,
};
use crate::task::test_support::sample_task;
use crate::task::{AttemptStatus, Candidate, CheckTally, TaskStatus, Usage};

fn task() -> Task {
    sample_task("native", TaskStatus::Running, AttemptStatus::Running)
}

fn complete(task: &mut Task, cost: Option<f64>) {
    record_run_completion(
        task,
        0,
        RunCompletion {
            session: Some("completed".into()),
            fingerprint: None,
            usage: Usage {
                input: 10,
                output: 5,
                cached: 2,
            },
            first_turn_tokens: Some(12),
            cost,
            estimated: false,
        },
    );
}

fn candidate(route: &str, cost: f64, picked: bool) -> Candidate {
    Candidate {
        route: route.into(),
        cost,
        verify: true,
        checks: CheckTally::default(),
        picked,
    }
}

#[test]
fn either_selected_candidate_and_its_picker_are_counted_once() {
    for b_won in [false, true] {
        let mut saved = task();
        let mut local = saved.clone();
        let extra = if b_won { 0.47 } else { 0.87 };
        let mut settlement = Settlement::capture(&local, 0, extra, &[]).unwrap();
        let mut winner = Route {
            id: "b".into(),
            label: "b".into(),
            harness: Harness::Claude,
            model: Some("opus".into()),
            effort: None,
            profile_id: None,
            account_id: None,
            strength: None,
        };
        if !b_won {
            winner.id = "a".into();
        }
        record_best_of(
            &mut local,
            0,
            BestOfSelection {
                winner: b_won.then_some(winner),
                candidates: vec![candidate("a", 0.4, !b_won), candidate("b", 0.8, b_won)],
                extra_cost: extra,
            },
        );
        settlement.merge(&mut saved, &local.attempts[0]).unwrap();
        settlement.accept(&mut local, &saved).unwrap();
        complete(&mut local, Some(if b_won { 0.8 } else { 0.4 }));
        settlement.merge(&mut saved, &local.attempts[0]).unwrap();
        settlement.accept(&mut local, &saved).unwrap();
        let receipt = serde_json::to_value(&saved).unwrap();
        settlement.merge(&mut saved, &local.attempts[0]).unwrap();
        settlement.accept(&mut local, &saved).unwrap();
        assert_eq!(serde_json::to_value(&saved).unwrap(), receipt);
        assert!((saved.cost_usd - 1.27).abs() < 1e-9);
        assert_eq!(saved.attempts[0].candidates[1].picked, b_won);
        assert_eq!(saved.attempts[0].usage.as_ref().unwrap().input, 10);
    }
}

#[test]
fn a_failed_selection_without_candidates_publishes_its_spend_once() {
    let mut saved = task();
    let mut local = saved.clone();
    let mut settlement = Settlement::capture(&local, 0, 0.47, &[]).unwrap();
    record_best_of(
        &mut local,
        0,
        BestOfSelection {
            winner: None,
            candidates: Vec::new(),
            extra_cost: 0.47,
        },
    );
    settlement.merge(&mut saved, &local.attempts[0]).unwrap();
    settlement.accept(&mut local, &saved).unwrap();
    settlement.merge(&mut saved, &local.attempts[0]).unwrap();
    assert!((saved.cost_usd - 0.47).abs() < 1e-9);
    assert!(saved.attempts[0].candidates.is_empty());
    assert_eq!(saved.attempts[0].cost_usd, None);
}

#[test]
fn prepriced_single_run_uses_the_cost_difference_and_retains_a_known_estimate() {
    let mut saved = task();
    saved.fixture_data().attempts[0].cost_usd = Some(0.2);
    saved.fixture_data().attempts[0].cost_estimated = true;
    saved.fixture_data().cost_usd = 0.2;
    let mut local = saved.clone();
    let mut settlement = Settlement::capture(&local, 0, 0.0, &[]).unwrap();
    complete(&mut local, None);
    settlement.merge(&mut saved, &local.attempts[0]).unwrap();
    settlement.accept(&mut local, &saved).unwrap();
    assert_eq!(saved.cost_usd, 0.2);
    assert_eq!(local.attempts[0].cost_usd, Some(0.2));
    assert!(local.attempts[0].cost_estimated);
    complete(&mut local, Some(0.8));
    settlement.merge(&mut saved, &local.attempts[0]).unwrap();
    settlement.accept(&mut local, &saved).unwrap();
    assert_eq!(saved.cost_usd, 0.8);
    assert_eq!(local.cost_usd, 0.8);
    assert!(!local.attempts[0].cost_estimated);
}

#[test]
fn adoption_restores_only_the_recounted_winner_cost() {
    let mut saved = task();
    saved.fixture_data().attempts[0].candidates =
        vec![candidate("a", 0.4, false), candidate("b", 0.8, true)];
    saved.fixture_data().attempts[0].cost_usd = Some(0.8);
    saved.fixture_data().cost_usd = 1.27;
    saved.fixture_data().attempts[0].cost_usd = None;
    saved.fixture_data().cost_usd = 0.47;
    let mut local = saved.clone();
    let mut settlement = Settlement::capture(&local, 0, 0.0, &[]).unwrap();
    complete(&mut local, Some(0.8));
    settlement.merge(&mut saved, &local.attempts[0]).unwrap();
    settlement.accept(&mut local, &saved).unwrap();
    settlement.merge(&mut saved, &local.attempts[0]).unwrap();
    assert!((saved.cost_usd - 1.27).abs() < 1e-9);
}

#[test]
fn a_replaced_attempt_cannot_receive_an_old_native_receipt() {
    let mut saved = task();
    let local = saved.clone();
    let settlement = Settlement::capture(&local, 0, 0.0, &[]).unwrap();
    saved.fixture_data().attempts[0].started_at += 1;
    let before = serde_json::to_value(&saved).unwrap();
    assert!(settlement.merge(&mut saved, &local.attempts[0]).is_err());
    assert_eq!(serde_json::to_value(&saved).unwrap(), before);
}
