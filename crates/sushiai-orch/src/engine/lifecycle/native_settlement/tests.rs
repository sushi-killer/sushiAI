use super::*;
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::test_support::test_app;
use crate::policy::routing::Harness;
use crate::store::costs::{append, costs_path, CostRecord};
use crate::task::amendment::{Amendment, PendingAmendment};
use crate::task::attempts::{
    record_best_of, record_run_completion, BestOfSelection, RunCompletion,
};
use crate::task::checks::authority::Origin;
use crate::task::test_support::sample_task;
use crate::task::{AttemptStatus, Candidate, CheckTally, Fingerprint, Usage};

fn task() -> Task {
    sample_task(
        "native-publication",
        TaskStatus::Running,
        AttemptStatus::Running,
    )
}

fn selection(task: &mut Task) {
    let mut route = crate::policy::settings::Settings::default().routes[0].clone();
    route.id = "b".into();
    route.harness = Harness::Claude;
    route.model = Some("opus".into());
    record_best_of(
        task,
        0,
        BestOfSelection {
            winner: Some(route),
            candidates: [("a", 0.4, false), ("b", 0.8, true)]
                .into_iter()
                .map(|(route, cost, picked)| Candidate {
                    route: route.into(),
                    cost,
                    verify: true,
                    checks: CheckTally::default(),
                    picked,
                })
                .collect(),
            extra_cost: 0.47,
        },
    );
}

fn completion(task: &mut Task) {
    record_run_completion(
        task,
        0,
        RunCompletion {
            session: None,
            fingerprint: Some(Fingerprint {
                models: vec!["opus".into()],
                harness: Harness::Claude,
                harness_version: Some("test".into()),
                prompt_hash: "brief".into(),
            }),
            usage: Usage {
                input: 100,
                output: 20,
                cached: 50,
            },
            first_turn_tokens: Some(150),
            cost: Some(0.8),
            estimated: false,
        },
    );
    crate::task::attempts::record_gate_blocks(task, 0, 3);
}

fn ledger(app: &App, task: &Task) -> Vec<u8> {
    for (stage, route, cost) in [
        ("implement", "a", 0.4),
        ("implement", "b", 0.8),
        ("pick", "picker", 0.07),
    ] {
        append(
            &app.data_dir,
            &CostRecord {
                ts: 2,
                repo: task.repo.clone(),
                task_id: Some(task.id.clone()),
                run_id: None,
                stage: stage.into(),
                attempt: Some(1),
                route_id: route.into(),
                harness: Harness::Claude,
                model: "model".into(),
                cost_usd: cost,
                estimated: false,
                input_tokens: 100,
                cached_tokens: 50,
                output_tokens: 20,
                ms: 1,
                backfilled: false,
            },
        );
    }
    std::fs::read(costs_path(&app.data_dir)).unwrap()
}

#[test]
fn selected_native_receipt_is_durable_before_questions_without_rewriting_the_ledger() {
    let (app, _dir) = test_app();
    let mut local = task();
    app.store.save_task(&local).unwrap();
    let ledger_before = ledger(&app, &local);
    let mut settlement = NativeSettlement::capture(&local, 0, 0.47, &[]).unwrap();
    selection(&mut local);
    completion(&mut local);
    assert_eq!(
        settlement.publish(&app, &mut local).unwrap(),
        Publication::Current
    );
    let saved = app.store.load_task(&local.id).unwrap().unwrap();
    let receipt = serde_json::to_value(&saved.attempts[0]).unwrap();
    assert_eq!(saved.attempts[0].route_id, "b");
    assert_eq!(saved.attempts[0].model, "opus");
    assert_eq!(saved.attempts[0].session_id, None);
    assert_eq!(
        saved.attempts[0].fingerprint.as_ref().unwrap().models,
        ["opus"]
    );
    assert_eq!(saved.attempts[0].usage.as_ref().unwrap().cached, 50);
    assert_eq!(saved.attempts[0].prefix_tokens, Some(150));
    assert_eq!(saved.attempts[0].gate_blocks, 3);
    assert!((saved.cost_usd - 1.27).abs() < 1e-9);
    assert_eq!(
        settlement.publish(&app, &mut local).unwrap(),
        Publication::Current
    );
    let saved = app.store.load_task(&local.id).unwrap().unwrap();
    assert_eq!(serde_json::to_value(&saved.attempts[0]).unwrap(), receipt);
    assert_eq!(local.cost_usd, saved.cost_usd);
    assert_eq!(
        std::fs::read(costs_path(&app.data_dir)).unwrap(),
        ledger_before
    );
    assert_eq!(crate::store::costs::read_all(&app.data_dir).len(), 3);
}

