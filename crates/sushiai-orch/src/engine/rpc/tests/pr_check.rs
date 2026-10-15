use crate::engine::decision::failure;
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::test_support::{attempt_with_failure, task_with_status, test_app};
use crate::task::{AskedBy, FailureKind, Question, QuestionKind, Task, TaskStatus};
use serde_json::json;

fn waiting(app: &crate::engine::state::App) -> Task {
    let mut task = task_with_status(TaskStatus::Waiting);
    task.schedule_pr_check_repair(
        "exact-cause".into(),
        failure(FailureKind::Verify, "named-case output".into()),
    );
    let step = task.queue.pr_check_repair.as_ref().unwrap().step;
    task.fixture_data()
        .attempts
        .push(attempt_with_failure(step, "same"));
    task.record_pr_pending(true);
    task.record_pr_plan(crate::task::PrPlan {
        branch: "feature/owner".into(),
        title: "owner title".into(),
        body: "owner body".into(),
        branch_from: crate::task::BranchFrom::Owner,
    });
    let mut question = Question::new(
        "same named-case failed after repair",
        vec!["retry".into(), "stop".into()],
        QuestionKind::PrCheck,
        AskedBy::Verify,
        1,
    );
    question.for_owner = true;
    task.ask(question, 1);
    app.store.save_task(&task).unwrap();
    task
}

#[tokio::test]
async fn owner_retry_grants_exactly_one_new_step_and_preserves_the_pr_plan() {
    let (app, _dir) = test_app();
    let before = waiting(&app);
    let old_step = before.queue.pr_check_repair.as_ref().unwrap().step;
    // The RPC admission is under test; this inert owned control prevents a native run.
    app.controls.lock().unwrap().insert(
        before.id.clone(),
        TaskControl {
            signals: TaskSignals::new(),
            handle: tokio::spawn(std::future::pending()),
        },
    );
    app.handle_task_answer(json!({"id":before.id,"answer":"retry"}))
        .await
        .unwrap();
    let saved = app.store.load_task(&before.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Queued);
    assert!(saved.question.is_none() && saved.pr_pending && saved.pr_url.is_none());
    let repair = saved.queue.pr_check_repair.as_ref().unwrap();
    assert_eq!(repair.step, old_step + 1);
    assert_eq!(repair.cause, "exact-cause");
    assert!(repair.failure.detail.contains("named-case output"));
    assert_eq!(saved.pr_plan.as_ref().unwrap().branch, "feature/owner");
    assert!(saved.pr_check_repair_due());
    assert!(app
        .handle_task_answer(json!({"id":before.id,"answer":"retry"}))
        .await
        .is_err());
    let control = app.controls.lock().unwrap().remove(&before.id).unwrap();
    control.handle.abort();
}

#[tokio::test]
async fn only_the_owner_may_retry_but_the_existing_stop_refusal_is_preserved() {
    let (app, _dir) = test_app();
    for source in ["lead", "chat", "mcp", "overseer"] {
        let before = waiting(&app);
        assert!(app
            .handle_task_answer(json!({"id":before.id,"answer":"retry","source":source}))
            .await
            .is_err());
        let saved = app.store.load_task(&before.id).unwrap().unwrap();
        assert_eq!(saved.status, TaskStatus::Waiting);
        assert_eq!(
            saved.queue.pr_check_repair.as_ref().unwrap().step,
            before.queue.pr_check_repair.as_ref().unwrap().step
        );
    }
    for source in [None, Some("overseer")] {
        let before = waiting(&app);
        let mut params = json!({"id":before.id,"answer":"stop"});
        if let Some(source) = source {
            params["source"] = json!(source);
        }
        app.handle_task_answer(params).await.unwrap();
        let saved = app.store.load_task(&before.id).unwrap().unwrap();
        assert_eq!(saved.status, TaskStatus::Stopped);
        assert!(saved.pr_url.is_none() && saved.question.is_none());
    }
}
