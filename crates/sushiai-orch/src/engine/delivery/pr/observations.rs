use super::carried_work::CarriedWork;
use crate::engine::state::App;
use crate::task::checks::admission::CheckAdmission;
use crate::task::{Task, TaskStatus};

pub(super) enum CarryPublication {
    Current(Box<Task>),
    Changed(Box<Task>),
    Deferred,
}

/// Saves actual delivery measurements even without an implement attempt.
/// A changed contract keeps its cost and raw receipts, but adopts no waiver or base.
pub(super) fn publish(
    app: &App,
    before: &Task,
    carried: &CarriedWork,
    expected_head: &str,
) -> Result<CarryPublication, String> {
    let lock = app.task_lock(&before.id);
    let _guard = lock.lock().unwrap();
    let mut fresh = app
        .store
        .load_task(&before.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    fresh.charge((carried.work.cost_usd - before.cost_usd).max(0.0));
    fresh.record_delivery_checks(
        carried.work.delivery_checks[before.delivery_checks.len()..].to_vec(),
    );
    let active = fresh.status == TaskStatus::Done
        && !fresh.archived
        && fresh.queue.park.is_none()
        && fresh.pending_amendment.is_none()
        && fresh.pr_url.is_none()
        && fresh.landed_sha.is_none();
    let pending = app
        .controls
        .lock()
        .unwrap()
        .get(&before.id)
        .is_some_and(|control| {
            control.signals.cancel.is_cancelled()
                || control.signals.pending_amend.lock().unwrap().is_some()
        });
    let active = active && !pending;
    let current = active
        && fresh.branch == before.branch
        && fresh.base_ref == before.base_ref
        && CheckAdmission::capture(before).matches(&fresh);
    let mut moved = Ok(());
    if current {
        fresh.record_decisions(carried.work.decisions[carried.before..].iter().cloned());
        for assumption in &carried.work.assumptions[before.assumptions.len()..] {
            fresh.record_assumption(assumption.clone());
        }
        if let Ok(sha) = &carried.committed {
            moved = crate::git::command::run_bounded(
                std::path::Path::new(&fresh.repo),
                &[
                    "update-ref",
                    &format!("refs/heads/{}", fresh.branch),
                    sha,
                    expected_head,
                ],
                crate::delivery::pr::command::GIT_TIMEOUT,
                std::time::Duration::from_secs(2),
            )
            .map(|_| ())
            .map_err(|error| error.to_string());
        }
        if carried.committed.is_ok() && moved.is_ok() {
            crate::task::transitions::move_base(&mut fresh, carried.work.base_sha.clone());
            crate::task::checks::edits::adopt_planner_tests(
                &mut fresh,
                carried.work.planner_tests.clone(),
            );
        }
    }
    fresh.touch(crate::clock::now_ms());
    app.store
        .save_task(&fresh)
        .map_err(|error| error.to_string())?;
    app.broadcast_task(&fresh);
    moved?;
    Ok(if current {
        CarryPublication::Current(Box::new(fresh))
    } else if active {
        CarryPublication::Changed(Box::new(fresh))
    } else {
        CarryPublication::Deferred
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{task_with_status, test_app};
    use crate::task::VerifyOutcome;

    #[test]
    fn a_parent_without_an_attempt_publishes_real_delivery_measurements_and_cost() {
        let (app, _dir) = test_app();
        let before = task_with_status(TaskStatus::Done);
        app.store.save_task(&before).unwrap();
        let mut work = before.clone();
        work.charge(0.25);
        work.record_delivery_checks(vec![VerifyOutcome {
            command: "actual-command".into(),
            code: Some(1),
            tail: "actual-output".into(),
            tree: Some("measured-tree".into()),
            ..Default::default()
        }]);
        let carried = CarriedWork {
            before: 0,
            work,
            committed: Err("red".into()),
            conflicted: false,
            contained: false,
            failed_check: None,
        };
        assert!(matches!(
            publish(&app, &before, &carried, "").unwrap(),
            CarryPublication::Current(_)
        ));
        let saved = app.store.load_task(&before.id).unwrap().unwrap();
        assert!(saved.attempts.is_empty());
        assert_eq!(saved.delivery_checks.len(), 1);
        assert_eq!(saved.delivery_checks[0].command, "actual-command");
        assert_eq!(saved.delivery_checks[0].tail, "actual-output");
        assert_eq!(
            saved.delivery_checks[0].tree.as_deref(),
            Some("measured-tree")
        );
        assert!((saved.cost_usd - 0.25).abs() < 1e-9);
    }

    #[tokio::test]
    async fn an_owner_overturn_during_post_carry_judgment_defers_pr_and_keeps_paid_observations() {
        let (app, _dir) = test_app();
        let mut before = task_with_status(TaskStatus::Done);
        before.record_assumption(crate::task::Assumption {
            question: "Accept the inherited check?".into(),
            answer: crate::task::questions::WAIVED.into(),
            evidence: "The base has the same named failure".into(),
            by: crate::task::questions::WAIVER_BY.into(),
            kind: Some(crate::task::QuestionKind::PreexistingFailure),
            attempt: None,
            commands: vec!["check".into()],
            overturned: false,
            owner_answer: None,
        });
        app.store.save_task(&before).unwrap();
        let mut work = before.clone();
        work.charge(0.25);
        work.record_delivery_checks(vec![VerifyOutcome {
            command: "check".into(),
            code: Some(1),
            tail: "actual carry receipt".into(),
            ..Default::default()
        }]);
        let mut replacement = before.assumptions[0].clone();
        replacement.evidence = "new base judgment".into();
        work.record_assumption(replacement);
        let carried = CarriedWork {
            before: 0,
            work,
            committed: Ok("carried-tip".into()),
            conflicted: false,
            contained: false,
            failed_check: None,
        };
        let id = before.id.clone();
        let worker_app = app.clone();
        let (release, blocked) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            blocked.await.unwrap();
            publish(&worker_app, &before, &carried, "").unwrap()
        });
        app.dispatch(
            "task.overturn",
            serde_json::json!({"id":id,"index":0,"answer":"Keep this check"}),
        )
        .await
        .unwrap();
        release.send(()).unwrap();
        let result = worker.await.unwrap();
        let saved = app.store.load_task(&id).unwrap().unwrap();
        assert!(
            matches!(result, CarryPublication::Changed(_)),
            "a stale base waiver admitted the pull request"
        );
        assert!(saved.assumptions[0].overturned);
        assert_eq!(
            saved.assumptions.len(),
            1,
            "the revoked waiver was recreated"
        );
        assert!(crate::delivery::pr::body::waived_section(&saved).is_empty());
        assert_eq!(saved.cost_usd, 0.25);
        assert_eq!(saved.delivery_checks.len(), 1);
        assert_eq!(saved.delivery_checks[0].tail, "actual carry receipt");
        assert!(saved.pr_url.is_none());
    }
}
