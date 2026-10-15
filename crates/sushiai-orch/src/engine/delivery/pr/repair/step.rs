use super::{cause, detail, question};
use crate::clock::now_ms;
use crate::engine::baseline::{judge_check, BaseVerdict};
use crate::engine::decision::failure;
use crate::engine::delivery_gate::DeliveryGate;
use crate::engine::lifecycle::attempt_files::AttemptFiles;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::questions::wake::wake_orchestrator_for_waiting;
use crate::engine::state::App;
use crate::engine::verification::base::answers::{command_named_by_task, is_kept};
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::runner::CheckRunner;
use crate::harness::cancel::CancelToken;
use crate::task::attempts::{end, AttemptEnd};
use crate::task::checks::admission::CheckAdmission;
use crate::task::{FailureKind, Task, TaskStatus, VerifyOutcome};
use std::sync::Arc;

pub(in crate::engine) enum StepOutcome {
    NotRepair,
    Waived,
    Changed,
    Retry,
    Waiting,
    Stopped,
}

/// Applies the same repair cap before `done`, when the repair's own checks fail.
pub(in crate::engine) async fn settle_step(
    app: &Arc<App>,
    task: &mut Task,
    files: &AttemptFiles,
    failed: &VerifyOutcome,
    slot: &mut QuestionSlot<'_>,
) -> StepOutcome {
    if !task
        .queue
        .pr_check_repair
        .as_ref()
        .is_some_and(|repair| repair.step == files.n)
    {
        return StepOutcome::NotRepair;
    }
    let Ok(Some(stored)) = app.store.load_task(&task.id) else {
        return StepOutcome::Stopped;
    };
    let (id, base) = (task.id.clone(), task.base_sha.clone());
    let sandbox = app.task_sandbox(&task.repo, &files.worktree, &files.run_dir);
    let runner = CheckRunner::from_context(
        CheckContext::new(app, Some(&id)).with_gate("PR repair checks"),
        &files.worktree,
        sandbox.as_ref(),
        &base,
        slot.cancel(),
    );
    let mut next = task.clone();
    next.record_delivery_checks(vec![failed.clone()]);
    let verdict = if is_kept(task, &failed.command) || command_named_by_task(task, &failed.command)
    {
        BaseVerdict::NewFailures(Box::new(failed.clone()))
    } else {
        judge_check(app, &mut next, failed, &files.run_dir, runner).await
    };
    let outcome = match verdict {
        BaseVerdict::Waived => StepOutcome::Waived,
        BaseVerdict::NewFailures(ref failed) => settle_failure(&mut next, files, failed),
        BaseVerdict::Unclear => settle_failure(&mut next, files, failed),
    };
    match publish(app, task, &stored, next, slot.cancel()) {
        Ok(DeliveryGate::Ready) => {
            if matches!(outcome, StepOutcome::Waiting) {
                wake_orchestrator_for_waiting(app, task);
            }
            outcome
        }
        Ok(DeliveryGate::Recheck) => StepOutcome::Changed,
        Ok(DeliveryGate::Stopped) => StepOutcome::Stopped,
        Err(error) => {
            eprintln!(
                "orch: publishing PR check repair for {} failed: {error}",
                task.id
            );
            StepOutcome::Stopped
        }
    }
}

fn settle_failure(task: &mut Task, files: &AttemptFiles, failed: &VerifyOutcome) -> StepOutcome {
    let failure = failure(FailureKind::Verify, detail(failed));
    end(
        task,
        files.idx,
        AttemptEnd::Failed(failure.clone()),
        now_ms(),
    );
    let retry = task.schedule_pr_check_repair(cause(failed), failure);
    task.record_pr_pending(true);
    if retry {
        task.resume_phase(TaskStatus::Running, now_ms());
        StepOutcome::Retry
    } else {
        task.ask(
            question(failed, "the same check failure survived its repair step"),
            now_ms(),
        );
        StepOutcome::Waiting
    }
}