#[test]
fn concurrent_owner_contract_and_stop_survive_both_native_publications() {
    for stop in [false, true] {
        let (app, _dir) = test_app();
        let mut local = task();
        app.store.save_task(&local).unwrap();
        let mut settlement = NativeSettlement::capture(&local, 0, 0.47, &[]).unwrap();
        selection(&mut local);
        app.update_task(&local.id, |fresh| {
            fresh.fixture_data().criteria = vec!["Owner's new contract".into()];
            fresh.fixture_data().attempts[0].summary = Some("Owner's retained observation".into());
            fresh.charge(0.03);
            if stop {
                fresh.resume_phase(TaskStatus::Stopped, 2);
                fresh.interrupt_running_attempts(2);
            }
        })
        .unwrap();
        let expected = if stop {
            Publication::Stopped
        } else {
            Publication::Changed
        };
        assert_eq!(settlement.publish(&app, &mut local).unwrap(), expected);
        completion(&mut local);
        assert_eq!(settlement.publish(&app, &mut local).unwrap(), expected);
        let saved = app.store.load_task(&local.id).unwrap().unwrap();
        assert_eq!(saved.criteria, ["Owner's new contract"]);
        assert_eq!(
            saved.status,
            if stop {
                TaskStatus::Stopped
            } else {
                TaskStatus::Running
            }
        );
        assert_eq!(
            saved.attempts[0].summary.as_deref(),
            Some("Owner's retained observation")
        );
        assert_eq!(
            saved.attempts[0].status,
            if stop {
                AttemptStatus::Interrupted
            } else {
                AttemptStatus::Running
            }
        );
        assert_eq!(saved.attempts[0].ended_at, stop.then_some(2));
        assert_eq!(saved.attempts[0].route_id, "b");
        assert!((saved.cost_usd - 1.30).abs() < 1e-9);
        assert_eq!(local.cost_usd, saved.cost_usd);
    }
}

#[tokio::test]
async fn a_pending_owner_amendment_is_preserved_while_the_native_receipt_is_saved() {
    let (app, _dir) = test_app();
    let mut local = task();
    app.store.save_task(&local).unwrap();
    let mut settlement = NativeSettlement::capture(&local, 0, 0.47, &[]).unwrap();
    selection(&mut local);
    let signals = TaskSignals::new();
    *signals.pending_amend.lock().unwrap() = Some(PendingAmendment::capture(
        &local,
        Amendment {
            criteria: Some(vec!["Pending owner contract".into()]),
            ..Default::default()
        },
        Origin::Owner,
    ));
    let mut queued = app.store.load_task(&local.id).unwrap().unwrap();
    queued.fixture_data().pending_amendment = signals.pending_amend.lock().unwrap().clone();
    app.store.save_task_with_pending(&queued, None).unwrap();
    let pending = signals.pending_amend.clone();
    app.controls.lock().unwrap().insert(
        local.id.clone(),
        TaskControl {
            signals,
            handle: tokio::spawn(async {}),
        },
    );
    assert_eq!(
        settlement.publish(&app, &mut local).unwrap(),
        Publication::Changed
    );
    let control = app.controls.lock().unwrap().remove(&local.id).unwrap();
    control.handle.await.unwrap();
    completion(&mut local);
    assert_eq!(
        settlement.publish(&app, &mut local).unwrap(),
        Publication::Changed
    );
    assert!(pending.lock().unwrap().is_some());
    let saved = app.store.load_task(&local.id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["works"]);
    assert_eq!(saved.attempts[0].route_id, "b");
    assert!((saved.cost_usd - 1.27).abs() < 1e-9);
}

#[test]
fn a_persistence_failure_is_returned_and_keeps_selection_spend_unpublished() {
    let (app, _dir) = test_app();
    let mut local = task();
    app.store.save_task(&local).unwrap();
    let mut settlement = NativeSettlement::capture(&local, 0, 0.47, &[]).unwrap();
    selection(&mut local);
    let path = app.store.task_path(&local.id);
    let backup = path.with_extension("backup");
    std::fs::rename(&path, &backup).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(settlement.publish(&app, &mut local).is_err());
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&backup, &path).unwrap();
    settlement.publish(&app, &mut local).unwrap();
    let saved = app.store.load_task(&local.id).unwrap().unwrap();
    assert!((saved.cost_usd - 0.47).abs() < 1e-9);
}

