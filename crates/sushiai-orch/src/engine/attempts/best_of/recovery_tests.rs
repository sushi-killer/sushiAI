use super::candidate::{CandidateEvidence, CandidateRun, EvaluatedCandidates};
use super::evaluate::Eval;
use super::outcome::{finish, BestOfExtra};
use crate::engine::attempts::ledger::{record_run, CostTag};
use crate::engine::attempts::pricing::ProviderRun;
use crate::engine::attempts::run::RunIdentity;
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::lifecycle::adopted;
use crate::engine::lifecycle::native_settlement::{NativeSettlement, Publication};
use crate::engine::recovery;
use crate::engine::test_support::{attempt_with_failure, task_with_status, test_app};
use crate::harness::cancel::CancelToken;
use crate::harness::{replay_events, RunOutcome};
use crate::policy::routing::{Harness, Route};
use crate::task::attempt::{AttemptStatus, CheckTally, Fingerprint, Usage};
use crate::task::attempts::{
    record_best_of, record_run_completion, BestOfSelection, RunCompletion,
};
use crate::task::state::{Task, TaskStatus};
use std::path::Path;
use std::sync::Arc;

const A_EVENTS: &str = include_str!("../../../../tests/fixtures/best_of/claude-selected-a.jsonl");
const B_EVENTS: &str = include_str!("../../../../tests/fixtures/best_of/codex-selected-b.jsonl");

fn candidate(route: Route, outcome: RunOutcome, cost: f64, dir: &Path) -> CandidateEvidence {
    CandidateEvidence {
        run: CandidateRun {
            route,
            worktree: dir.to_path_buf(),
            run_dir: dir.to_path_buf(),
            outcome: Ok(outcome),
            cost,
        },
        checks: Eval {
            passed: true,
            verify_ok: true,
            failing: 0,
            verify: vec![],
            tally: CheckTally::default(),
        },
        restored: vec![],
    }
}

fn record_candidate_costs(
    app: &crate::engine::state::App,
    task: &Task,
    candidates: &EvaluatedCandidates,
) {
    let a = &candidates.a.run;
    let b = &candidates.b.run;
    let pick = RunOutcome {
        cost_usd: Some(0.07),
        ..Default::default()
    };
    for (route, outcome, stage) in [
        (&a.route, a.outcome.as_ref().ok().unwrap(), "implement"),
        (&b.route, b.outcome.as_ref().ok().unwrap(), "implement"),
        (&a.route, &pick, "pick"),
    ] {
        record_run(
            app,
            &RunIdentity {
                id: &task.id,
                attempt_n: 1,
                track: RunTrack::No,
                tag: CostTag::task(stage, &route.id),
            },
            ProviderRun {
                harness: route.harness,
                model: route.model.as_deref(),
                resume: None,
            },
            outcome,
            0,
        );
    }
}

fn selected_b(app: &Arc<crate::engine::state::App>) -> (Task, Fingerprint) {
    let mut task = task_with_status(TaskStatus::Running);
    let settings = app.settings.read().unwrap().clone();
    let a = settings
        .routes
        .iter()
        .find(|route| route.harness == Harness::Claude)
        .unwrap()
        .clone();
    let b = settings
        .routes
        .iter()
        .find(|route| route.harness == Harness::Codex)
        .unwrap()
        .clone();
    let mut attempt = attempt_with_failure(1, "running");
    attempt.route_id = a.id.clone();
    attempt.harness = a.harness;
    attempt.model = a.model.clone().unwrap_or_default();
    attempt.status = AttemptStatus::Running;
    attempt.failure = None;
    task.fixture_data().attempts.push(attempt);
    app.store.save_task(&task).unwrap();
    let dir = app.store.run_dir(&task.id, 1);
    std::fs::create_dir_all(dir.join("b")).unwrap();
    std::fs::write(dir.join("events.raw"), A_EVENTS).unwrap();
    std::fs::write(dir.join("events.exit"), "0\n").unwrap();
    std::fs::write(dir.join("b/events.jsonl"), B_EVENTS).unwrap();
    let a_outcome = replay_events(a.harness, A_EVENTS);
    let fingerprint = Fingerprint {
        models: vec!["actual-b".into()],
        harness: b.harness,
        harness_version: Some("fixture-b-version".into()),
        prompt_hash: "b-input-hash".into(),
    };
    let b_outcome = RunOutcome {
        cost_usd: Some(0.8),
        fingerprint: Some(fingerprint.clone()),
        ..replay_events(b.harness, B_EVENTS)
    };
    let candidates = EvaluatedCandidates {
        a: candidate(a.clone(), a_outcome, 0.4, &dir),
        b: candidate(b, b_outcome, 0.8, &dir.join("b")),
        decisions: vec![],
    };
    record_candidate_costs(app, &task, &candidates);
    let (outcome, extra) = finish(
        BestOfExtra {
            cost: 0.07,
            ..Default::default()
        },
        candidates,
        true,
    );
    let outcome = outcome.ok().expect("selected native run");
    record_best_of(
        &mut task,
        0,
        BestOfSelection {
            winner: extra.winner,
            candidates: extra.candidates,
            extra_cost: extra.cost,
        },
    );
    record_run_completion(
        &mut task,
        0,
        RunCompletion {
            session: None,
            fingerprint: outcome.fingerprint,
            usage: Usage {
                input: outcome.usage_input,
                output: outcome.usage_output,
                cached: outcome.usage_cached,
            },
            first_turn_tokens: outcome.first_turn_tokens,
            cost: outcome.cost_usd,
            estimated: outcome.cost_estimated,
        },
    );
    crate::task::attempts::record_report(
        &mut task,
        0,
        crate::task::attempts::AttemptReport {
            summary: Some("report-b".into()),
            handoff: Some("b-handoff".into()),
            declines: vec![],
        },
    );
    task.record_discarded_tests(vec!["protected-b".into()]);
    app.store.save_task(&task).unwrap();
    (task, fingerprint)
}

