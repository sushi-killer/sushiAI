#[tokio::test]
async fn task_archive_refuses_running_drafting_and_waiting_tasks() {
    let (app, _dir) = test_app();
    for status in [
        TaskStatus::Running,
        TaskStatus::Drafting,
        TaskStatus::Waiting,
    ] {
        let task = task_with_status(status);
        app.store.save_task(&task).unwrap();
        let err = app
            .dispatch("task.archive", json!({"id": task.id}))
            .await
            .unwrap_err();
        assert!(
            err.contains("running, drafting, or waiting"),
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
    task.archived = true;
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
async fn task_amend_rejects_what_it_cannot_amend() {
    let (app, _dir) = test_app();
    let unknown = app
        .dispatch(
            "task.amend",
            json!({"id": uuid::Uuid::new_v4().to_string(), "criteria": ["a"]}),
        )
        .await
        .unwrap_err();
    assert!(unknown.contains("not found"), "{unknown}");

    for status in [TaskStatus::Done, TaskStatus::Failed, TaskStatus::Drafting] {
        let task = task_with_status(status);
        app.store.save_task(&task).unwrap();
        let err = app
            .dispatch("task.amend", json!({"id": task.id, "criteria": ["a"]}))
            .await
            .unwrap_err();
        assert!(err.contains("can be amended"), "{status:?}: {err}");
    }
    let mut archived = task_with_status(TaskStatus::Stopped);
    archived.archived = true;
    app.store.save_task(&archived).unwrap();
    let err = app
        .dispatch("task.amend", json!({"id": archived.id, "criteria": ["a"]}))
        .await
        .unwrap_err();
    assert!(err.contains("archived"), "{err}");

    let mut task = task_with_status(TaskStatus::Stopped);
    task.criteria = vec!["one".into(), "two".into()];
    app.store.save_task(&task).unwrap();
    let cases = [
        (json!({"id": task.id}), "at least one"),
        (json!({"id": task.id, "criteria": "a"}), "criteria must be"),
        (json!({"id": task.id, "verify": [1]}), "verify must be"),
        (json!({"id": task.id, "finalVerify": {}}), "finalVerify must be"),
        (json!({"id": task.id, "checks": [{"run": "x"}]}), "checks must be"),
        (json!({"id": task.id, "heldOut": "x"}), "heldOut must be"),
        (
            json!({"id": task.id, "checks": [{"criterion": 2, "run": "x"}]}),
            "out of range",
        ),
        (
            json!({"id": task.id, "heldOut": {"criterion": 5, "run": "x"}}),
            "out of range",
        ),
        // Shrinking the criteria under an existing check.
        (json!({"id": task.id, "criteria": ["only"]}), "out of range"),
    ];
    task.checks = vec![Check {
        criterion: 1,
        run: "x".into(),
        baseline: None,
    }];
    app.store.save_task(&task).unwrap();
    for (params, expected) in cases {
        let err = app.dispatch("task.amend", params.clone()).await.unwrap_err();
        assert!(err.contains(expected), "{params}: {err}");
    }
    let unchanged = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(unchanged.criteria, vec!["one", "two"]);
    assert!(unchanged.decisions.is_empty());
}

#[tokio::test]
async fn task_amend_saves_directly_without_a_loop_and_names_only_fields() {
    let (app, _dir) = test_app();
    for status in [TaskStatus::Queued, TaskStatus::Stopped, TaskStatus::Waiting] {
        let mut task = task_with_status(status);
        task.criteria = vec!["one".into()];
        app.store.save_task(&task).unwrap();
        let result = app
            .dispatch(
                "task.amend",
                json!({
                    "id": task.id,
                    "criteria": ["new criterion"],
                    "verify": ["secret-verify-command"],
                    "finalVerify": ["secret-final-command"],
                    "heldOut": {"criterion": 0, "run": "secret-held-out-command"},
                }),
            )
            .await
            .unwrap();
        assert_eq!(result["pending"], false);
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.criteria, vec!["new criterion"]);
        assert_eq!(saved.verify, vec!["secret-verify-command"]);
        assert_eq!(saved.final_verify, vec!["secret-final-command"]);
        assert_eq!(saved.held_out.as_ref().unwrap().run, "secret-held-out-command");
        assert_eq!(
            saved.decisions,
            vec!["Amended: criteria, verify, finalVerify, heldOut"]
        );
        for line in &saved.decisions {
            assert!(!line.contains("secret") && !line.contains("new criterion"));
        }
    }
}

#[tokio::test]
async fn task_amend_hands_a_live_loops_task_to_the_loop_instead_of_saving() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    task.criteria = vec!["old".into()];
    app.store.save_task(&task).unwrap();
    let pending = Arc::new(StdMutex::new(None));
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            cancel: CancelToken::new(),
            pending_answer: Arc::new(StdMutex::new(None)),
            pending_amend: pending.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    for criteria in [json!(["first"]), json!(["second"])] {
        let result = app
            .dispatch("task.amend", json!({"id": task.id, "criteria": criteria}))
            .await
            .unwrap();
        assert_eq!(result["pending"], true);
    }
    app.dispatch("task.amend", json!({"id": task.id, "verify": ["v"]}))
        .await
        .unwrap();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, vec!["old"], "the store is the loop's to write");
    assert!(saved.decisions.is_empty());

    let mut copy = saved;
    apply_pending_amendment(&app, &mut copy, &pending, &CancelToken::new()).await;
    assert_eq!(copy.criteria, vec!["second"]);
    assert_eq!(copy.verify, vec!["v"]);
    assert_eq!(copy.decisions, vec!["Amended: criteria, verify"]);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, vec!["second"]);
}
