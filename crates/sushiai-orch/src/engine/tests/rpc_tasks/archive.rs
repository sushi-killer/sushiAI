#[tokio::test]
async fn task_archive_refuses_running_drafting_waiting_and_landing_tasks() {
    let (app, _dir) = test_app();
    for status in [
        TaskStatus::Running,
        TaskStatus::Drafting,
        TaskStatus::Waiting,
        TaskStatus::Landing,
    ] {
        let task = task_with_status(status);
        app.store.save_task(&task).unwrap();
        let err = app
            .dispatch("task.archive", json!({"id": task.id}))
            .await
            .unwrap_err();
        assert!(
            err.contains("running, drafting, waiting or landing"),
            "status {status:?}: unexpected error {err}"
        );
        let reloaded = app.store.load_task(&task.id).unwrap().unwrap();
        assert!(!reloaded.archived, "status {status:?} must not be archived");
    }
}

#[tokio::test]
async fn task_archive_succeeds_on_queued_stopped_failed_and_done_tasks() {
    let (app, _dir) = test_app();
    for status in [
        TaskStatus::Queued,
        TaskStatus::Stopped,
        TaskStatus::Failed,
        TaskStatus::Done,
    ] {
        let task = task_with_status(status);
        app.store.save_task(&task).unwrap();
        let result = app
            .dispatch("task.archive", json!({"id": task.id}))
            .await
            .unwrap();
        assert_eq!(result["archived"], true, "status {status:?}");
        let reloaded = app.store.load_task(&task.id).unwrap().unwrap();
        assert!(
            reloaded.archived,
            "status {status:?} should now be archived"
        );
    }
}

#[tokio::test]
async fn task_list_hides_archived_tasks_unless_include_archived_is_set() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Done);
    app.store.save_task(&task).unwrap();
    app.dispatch("task.archive", json!({"id": task.id}))
        .await
        .unwrap();

    let default_list = app.dispatch("task.list", json!({})).await.unwrap();
    assert!(default_list.as_array().unwrap().is_empty());

    let with_archived = app
        .dispatch("task.list", json!({"includeArchived": true}))
        .await
        .unwrap();
    let listed = with_archived.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], task.id);
    assert_eq!(listed[0]["archived"], true);
}

#[tokio::test]
async fn task_unarchive_makes_a_task_reappear_in_the_default_task_list() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Done);
    app.store.save_task(&task).unwrap();
    app.dispatch("task.archive", json!({"id": task.id}))
        .await
        .unwrap();
    assert!(app
        .dispatch("task.list", json!({}))
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());

    let result = app
        .dispatch("task.unarchive", json!({"id": task.id}))
        .await
        .unwrap();
    assert_eq!(result["archived"], false);

    let default_list = app.dispatch("task.list", json!({})).await.unwrap();
    let listed = default_list.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], task.id);
}

#[tokio::test]
async fn task_archive_refuses_a_queued_task_with_a_live_loop() {
    let (app, _dir) = test_app_parallel_one();

    // A second task holds the app's only slot, so the task under test
    // stays parked at the top of `run_task_loop` (permit acquired
    // before anything else) instead of ever reaching the harness.
    let holder = task_with_status(TaskStatus::Running);
    app.store.save_task(&holder).unwrap();
    let held_permit = app.slots.clone().try_acquire_owned().unwrap();

    let task = task_with_status(TaskStatus::Queued);
    app.store.save_task(&task).unwrap();
    app.spawn_task_loop(task.id.clone(), true);
    assert!(
        app.controls.lock().unwrap().contains_key(&task.id),
        "spawn_task_loop registers its control entry synchronously"
    );

    let err = app
        .dispatch("task.archive", json!({"id": task.id}))
        .await
        .unwrap_err();
    assert!(err.contains("live loop"), "unexpected error: {err}");
    let reloaded = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(!reloaded.archived);

    drop(held_permit);
}

#[tokio::test]
async fn task_start_refuses_an_archived_task() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Stopped);
    task.fixture_data().archived = true;
    app.store.save_task(&task).unwrap();

    let err = app
        .dispatch("task.start", json!({"id": task.id}))
        .await
        .unwrap_err();
    assert!(
        err.contains("archived") && err.contains("unarchive"),
        "unexpected error: {err}"
    );
    assert!(!app.controls.lock().unwrap().contains_key(&task.id));
}

#[tokio::test]
async fn task_delete_via_mcp_refuses_a_live_task_until_task_stop() {
    let (app, _dir) = test_app();
    for status in [
        TaskStatus::Running,
        TaskStatus::Drafting,
        TaskStatus::Waiting,
        TaskStatus::Landing,
    ] {
        let task = task_with_status(status);
        app.store.save_task(&task).unwrap();
        let err = app
            .dispatch("task.delete", json!({"id": task.id, "source": "chat"}))
            .await
            .unwrap_err();
        assert!(err.contains("task_stop"), "status {status:?}: {err}");
        assert!(
            app.store.load_task(&task.id).unwrap().is_some(),
            "status {status:?} must keep its record"
        );
        app.dispatch("task.stop", json!({"id": task.id, "source": "chat"}))
            .await
            .unwrap();
        app.dispatch("task.delete", json!({"id": task.id, "source": "chat"}))
            .await
            .unwrap();
        assert!(
            app.store.load_task(&task.id).unwrap().is_none(),
            "status {status:?} must be deleted after task_stop"
        );
    }
}

#[tokio::test]
async fn task_delete_via_mcp_refuses_a_task_with_a_live_loop_but_the_owner_may_delete_it() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Queued);
    app.store.save_task(&task).unwrap();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                resume: Default::default(),
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: Arc::new(StdMutex::new(None)),
            },
            handle: tokio::spawn(async {}),
        },
    );
    let err = app
        .dispatch("task.delete", json!({"id": task.id, "source": "lead"}))
        .await
        .unwrap_err();
    assert!(
        err.contains("live loop") && err.contains("task_stop"),
        "{err}"
    );
    assert!(app.store.load_task(&task.id).unwrap().is_some());
    app.dispatch("task.delete", json!({"id": task.id}))
        .await
        .unwrap();
    assert!(app.store.load_task(&task.id).unwrap().is_none());
}

#[tokio::test]
async fn task_archive_error_names_task_stop() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let err = app
        .dispatch("task.archive", json!({"id": task.id}))
        .await
        .unwrap_err();
    assert!(
        err.contains("running, drafting, waiting or landing"),
        "{err}"
    );
    assert!(err.contains("task_stop"), "{err}");

    let queued = task_with_status(TaskStatus::Queued);
    app.store.save_task(&queued).unwrap();
    app.controls.lock().unwrap().insert(
        queued.id.clone(),
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                resume: Default::default(),
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: Arc::new(StdMutex::new(None)),
            },
            handle: tokio::spawn(async {}),
        },
    );
    let err = app
        .dispatch("task.archive", json!({"id": queued.id}))
        .await
        .unwrap_err();
    assert!(
        err.contains("live loop") && err.contains("task_stop"),
        "{err}"
    );
}