#[tokio::test]
async fn a_saved_b_winner_is_recovered_from_b_without_replaying_a_as_b() {
    let (app, _dir) = test_app();
    let (mut task, fingerprint) = selected_b(&app);
    let saved = serde_json::to_value(&task.attempts[0]).unwrap();
    for after_recount in [false, true] {
        if after_recount {
            std::fs::remove_file(app.store.run_dir(&task.id, 1).join("events.raw")).unwrap();
            task.fixture_data().attempts[0].cost_usd = None;
            task.fixture_data().cost_usd = 0.47;
            app.store.save_task(&task).unwrap();
        }
        let recovered = recovery::interrupted::tasks(
            &app.store,
            |task, index| app.run_fate(task, index),
            |_, _| panic!("completed B is recoverable"),
        )
        .unwrap();
        assert!(recovered.is_empty());
        assert_eq!(recovery::resumable_attempt(&app, &task), Some(0));
        let executed = adopted::follow(&app, &mut task, 0, &CancelToken::new()).await;
        let outcome = executed.run_result.ok().expect("completed B outcome");
        assert_eq!(outcome.final_text.as_deref(), Some("report-b"));
        assert_eq!(outcome.fingerprint.as_ref(), Some(&fingerprint));
        assert_eq!(
            (
                outcome.usage_input,
                outcome.usage_output,
                outcome.usage_cached
            ),
            (400, 80, 20)
        );
        assert_eq!(outcome.cost_usd, Some(0.8));
        assert_eq!(executed.run.route.harness, Harness::Codex);
        assert!(executed.best_of.winner.is_none());
        assert_eq!(executed.best_of.candidates.len(), 2);
        assert_eq!(executed.best_of.discarded, ["protected-b"]);
        assert_eq!(executed.best_of.cost, 0.0);
        assert!(outcome.session_id.is_none());
        task = app.store.load_task(&task.id).unwrap().unwrap();
        let mut settlement =
            NativeSettlement::capture(&task, 0, executed.best_of.cost, &executed.best_of.discarded)
                .unwrap();
        record_best_of(
            &mut task,
            0,
            BestOfSelection {
                winner: executed.best_of.winner,
                candidates: executed.best_of.candidates,
                extra_cost: executed.best_of.cost,
            },
        );
        record_run_completion(
            &mut task,
            0,
            RunCompletion {
                session: outcome.session_id,
                fingerprint: outcome.fingerprint,
                usage: Usage {
                    input: outcome.usage_input,
                    output: outcome.usage_output,
                    cached: outcome.usage_cached,
                },
                first_turn_tokens: outcome.first_turn_tokens,
                cost: outcome.cost_usd,
                estimated: outcome.cost_estimated,
            },
        );
        assert_eq!(
            settlement.publish(&app, &mut task).unwrap(),
            Publication::Current
        );
        assert_eq!(serde_json::to_value(&task.attempts[0]).unwrap(), saved);
        assert!((task.cost_usd - 1.27).abs() < 1e-9);
        assert_eq!(crate::store::costs::read_all(&app.data_dir).len(), 3);
        app.store.save_task(&task).unwrap();
    }
}

#[tokio::test]
async fn adoption_keeps_the_saved_provider_when_the_route_id_was_reconfigured() {
    for changed_harness in [Harness::Codex, Harness::Claude] {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        let mut attempt = attempt_with_failure(1, "running");
        attempt.status = AttemptStatus::Running;
        attempt.failure = None;
        let route_id = attempt.route_id.clone();
        let harness = attempt.harness;
        let model = attempt.model.clone();
        task.fixture_data().attempts.push(attempt);
        app.store.save_task(&task).unwrap();
        let dir = app.store.run_dir(&task.id, 1);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("events.raw"), A_EVENTS).unwrap();
        std::fs::write(dir.join("events.exit"), "0\n").unwrap();
        {
            let mut settings = app.settings.write().unwrap();
            let index = settings
                .routes
                .iter()
                .position(|route| route.id == route_id)
                .unwrap();
            settings.routes[index].harness = changed_harness;
            settings.routes[index].model = Some(if changed_harness == harness {
                "changed-model".into()
            } else {
                model.clone()
            });
        }
        let executed = adopted::follow(&app, &mut task, 0, &CancelToken::new()).await;
        assert_eq!(executed.run.route.id, route_id);
        assert_eq!(executed.run.route.harness, harness);
        assert_eq!(executed.run.route.model.as_deref(), Some(model.as_str()));
    }
}

