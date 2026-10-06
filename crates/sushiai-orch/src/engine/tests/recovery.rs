#[tokio::test]
async fn recover_on_start_skips_archived_tasks() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.archived = true;
    app.store.save_task(&task).unwrap();

    app.recover_on_start().unwrap();

    assert!(
        !app.controls.lock().unwrap().contains_key(&task.id),
        "an archived task must not get a loop started on recovery"
    );
}