#[test]
fn a_failed_selection_is_durable_before_a_changed_failure_question() {
    let (app, _dir) = test_app();
    let mut local = task();
    app.store.save_task(&local).unwrap();
    let mut settlement = NativeSettlement::capture(&local, 0, 0.47, &[]).unwrap();
    record_best_of(
        &mut local,
        0,
        BestOfSelection {
            winner: None,
            candidates: Vec::new(),
            extra_cost: 0.47,
        },
    );
    app.update_task(&local.id, |fresh| {
        fresh.fixture_data().goal = "Owner changed the work".into();
    })
    .unwrap();
    assert_eq!(
        settlement.publish(&app, &mut local).unwrap(),
        Publication::Changed
    );
    assert_eq!(
        settlement.publish(&app, &mut local).unwrap(),
        Publication::Changed
    );
    let saved = app.store.load_task(&local.id).unwrap().unwrap();
    assert_eq!(saved.goal, "Owner changed the work");
    assert!(saved.attempts[0].candidates.is_empty());
    assert_eq!(saved.attempts[0].cost_usd, None);
    assert!((saved.cost_usd - 0.47).abs() < 1e-9);
}

#[test]
fn native_discarded_evidence_survives_changed_contract_and_repeated_publication() {
    let (app, _dir) = test_app();
    let mut local = task();
    app.store.save_task(&local).unwrap();
    let discarded = vec!["native.rs".into(), "shared.rs".into()];
    local.record_discarded_tests(discarded.clone());
    let mut settlement = NativeSettlement::capture(&local, 0, 0.47, &discarded).unwrap();
    selection(&mut local);
    completion(&mut local);
    let fresh = app
        .update_task(&local.id, |fresh| {
            fresh.fixture_data().criteria = vec!["Owner's changed contract".into()];
            fresh.fixture_data().planner_tests.files.push(
                crate::task::checks::types::PlannedTest {
                    path: "owner.rs".into(),
                    content: "Owner's retained test".into(),
                },
            );
            fresh.record_discarded_tests(vec!["fresh.rs".into(), "shared.rs".into()]);
        })
        .unwrap();
    let contract = crate::task::checks::snapshot::CheckSnapshot::capture(&fresh);
    assert_eq!(
        settlement.publish(&app, &mut local).unwrap(),
        Publication::Changed
    );
    let saved = app.store.load_task(&local.id).unwrap().unwrap();
    assert!(contract.matches(&saved));
    assert_eq!(
        saved.planner_tests.discarded,
        ["fresh.rs", "native.rs", "shared.rs"]
    );
    assert_eq!(local.planner_tests.discarded, saved.planner_tests.discarded);
    let mut receipt = serde_json::to_value(&saved).unwrap();
    settlement.publish(&app, &mut local).unwrap();
    let mut resumed = saved;
    let mut empty = NativeSettlement::capture(&resumed, 0, 0.0, &[]).unwrap();
    assert_eq!(
        empty.publish(&app, &mut resumed).unwrap(),
        Publication::Current
    );
    let after = serde_json::to_value(app.store.load_task(&local.id).unwrap().unwrap()).unwrap();
    // Saving the same receipt still advances the store's observation timestamp.
    receipt["updatedAt"] = after["updatedAt"].clone();
    assert_eq!(after, receipt);
    assert!((resumed.cost_usd - 1.27).abs() < 1e-9);
}

#[test]
fn an_owner_question_survives_native_completion_publication() {
    use crate::task::{AskedBy, Question, QuestionKind};
    let (app, _dir) = test_app();
    let mut local = task();
    app.store.save_task(&local).unwrap();
    let mut settlement = NativeSettlement::capture(&local, 0, 0.0, &[]).unwrap();
    completion(&mut local);
    let owner = app
        .update_task(&local.id, |fresh| {
            fresh.ask(
                Question::new(
                    "Owner's question",
                    vec!["continue".into()],
                    QuestionKind::AgentQuestion,
                    AskedBy::Implement,
                    2,
                ),
                2,
            );
        })
        .unwrap();
    let question = serde_json::to_value(&owner.question).unwrap();
    assert_eq!(
        settlement.publish(&app, &mut local).unwrap(),
        Publication::Changed
    );
    let saved = app.store.load_task(&local.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Waiting);
    assert_eq!(serde_json::to_value(&saved.question).unwrap(), question);
    assert_eq!(saved.attempts[0].cost_usd, Some(0.8));
}

#[tokio::test]
async fn cancellation_saves_completed_native_spend_before_reporting_stopped() {
    let (app, _dir) = test_app();
    let mut local = task();
    app.store.save_task(&local).unwrap();
    let mut settlement = NativeSettlement::capture(&local, 0, 0.0, &[]).unwrap();
    completion(&mut local);
    let signals = TaskSignals::new();
    signals.cancel.cancel();
    app.controls.lock().unwrap().insert(
        local.id.clone(),
        TaskControl {
            signals,
            handle: tokio::spawn(async {}),
        },
    );
    assert_eq!(
        settlement.publish(&app, &mut local).unwrap(),
        Publication::Stopped
    );
    let saved = app.store.load_task(&local.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Running);
    assert_eq!(saved.attempts[0].cost_usd, Some(0.8));
    assert_eq!(saved.cost_usd, 0.8);
}

