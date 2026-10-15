use crate::engine::ladder;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
#[cfg(test)]
use crate::engine::test_support::test_app;
use crate::task::TaskStatus;
use serde_json::json;

#[tokio::test]
async fn an_agent_restarts_a_failed_task_once_and_the_owner_any_time() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Failed);
    task.fixture_data().repo = std::env::temp_dir().display().to_string();
    app.store.save_task(&task).unwrap();
    let start = |source: Option<&str>| {
        let mut params = json!({"id": task.id});
        if let Some(s) = source {
            params["source"] = json!(s);
        }
        params
    };
    app.handle_task_start(start(Some("overseer")))
        .await
        .unwrap();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(saved
        .decisions
        .contains(&ladder::AGENT_RESTART_LINE.to_string()));
    // Failed again: the agent's second restart is refused, the owner's is not.
    let mut failed = saved;
    failed.mark_failed();
    app.store.save_task(&failed).unwrap();
    let err = app
        .handle_task_start(start(Some("overseer")))
        .await
        .unwrap_err();
    assert!(err.contains("only the owner"), "{err}");
    app.handle_task_start(start(None)).await.unwrap();
    app.shutdown();
}
