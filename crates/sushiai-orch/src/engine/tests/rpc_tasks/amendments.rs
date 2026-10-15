#[tokio::test]
async fn task_amend_on_a_failed_task_is_saved_for_the_next_start() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Failed);
    task.fixture_data().criteria = vec!["old".into()];
    app.store.save_task(&task).unwrap();
    let reply = app
        .dispatch(
            "task.amend",
            json!({"id": task.id, "criteria": ["new"], "verify": ["true"]}),
        )
        .await
        .unwrap();
    assert_eq!(reply["pending"], json!(false));
    let stored = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(stored.criteria, vec!["new"]);
    assert_eq!(stored.verify, vec!["true"]);
    assert!(stored.decisions.iter().any(|d| d.starts_with("Amended:")));
    assert_eq!(stored.status, TaskStatus::Failed);
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

    let done = task_with_status(TaskStatus::Done);
    app.store.save_task(&done).unwrap();
    let err = app
        .dispatch("task.amend", json!({"id": done.id, "criteria": ["a"]}))
        .await
        .unwrap_err();
    assert!(err.contains("can be amended"), "{err}");
    let mut archived = task_with_status(TaskStatus::Stopped);
    archived.fixture_data().archived = true;
    app.store.save_task(&archived).unwrap();
    let err = app
        .dispatch("task.amend", json!({"id": archived.id, "criteria": ["a"]}))
        .await
        .unwrap_err();
    assert!(err.contains("archived"), "{err}");

    let mut task = task_with_status(TaskStatus::Stopped);
    task.fixture_data().criteria = vec!["one".into(), "two".into()];
    app.store.save_task(&task).unwrap();
    let cases = [
        (json!({"id": task.id}), "at least one"),
        (json!({"id": task.id, "criteria": "a"}), "criteria must be"),
        (json!({"id": task.id, "verify": [1]}), "verify must be"),
        (
            json!({"id": task.id, "finalVerify": {}}),
            "finalVerify must be",
        ),
        (
            json!({"id": task.id, "dependsOn": [1]}),
            "dependsOn must be",
        ),
        (
            json!({"id": task.id, "checks": [{"run": "x"}]}),
            "checks must be",
        ),
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
    task.fixture_data().checks = vec![Check {
        criterion: 1,
        run: "x".into(),
        baseline: None,
    }];
    app.store.save_task(&task).unwrap();
    for (params, expected) in cases {
        let err = app
            .dispatch("task.amend", params.clone())
            .await
            .unwrap_err();
        assert!(err.contains(expected), "{params}: {err}");
    }
    let unchanged = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(unchanged.criteria, vec!["one", "two"]);
    assert!(unchanged.decisions.is_empty());
}

#[tokio::test]
async fn task_amend_saves_directly_without_a_loop_and_names_only_fields() {
    let (app, _dir) = test_app();
    for status in [
        TaskStatus::Drafting,
        TaskStatus::Queued,
        TaskStatus::Stopped,
        TaskStatus::Waiting,
    ] {
        let mut task = task_with_status(status);
        task.fixture_data().criteria = vec!["one".into()];
        app.store.save_task(&task).unwrap();
        let result = app
            .dispatch(
                "task.amend",
                json!({
                    "id": task.id,
                    "criteria": ["new criterion"],
                    "verify": ["secret-verify-command"],
                    "finalVerify": ["secret-final-command"],
                    "heldOut": {"criterion": 0, "run": "true # secret-held-out-command"},
                }),
            )
            .await
            .unwrap();
        assert_eq!(result["pending"], false);
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.criteria, vec!["new criterion"]);
        assert_eq!(saved.verify, vec!["secret-verify-command"]);
        assert_eq!(saved.final_verify, vec!["secret-final-command"]);
        assert_eq!(
            saved.held_out.as_ref().unwrap().run,
            "true # secret-held-out-command"
        );
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
async fn task_amend_keeps_grounded_baselines_when_saved_under_the_task_lock() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Stopped);
    task.fixture_data().criteria = vec!["a".into(), "b".into()];
    app.store.save_task(&task).unwrap();
    for (command, code) in [
        ("test -f NOPE.txt", 1),
        ("test -f README.md", 0),
        ("test -f NOPE2.txt", 1),
    ] {
        app.base_runs.lock().unwrap().insert(
            (task.base_sha.clone(), command.into()),
            VerifyOutcome {
                command: command.into(),
                code: Some(code),
                ..Default::default()
            },
        );
    }
    let result = app
        .dispatch(
            "task.amend",
            json!({
                "id": task.id,
                "checks": [
                    {"criterion": 0, "run": "test -f NOPE.txt"},
                    {"criterion": 1, "run": "test -f README.md"},
                ],
                "heldOut": {"criterion": 0, "run": "test -f NOPE2.txt"},
            }),
        )
        .await
        .unwrap();
    assert_eq!(result["pending"], false);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.checks[0].baseline, Some(Baseline::Fail));
    assert_eq!(
        saved.checks[1].baseline,
        Some(Baseline::Pass),
        "{:?}",
        app.base_runs.lock().unwrap()
    );
    assert_eq!(
        saved.held_out.as_ref().unwrap().baseline,
        Some(Baseline::Fail)
    );
    assert_eq!(gated_checks(&saved).len(), 1);
    assert!(saved.decisions.contains(&"Amended: checks, heldOut".into()));
    assert!(saved
        .decisions
        .iter()
        .any(|d| d.contains("already passes on base")));
    assert!(saved.decisions.iter().all(|d| !d.contains("NOPE2")));
}

#[tokio::test]
async fn task_amend_hands_a_live_loops_task_to_the_loop_instead_of_saving() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().criteria = vec!["old".into()];
    app.store.save_task(&task).unwrap();
    let pending = Arc::new(StdMutex::new(None));
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals: crate::engine::run_control::TaskSignals {
                resume: Default::default(),
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: pending.clone(),
            },
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
    assert_eq!(
        saved.criteria,
        vec!["old"],
        "the store is the loop's to write"
    );
    assert!(saved.decisions.is_empty());

    let mut copy = saved;
    apply_pending_amendment(&app, &mut copy, &pending, &CancelToken::new())
        .await
        .unwrap();
    assert_eq!(copy.criteria, vec!["second"]);
    assert_eq!(copy.verify, vec!["v"]);
    assert_eq!(copy.decisions, vec!["Amended: criteria, verify"]);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, vec!["second"]);
}
