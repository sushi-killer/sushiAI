use super::run_base_preflight;
use super::spawn_base_preflight;
use super::{settle_base_preflight, BasePreflight, PreflightJob};
use crate::engine::persistence::contract::tests::pending_once;
use crate::engine::questions::parking::WaitEnd;
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::state::App;
use crate::engine::test_support::assert_helper_timeout;
use crate::engine::test_support::attempt_with_failure;
use crate::engine::test_support::slow_helper_task;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::engine::verification::healing::answers::heal_answers;
use crate::engine::verification::healing::repeated::heal_repeated_unmet;
use crate::harness::cancel::CancelToken;
use crate::task::review::ReviewResult;
use crate::task::state::TaskStatus;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn a_rewritten_verify_starts_the_preflight_again_for_the_new_commands() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().verify = vec!["echo one".into()];
    let cancel = CancelToken::new();
    let id = task.id.clone();
    let job = |app: &Arc<App>| app.base_preflights.lock().unwrap()[&id].clone();
    spawn_base_preflight(&app, &task, &cancel);
    let first = job(&app);
    // The same commands again: the job is not started twice.
    spawn_base_preflight(&app, &task, &cancel);
    assert!(Arc::ptr_eq(&first, &job(&app)));
    // A rewrite replaces the job and stops the old one.
    task.fixture_data().verify = vec!["echo two".into()];
    spawn_base_preflight(&app, &task, &cancel);
    let second = job(&app);
    assert!(!Arc::ptr_eq(&first, &second));
    assert!(second.snapshot.matches(&task));
    assert!(first.cancel.is_cancelled());
    assert!(!second.cancel.is_cancelled());
}
#[cfg(test)]
mod timeout_tests {
    use super::*;

    #[tokio::test]
    async fn feasibility_timed_out_keeps_the_command_unchanged() {
        let (app, _dir, mut task, _) = slow_helper_task();
        task.fixture_data().verify = vec!["false".into()];
        let result = run_base_preflight(&app, &task, &CancelToken::new())
            .await
            .unwrap();
        assert!(result.settled);
        assert!(result.rulings.is_empty());
        task.record_decisions(result.decisions);
        assert_eq!(task.verify, ["false"]);
        assert_helper_timeout(&task, "feasibility");
        let costs = crate::store::costs::read_all(&app.data_dir);
        assert_eq!(costs.len(), 1);
        assert!(costs[0].attempt.is_none());
    }

    #[tokio::test]
    async fn criterion_judge_timed_out_keeps_the_criterion() {
        let (app, _dir, mut task, _) = slow_helper_task();
        let review: ReviewResult = serde_json::from_value(
            json!({"verdict":"FAIL", "criteria":[{"criterion":"Keep the API","index":0,"met":false,"evidence":"missing"}]}),
        )
        .unwrap();
        task.fixture_data().attempts[0].review = Some(review.clone());
        task.fixture_data()
            .attempts
            .push(attempt_with_failure(2, "failure"));
        let criteria = task.criteria.clone();
        assert!(matches!(
            heal_repeated_unmet(&app, &mut task, 1, &review, &CancelToken::new()).await,
            Ok(crate::engine::persistence::contract::Publication::Applied(
                false
            ))
        ));
        assert_eq!(task.criteria, criteria);
        assert!(task.assumptions.is_empty());
        assert_helper_timeout(&task, "criterion-judge");
    }

    #[tokio::test]
    async fn answer_check_timed_out_keeps_the_criteria() {
        let (app, _dir, mut task, _) = slow_helper_task();
        task.record_decision("Owner: Which API? -> Remove it".into());
        app.store.save_task(&task).unwrap();
        let criteria = task.criteria.clone();
        assert!(matches!(
            heal_answers(&app, &mut task, 1, &CancelToken::new()).await,
            Ok(crate::engine::verification::healing::answers::AnswerHealing::Ready)
        ));
        assert_eq!(task.criteria, criteria);
        assert!(task.assumptions.is_empty());
        assert_helper_timeout(&task, "answer-check");
    }
}

