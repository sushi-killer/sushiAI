#[cfg(test)]
use crate::engine::test_support::task_with_status;
#[cfg(test)]
use crate::engine::test_support::test_app;
use crate::task::AnswerSource;
use crate::task::TaskStatus;
use crate::task::Wait;
use serde_json::json;

#[test]
fn a_stop_line_names_its_source_and_reason() {
    assert_eq!(
        AnswerSource::Owner.control_line("stop", None),
        "Owner: stop (UI)"
    );
    assert_eq!(
        AnswerSource::Owner.control_line("stop", Some(" wrong repo ")),
        "Owner: stop (UI): wrong repo"
    );
}

#[tokio::test]
async fn stopping_an_idle_queued_task_records_who_and_why() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().queue.wait = Some(Wait::Dependency {
        titles: vec!["another".into()],
    });
    app.store.save_task(&task).unwrap();
    let out = app
        .handle_task_stop(json!({"id": task.id, "reason": "not needed"}))
        .await
        .unwrap();
    assert_eq!(out["status"], "stopped");
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.decisions, ["Owner: stop (UI): not needed"]);
    // Stopping it again adds nothing.
    app.handle_task_stop(json!({"id": task.id})).await.unwrap();
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.decisions.len(), 1);
}