async fn retry_timed_out_native(app: &std::sync::Arc<App>, task: &mut Task) {
    use crate::engine::lifecycle::execution::RunReceipt;
    use crate::engine::lifecycle::inspection::{inspect, InspectionEnd};
    use crate::engine::lifecycle::native_result::ObservedRun;
    use crate::engine::questions::parking::QuestionSlot;
    let settings = app.settings.read().unwrap().clone();
    let observed = ObservedRun {
        run: RunReceipt {
            route: settings.routes[0].clone(),
            settings,
            attempt_n: task.attempts[0].n,
            idx: 0,
            worktree: task.worktree.clone().into(),
            base_sha: task.base_sha.clone(),
            run_dir: app.store.task_dir(&task.id).join("runs/1"),
        },
        outcome: crate::harness::RunOutcome {
            time_budget: true,
            error: Some("The native attempt reached its wall budget".into()),
            ..Default::default()
        },
        handled: Default::default(),
        discarded: Vec::new(),
    };
    let signals = TaskSignals::new();
    let mut permit = None;
    let mut slot = QuestionSlot::new(&signals, &mut permit);
    let Err(InspectionEnd::Failed(failure)) = inspect(app, task, &observed, 2, &mut slot).await
    else {
        panic!("the timed-out native receipt must reach the actual failure path");
    };
    assert_eq!(failure.kind, crate::task::FailureKind::TimeBudget);
    assert!(matches!(
        crate::engine::decision::fail_and_continue(app, task, failure, &mut 2, &mut slot).await,
        crate::engine::decision::LoopSignal::Continue
    ));
}

#[tokio::test]
async fn an_owner_revocation_between_native_capture_and_publication_survives_failure_retry() {
    let (app, dir) = test_app();
    let mut local = task();
    local.fixture_data().id = uuid::Uuid::new_v4().to_string();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    crate::engine::test_support::init_git_repo(&repo);
    local.fixture_data().repo = repo.display().to_string();
    local.fixture_data().worktree = local.repo.clone();
    local.fixture_data().base_sha =
        crate::engine::test_support::git_in(&repo, &["rev-parse", "HEAD"]);
    local.record_assumption(crate::task::Assumption {
        question: "Accept the inherited failure?".into(),
        answer: crate::task::questions::WAIVED.into(),
        evidence: "The base has the same named failure".into(),
        by: crate::task::questions::WAIVER_BY.into(),
        kind: Some(crate::task::QuestionKind::PreexistingFailure),
        attempt: None,
        commands: vec!["check".into()],
        overturned: false,
        owner_answer: None,
    });
    app.store.save_task(&local).unwrap();
    let ledger_before = ledger(&app, &local);
    let mut settlement = NativeSettlement::capture(&local, 0, 0.47, &[]).unwrap();
    let (ready, waiting) = tokio::sync::oneshot::channel();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let worker_app = app.clone();
    let id = local.id.clone();
    let worker = tokio::spawn(async move {
        ready.send(()).unwrap();
        blocked.await.unwrap();
        selection(&mut local);
        completion(&mut local);
        let publication = settlement.publish(&worker_app, &mut local).unwrap();
        assert_eq!(
            settlement.publish(&worker_app, &mut local).unwrap(),
            publication
        );
        if publication == Publication::Current {
            retry_timed_out_native(&worker_app, &mut local).await;
        }
        (publication, local)
    });
    waiting.await.unwrap();
    app.dispatch(
        "task.overturn",
        serde_json::json!({"id": id, "index": 0, "answer": "This check must pass"}),
    )
    .await
    .unwrap();
    release.send(()).unwrap();
    let (publication, local) = worker.await.unwrap();
    let saved = app.store.load_task(&id).unwrap().unwrap();
    assert!(
        saved.assumptions[0].overturned,
        "the failure retry saved a stale waiver"
    );
    assert_eq!(
        saved.assumptions[0].owner_answer.as_deref(),
        Some("This check must pass")
    );
    assert_eq!(publication, Publication::Changed);
    assert_eq!(saved.attempts[0].status, AttemptStatus::Running);
    assert!((saved.cost_usd - 1.27).abs() < 1e-9);
    assert_eq!(local.cost_usd, saved.cost_usd);
    let usage = saved.attempts[0].usage.as_ref().unwrap();
    assert_eq!((usage.input, usage.output, usage.cached), (100, 20, 50));
    assert_eq!(
        std::fs::read(costs_path(&app.data_dir)).unwrap(),
        ledger_before
    );
}