fn publish(
    app: &App,
    task: &mut Task,
    stored: &Task,
    mut next: Task,
    cancel: &CancelToken,
) -> Result<DeliveryGate, String> {
    let lock = app.task_lock(&task.id);
    let guard = lock.lock().unwrap();
    let mut fresh = app
        .store
        .load_task(&task.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    let cost = next.cost_usd - task.cost_usd;
    let stopped = cancel.is_cancelled()
        || fresh.archived
        || fresh.queue.park.is_some()
        || matches!(
            fresh.status,
            TaskStatus::Stopped | TaskStatus::Done | TaskStatus::Failed
        );
    let changed = fresh.status != stored.status
        || fresh.question.is_some()
        || fresh.pending_amendment.is_some()
        || !CheckAdmission::capture(task).matches(&fresh)
        || fresh
            .attempts
            .last()
            .zip(task.attempts.last())
            .is_none_or(|(left, right)| left.n != right.n || left.started_at != right.started_at);
    let outcome = if stopped || changed {
        fresh.charge(cost);
        fresh.record_delivery_checks(next.delivery_checks[task.delivery_checks.len()..].to_vec());
        *task = fresh;
        if stopped {
            DeliveryGate::Stopped
        } else {
            DeliveryGate::Recheck
        }
    } else {
        next.charge(fresh.cost_usd - stored.cost_usd);
        *task = next;
        DeliveryGate::Ready
    };
    task.touch(now_ms());
    app.store
        .save_task(task)
        .map_err(|error| error.to_string())?;
    drop(guard);
    app.broadcast_task(task);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::run_control::TaskSignals;
    use crate::engine::test_support::{
        attempt_with_failure, git_in, init_git_repo, task_with_status, test_app,
    };
    use crate::policy::settings::SandboxMode;
    use crate::task::AttemptStatus;

    #[tokio::test]
    async fn the_repair_still_red_before_done_ends_at_one_owner_question() {
        assert_one_owner_question(None).await;
    }

    #[tokio::test]
    async fn an_interrupted_repair_replacement_still_ends_at_one_owner_question() {
        assert_one_owner_question(Some(AttemptEnd::Interrupted)).await;
    }

    #[tokio::test]
    async fn a_native_timeout_repair_continuation_still_ends_at_one_owner_question() {
        assert_one_owner_question(Some(AttemptEnd::Failed(failure(
            FailureKind::TimeBudget,
            "native wall-clock limit".into(),
        ))))
        .await;
    }

    async fn assert_one_owner_question(replacement: Option<AttemptEnd>) {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.to_string_lossy().into_owned();
        task.fixture_data().worktree = task.repo.clone();
        task.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
        let failed = VerifyOutcome {
            command: "printf 'not ok 1 - case_17\\n'; exit 1".into(),
            code: Some(1),
            failing: vec!["case_17".into()],
            tail: "not ok 1 - case_17".into(),
            ..Default::default()
        };
        task.schedule_pr_check_repair(
            cause(&failed),
            failure(FailureKind::Verify, detail(&failed)),
        );
        let step = task.queue.pr_check_repair.as_ref().unwrap().step;
        let mut attempt = attempt_with_failure(step, "old");
        attempt.status = AttemptStatus::Running;
        attempt.failure = None;
        task.fixture_data().attempts.push(attempt);
        task.record_pr_pending(true);
        app.base_runs.lock().unwrap().insert(
            (task.base_sha.clone(), failed.command.clone()),
            VerifyOutcome {
                command: failed.command.clone(),
                code: Some(0),
                ..Default::default()
            },
        );
        let index = if let Some(interruption) = replacement {
            end(&mut task, 0, interruption, now_ms());
            let restored: Task =
                serde_json::from_value(serde_json::to_value(&task).unwrap()).unwrap();
            task = restored;
            let route = app.settings.read().unwrap().routes[0].clone();
            let index = crate::task::attempts::start(
                &mut task,
                crate::task::Stage::Implement,
                &route,
                "replace interrupted repair",
                now_ms(),
            );
            assert_eq!(task.queue.pr_check_repair.as_ref().unwrap().step, step + 1);
            index
        } else {
            0
        };
        let step = task.attempts[index].n;
        app.store.save_task(&task).unwrap();
        let files = AttemptFiles::new(index, step, repo, dir.path().join("runs"));
        let signals = TaskSignals::new();
        let mut permit = None;
        let result = settle_step(
            &app,
            &mut task,
            &files,
            &failed,
            &mut QuestionSlot::new(&signals, &mut permit),
        )
        .await;
        assert!(matches!(result, StepOutcome::Waiting));
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.status, TaskStatus::Waiting);
        assert!(saved.pr_pending && saved.pr_url.is_none());
        assert_eq!(saved.attempts[index].status, AttemptStatus::Failed);
        assert_eq!(saved.queue.pr_check_repair.as_ref().unwrap().step, step);
        let question = saved.question.as_ref().unwrap();
        assert!(question.for_owner && question.kind == crate::task::QuestionKind::PrCheck);
        assert!(question.text.contains("case_17"));
        assert_eq!(saved.delivery_checks.len(), 1);
    }

    #[test]
    fn a_changed_contract_retains_independent_helper_cost_once_without_publishing_the_question() {
        let (app, _dir) = test_app();
        let mut local = task_with_status(TaskStatus::Running);
        local
            .fixture_data()
            .attempts
            .push(attempt_with_failure(1, "old"));
        let stored = local.clone();
        let mut next = local.clone();
        next.charge(0.4);
        next.record_delivery_checks(vec![VerifyOutcome {
            command: "actual-check".into(),
            code: Some(1),
            tail: "actual-output".into(),
            ..Default::default()
        }]);
        next.ask(question(&VerifyOutcome::default(), "same cause"), now_ms());
        let mut amended = stored.clone();
        amended.fixture_data().criteria.push("new contract".into());
        app.store.save_task(&amended).unwrap();
        let result = publish(&app, &mut local, &stored, next, &CancelToken::new()).unwrap();
        assert!(matches!(result, DeliveryGate::Recheck));
        let saved = app.store.load_task(&local.id).unwrap().unwrap();
        assert!((saved.cost_usd - 0.4).abs() < 1e-9);
        assert_eq!(saved.criteria, ["new contract"]);
        assert!(saved.question.is_none());
        assert_eq!(saved.delivery_checks[0].tail, "actual-output");
        assert_eq!(local.cost_usd, saved.cost_usd);
    }

    #[tokio::test]
    async fn an_owner_overturn_during_repair_judgment_keeps_raw_receipts_but_defers_the_result() {
        let (app, _dir) = test_app();
        let mut local = task_with_status(TaskStatus::Running);
        local.record_assumption(crate::task::Assumption {
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
        local
            .fixture_data()
            .attempts
            .push(attempt_with_failure(1, "old"));
        local.fixture_data().attempts[0].summary = Some("paid native receipt".into());
        app.store.save_task(&local).unwrap();
        let stored = local.clone();
        let mut next = local.clone();
        next.charge(0.4);
        next.record_delivery_checks(vec![VerifyOutcome {
            command: "check".into(),
            code: Some(1),
            tail: "actual receipt".into(),
            ..Default::default()
        }]);
        let id = local.id.clone();
        let worker_app = app.clone();
        let (release, blocked) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            blocked.await.unwrap();
            let result =
                publish(&worker_app, &mut local, &stored, next, &CancelToken::new()).unwrap();
            (result, local)
        });
        app.dispatch(
            "task.overturn",
            serde_json::json!({"id":id,"index":0,"answer":"Keep this check"}),
        )
        .await
        .unwrap();
        release.send(()).unwrap();
        let (result, local) = worker.await.unwrap();
        let saved = app.store.load_task(&id).unwrap().unwrap();
        assert!(
            saved.assumptions[0].overturned,
            "repair publication restored the revoked waiver"
        );
        assert!(matches!(result, DeliveryGate::Recheck));
        assert_eq!(saved.cost_usd, 0.4);
        assert_eq!(saved.delivery_checks.len(), 1);
        assert_eq!(saved.delivery_checks[0].tail, "actual receipt");
        assert_eq!(
            local.attempts[0].summary.as_deref(),
            Some("paid native receipt")
        );
        assert!(saved.question.is_none());
    }
}