#[tokio::test]
async fn pending_owner_edit_defers_a_completed_preflight_without_charging_twice() {
    use super::{settle_base_preflight, BasePreflight, PreflightJob};
    use crate::engine::persistence::contract::tests::{
        assert_owner_applies, pending_once, queue_owner,
    };
    use crate::engine::questions::parking::WaitEnd;
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().verify = vec!["false".into()];
    app.store.save_task(&task).unwrap();
    let cancel = CancelToken::new();
    let (tx, rx) = tokio::sync::watch::channel(None);
    app.base_preflights.lock().unwrap().insert(
        task.id.clone(),
        Arc::new(PreflightJob {
            result: rx,
            applied: std::sync::atomic::AtomicBool::new(false),
            snapshot: crate::task::checks::snapshot::CheckSnapshot::capture(&task),
            cancel: cancel.clone(),
        }),
    );
    let observed = task.clone();
    {
        let future = settle_base_preflight(&app, &mut task, &cancel);
        tokio::pin!(future);
        pending_once(future.as_mut()).await;
        queue_owner(&app, &observed);
        tx.send(Some(Arc::new(BasePreflight {
            settled: true,
            cost: 0.4,
            decisions: vec!["feasibility helper completed".into()],
            rulings: vec![(
                crate::task::VerifyOutcome {
                    command: "false".into(),
                    code: Some(1),
                    tail: String::new(),
                    ..Default::default()
                },
                crate::verify::healing::reply::Feasibility::Rewrite {
                    command: "true".into(),
                    criterion: None,
                    reason: "unavailable".into(),
                },
            )],
        })))
        .unwrap();
        assert_eq!(future.await, Err(WaitEnd::Changed));
    }
    assert_eq!(
        settle_base_preflight(&app, &mut task, &cancel).await,
        Ok(())
    );
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.verify, ["false"]);
    assert!(!saved.brief_check.feasibility_done);
    assert_eq!(saved.cost_usd, 0.4);
    assert_eq!(task.cost_usd, 0.4);
    assert_eq!(
        saved
            .decisions
            .iter()
            .filter(|line| line.as_str() == "feasibility helper completed")
            .count(),
        1
    );
    assert_owner_applies(&app, &task);
}

#[tokio::test]
async fn an_obsolete_preflight_is_discarded_without_replaying_the_carried_native_run() {
    for owner_edit in [false, true] {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().criteria = vec!["Keep the requested behavior".into()];
        task.fixture_data().verify = vec!["false".into()];
        app.store.save_task(&task).unwrap();
        let old = task.clone();
        let snapshot = crate::task::checks::snapshot::CheckSnapshot::capture(&task);
        let (tx, rx) = tokio::sync::watch::channel(None);
        let signals = TaskSignals::new();
        let cancel = signals.cancel.clone();
        app.controls.lock().unwrap().insert(
            task.id.clone(),
            TaskControl {
                signals,
                handle: tokio::spawn(std::future::pending()),
            },
        );
        app.base_preflights.lock().unwrap().insert(
            task.id.clone(),
            Arc::new(PreflightJob {
                result: rx,
                applied: std::sync::atomic::AtomicBool::new(false),
                snapshot,
                cancel: cancel.clone(),
            }),
        );
        crate::task::transitions::move_base(&mut task, "new-base".into());
        assert!(matches!(
            crate::engine::delivery_gate::publish_carry(&app, &task, &old, &cancel).unwrap(),
            crate::engine::delivery_gate::DeliveryGate::Ready
        ));
        let id = task.id.clone();
        {
            let future = settle_base_preflight(&app, &mut task, &cancel);
            tokio::pin!(future);
            pending_once(future.as_mut()).await;
            if owner_edit {
                let reply = app
                    .dispatch(
                        "task.amend",
                        json!({"id":id,
                    "criteria":["Owner's accepted replacement"]}),
                    )
                    .await
                    .unwrap();
                assert_eq!(reply["pending"], true);
            }
            tx.send(Some(Arc::new(BasePreflight {
                settled: true,
                cost: 0.4,
                decisions: vec!["old-base helper completed".into()],
                rulings: vec![(
                    crate::task::VerifyOutcome {
                        command: "false".into(),
                        code: Some(1),
                        ..Default::default()
                    },
                    crate::verify::healing::reply::Feasibility::Rewrite {
                        command: "true".into(),
                        criterion: None,
                        reason: "old base".into(),
                    },
                )],
            })))
            .unwrap();
            assert_eq!(
                future.await,
                if owner_edit {
                    Err(WaitEnd::Changed)
                } else {
                    Ok(())
                }
            );
        }
        assert_eq!(
            settle_base_preflight(&app, &mut task, &cancel).await,
            Ok(())
        );
        let saved = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!(saved.verify, ["false"]);
        assert_eq!(saved.base_sha, "new-base");
        assert!(!saved.brief_check.feasibility_done);
        assert_eq!(saved.cost_usd, 0.4);
        assert_eq!(task.cost_usd, 0.4);
        assert_eq!(saved.pending_amendment.is_some(), owner_edit);
        assert!(!app.base_preflights.lock().unwrap().contains_key(&id));
        app.controls
            .lock()
            .unwrap()
            .remove(&id)
            .unwrap()
            .handle
            .abort();
    }
}
