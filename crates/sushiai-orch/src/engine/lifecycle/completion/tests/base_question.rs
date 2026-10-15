use super::*;
use crate::engine::run_control::TaskSignals;
use crate::engine::test_support::{attempt_with_failure, task_with_status, test_app};
use crate::engine::verification::base::answers::KEEP_ALL;
use crate::task::VerifyOutcome;

#[tokio::test]
async fn a_changed_final_check_question_returns_to_the_loop_without_another_attempt() {
    let (app, _dir) = test_app();
    app.settings.write().unwrap().answer_policy = true;
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().criteria = vec!["Original contract".into()];
    task.fixture_data().final_verify = vec!["false".into()];
    task.fixture_data().attempts = vec![attempt_with_failure(1, "verify:failed")];
    task.fixture_data().cost_usd = 0.25;
    app.store.save_task(&task).unwrap();
    let files = AttemptFiles::new(
        0,
        1,
        task.worktree.clone().into(),
        app.store.run_dir(&task.id, 1),
    );
    let failure = VerifyOutcome {
        command: "false".into(),
        code: Some(1),
        tail: "existing failure".into(),
        ..Default::default()
    };
    let mut question = FinalQuestion::new(
        &files,
        &task,
        FinalFailures {
            results: vec![failure.clone()],
            ordinary: Vec::new(),
            pre_existing: vec![failure],
        },
    );
    question.question.options.push(KEEP_ALL.into());
    let mut fresh = app.store.load_task(&task.id).unwrap().unwrap();
    fresh.fixture_data().criteria = vec!["Owner's new contract".into()];
    app.store.save_task(&fresh).unwrap();
    let signals = TaskSignals::new();
    let mut permit = None;
    let mut budget = 1;
    let result = question
        .settle(
            &app,
            &mut task,
            &mut budget,
            &mut QuestionSlot::new(&signals, &mut permit),
        )
        .await;
    assert!(matches!(result, Some(Tail::Continue)));
    assert_eq!(budget, 1);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, fresh.criteria);
    assert_eq!(saved.status, TaskStatus::Running);
    assert_eq!(saved.cost_usd, 0.25);
    assert_eq!(saved.attempts.len(), 1);
    assert!(saved.assumptions.is_empty());
    assert!(saved.question_history.is_empty());
}