#[tokio::test]
async fn a_settled_primary_receipt_is_unchanged_by_new_prices_and_template_inputs() {
    let (app, _dir) = test_app();
    let mut route = app
        .settings
        .read()
        .unwrap()
        .routes
        .iter()
        .find(|route| route.harness == Harness::Codex)
        .unwrap()
        .clone();
    route.model = Some("gpt-5.3-codex".into());
    app.settings
        .write()
        .unwrap()
        .routes
        .iter_mut()
        .find(|configured| configured.id == route.id)
        .unwrap()
        .model = route.model.clone();
    let mut task = task_with_status(TaskStatus::Running);
    let mut attempt = attempt_with_failure(1, "running");
    attempt.route_id = route.id.clone();
    attempt.harness = route.harness;
    attempt.model = route.model.clone().unwrap_or_default();
    attempt.status = AttemptStatus::Running;
    attempt.failure = None;
    task.fixture_data().attempts.push(attempt);
    app.store.save_task(&task).unwrap();
    let mut native = replay_events(route.harness, B_EVENTS);
    crate::engine::attempts::pricing::finalize_cost(
        &app,
        route.harness,
        route.model.as_deref(),
        &mut native,
    );
    native.fingerprint = Some(Fingerprint {
        models: vec!["actual-primary".into()],
        harness: route.harness,
        harness_version: Some("saved-version".into()),
        prompt_hash: "previous-template-inputs".into(),
    });
    record_run(
        &app,
        &RunIdentity {
            id: &task.id,
            attempt_n: 1,
            track: RunTrack::Resumable,
            tag: CostTag::task("implement", &route.id),
        },
        ProviderRun {
            harness: route.harness,
            model: route.model.as_deref(),
            resume: None,
        },
        &native,
        0,
    );
    record_run_completion(
        &mut task,
        0,
        RunCompletion {
            session: native.session_id.clone(),
            fingerprint: native.fingerprint.clone(),
            usage: Usage {
                input: native.usage_input,
                output: native.usage_output,
                cached: native.usage_cached,
            },
            first_turn_tokens: native.first_turn_tokens,
            cost: native.cost_usd,
            estimated: native.cost_estimated,
        },
    );
    app.store.save_task(&task).unwrap();
    let dir = app.store.run_dir(&task.id, 1);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("events.raw"), B_EVENTS).unwrap();
    std::fs::write(dir.join("events.exit"), "0\n").unwrap();
    for price in app.settings.write().unwrap().prices.values_mut() {
        price.input *= 100.0;
        price.output *= 100.0;
        price.cached_input *= 100.0;
    }
    let executed = adopted::follow(&app, &mut task, 0, &CancelToken::new()).await;
    let outcome = executed.run_result.ok().expect("saved primary outcome");
    assert_eq!(outcome.final_text, native.final_text);
    assert_eq!(outcome.fingerprint, native.fingerprint);
    assert_eq!(outcome.cost_usd, native.cost_usd);
    assert_eq!(outcome.cost_estimated, native.cost_estimated);
    assert_eq!(outcome.session_id, native.session_id);
    assert_eq!(outcome.usage_input, native.usage_input);
    assert_eq!(outcome.usage_output, native.usage_output);
    assert_eq!(outcome.usage_cached, native.usage_cached);
    assert_eq!(task.cost_usd, native.cost_usd.unwrap());
    assert_eq!(crate::store::costs::read_all(&app.data_dir).len(), 1);
}

#[test]
fn a_saved_b_winner_without_its_events_does_not_adopt_primary_a() {
    let (app, _dir) = test_app();
    let (task, _) = selected_b(&app);
    std::fs::remove_file(app.store.run_dir(&task.id, 1).join("b/events.jsonl")).unwrap();
    let recovered = recovery::interrupted::tasks(
        &app.store,
        |task, index| app.run_fate(task, index),
        |task, index| {
            crate::engine::cost::settle_unfinished_cost(
                task,
                index,
                &app.store,
                &app.settings.read().unwrap().prices,
            )
        },
    )
    .unwrap();
    assert_eq!(recovered.len(), 1);
    let task = &recovered[0];
    assert_eq!(task.status, TaskStatus::Queued);
    assert_eq!(task.attempts[0].status, AttemptStatus::Interrupted);
    assert_eq!(task.attempts[0].summary.as_deref(), Some("report-b"));
    assert!((task.cost_usd - 1.27).abs() < 1e-9);
    assert_eq!(crate::store::costs::read_all(&app.data_dir).len(), 3);
}
