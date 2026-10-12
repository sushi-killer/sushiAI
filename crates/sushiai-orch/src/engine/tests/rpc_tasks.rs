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

    for status in [TaskStatus::Done, TaskStatus::Failed] {
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
    task.checks = vec![Check {
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
        assert_eq!(
            saved.held_out.as_ref().unwrap().run,
            "secret-held-out-command"
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
    task.criteria = vec!["a".into(), "b".into()];
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
async fn task_amend_depends_on_refuses_cycles_and_invalid_dependencies_without_changes() {
    let (app, _dir) = test_app();
    let mut a = task_with_status(TaskStatus::Stopped);
    let b = task_with_status(TaskStatus::Stopped);
    a.depends_on = vec![b.id.clone()];
    let mut child = task_with_status(TaskStatus::Stopped);
    child.parent = Some(b.id.clone());
    let mut other_repo = task_with_status(TaskStatus::Stopped);
    other_repo.repo = "/other-repo".into();
    for task in [&a, &b, &child, &other_repo] {
        app.store.save_task(task).unwrap();
    }
    let cases = [
        (&b, a.id.clone(), "dependsOn would make a dependency cycle"),
        (&b, b.id.clone(), "dependsOn would make a dependency cycle"),
        (
            &child,
            b.id.clone(),
            "dependsOn would make a dependency cycle",
        ),
        (&b, child.id.clone(), "own child"),
        (&b, other_repo.id.clone(), "another repository"),
        (&b, uuid::Uuid::new_v4().to_string(), "unknown dependency"),
        (&b, "not-an-id".into(), "unknown dependency"),
    ];
    for (task, dependency, expected) in cases {
        let err = app
            .dispatch(
                "task.amend",
                json!({"id": task.id, "dependsOn": [dependency]}),
            )
            .await
            .unwrap_err();
        assert!(err.contains(expected), "{err}");
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.depends_on, task.depends_on);
        assert_eq!(saved.decisions, task.decisions);
    }
}

#[tokio::test]
async fn task_amend_depends_on_refuses_implement_attempts_including_non_archived_children() {
    let (app, _dir) = test_app();
    let mut parent = task_with_status(TaskStatus::Stopped);
    parent.attempts.push(attempt_with_failure(1, "old"));
    app.store.save_task(&parent).unwrap();
    let err = app
        .dispatch("task.amend", json!({"id": parent.id, "dependsOn": []}))
        .await
        .unwrap_err();
    assert!(err.contains("implement attempt"), "{err}");
    assert!(app
        .store
        .load_task(&parent.id)
        .unwrap()
        .unwrap()
        .decisions
        .is_empty());

    parent.attempts.clear();
    app.store.save_task(&parent).unwrap();
    let mut child = task_with_status(TaskStatus::Stopped);
    child.parent = Some(parent.id.clone());
    child.attempts.push(attempt_with_failure(1, "child"));
    app.store.save_task(&child).unwrap();
    let err = app
        .dispatch("task.amend", json!({"id": parent.id, "dependsOn": []}))
        .await
        .unwrap_err();
    assert!(err.contains("implement attempt"), "{err}");
    assert!(app
        .store
        .load_task(&parent.id)
        .unwrap()
        .unwrap()
        .decisions
        .is_empty());

    child.archived = true;
    app.store.save_task(&child).unwrap();
    app.dispatch("task.amend", json!({"id": parent.id, "dependsOn": []}))
        .await
        .unwrap();
    let saved = app.store.load_task(&parent.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert_eq!(
        saved.decisions,
        ["Amended: dependsOn", "Dependencies: waits for nothing"]
    );
}

#[tokio::test]
async fn task_amend_depends_on_replaces_and_deduplicates_on_unstarted_tasks() {
    let (app, _dir) = test_app();
    let mut first = task_with_status(TaskStatus::Running);
    first.title = "First dependency".into();
    let mut second = task_with_status(TaskStatus::Running);
    second.title = "Second dependency".into();
    for dependency in [&first, &second] {
        app.store.save_task(dependency).unwrap();
    }
    for status in [
        TaskStatus::Drafting,
        TaskStatus::Stopped,
        TaskStatus::Waiting,
        TaskStatus::Queued,
    ] {
        let mut task = task_with_status(status);
        task.depends_on = vec![first.id.clone()];
        // A queued backlog task must stay parked after the replacement.
        if status == TaskStatus::Queued {
            task.queue.backlog = Some(Backlog {
                bucket: BacklogBucket::Later,
                order: 0,
            });
        }
        let mut plan = attempt_with_failure(1, "plan");
        plan.stage = Stage::Plan;
        task.attempts.push(plan);
        app.store.save_task(&task).unwrap();
        let result = app
            .dispatch(
                "task.amend",
                json!({"id": task.id, "dependsOn": [second.id, first.id, second.id]}),
            )
            .await
            .unwrap();
        assert_eq!(result["pending"], false);
        assert_eq!(result["amended"], json!(["dependsOn"]));
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.depends_on, [second.id.clone(), first.id.clone()]);
        assert_eq!(saved.status, status);
        assert!(!app.controls.lock().unwrap().contains_key(&task.id));
        assert_eq!(
            saved.decisions,
            [
                "Amended: dependsOn",
                "Dependencies: waits for \"Second dependency\", \"First dependency\""
            ]
        );
    }
}

#[tokio::test]
async fn task_amend_depends_on_merges_pending_replacements_with_their_titles() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Running);
    let dependency = task_with_status(TaskStatus::Running);
    for task in [&task, &dependency] {
        app.store.save_task(task).unwrap();
    }
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
    app.dispatch(
        "task.amend",
        json!({"id": task.id, "dependsOn": [dependency.id]}),
    )
    .await
    .unwrap();
    app.dispatch("task.amend", json!({"id": task.id, "verify": ["true"]}))
        .await
        .unwrap();
    app.dispatch("task.amend", json!({"id": task.id, "dependsOn": []}))
        .await
        .unwrap();
    assert!(app
        .store
        .load_task(&task.id)
        .unwrap()
        .unwrap()
        .decisions
        .is_empty());
    let mut copy = task;
    apply_pending_amendment(&app, &mut copy, &pending, &CancelToken::new()).await;
    assert!(copy.depends_on.is_empty());
    assert_eq!(copy.verify, ["true"]);
    assert_eq!(
        copy.decisions,
        [
            "Amended: verify, dependsOn",
            "Dependencies: waits for nothing"
        ]
    );
}

#[tokio::test]
async fn task_amend_depends_on_refuses_a_cycle_through_another_pending_amendment() {
    let (app, _dir) = test_app();
    let a = task_with_status(TaskStatus::Running);
    let b = task_with_status(TaskStatus::Running);
    for task in [&a, &b] {
        app.store.save_task(task).unwrap();
        app.controls.lock().unwrap().insert(
            task.id.clone(),
            TaskControl {
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: Arc::new(StdMutex::new(None)),
                handle: tokio::spawn(async {}),
            },
        );
    }
    let amended = app
        .dispatch("task.amend", json!({"id": a.id, "dependsOn": [b.id]}))
        .await
        .unwrap();
    assert_eq!(amended["pending"], true);
    let err = app
        .dispatch("task.amend", json!({"id": b.id, "dependsOn": [a.id]}))
        .await
        .unwrap_err();
    assert_eq!(err, "dependsOn would make a dependency cycle");
    assert!(app
        .store
        .load_task(&b.id)
        .unwrap()
        .unwrap()
        .depends_on
        .is_empty());
    assert!(app
        .controls
        .lock()
        .unwrap()
        .get(&b.id)
        .unwrap()
        .pending_amend
        .lock()
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn task_amend_depends_on_during_parent_preflight_starts_no_child() {
    let (app, dir) = test_app();
    app.settings.write().unwrap().answer_policy = false;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let mut parent = task_with_status(TaskStatus::Running);
    parent.repo = repo.to_string_lossy().into();
    parent.worktree = parent.repo.clone();
    parent.base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    parent.verify = vec!["false".into()];
    let mut child = task_with_status(TaskStatus::Queued);
    child.repo = parent.repo.clone();
    child.parent = Some(parent.id.clone());
    let mut dependency = task_with_status(TaskStatus::Running);
    dependency.repo = parent.repo.clone();
    for task in [&parent, &child, &dependency] {
        app.store.save_task(task).unwrap();
    }
    let pending = Arc::new(StdMutex::new(None));
    let answers: Arc<StdMutex<Option<oneshot::Sender<Reply>>>> = Arc::new(StdMutex::new(None));
    let deliver = async {
        loop {
            let tx = answers.lock().unwrap().take();
            if let Some(tx) = tx {
                *pending.lock().unwrap() = Some(Amendment {
                    depends_on: Some(vec![dependency.id.clone()]),
                    dependency_titles: vec![dependency.title.clone()],
                    ..Default::default()
                });
                tx.send(Reply {
                    text: PARENT_KEEP.into(),
                    default_evidence: None,
                })
                .unwrap();
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let cancel = CancelToken::new();
    let (end, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            run_parent(&app, &parent.id, &answers, &pending, &cancel),
            deliver
        )
    })
    .await
    .unwrap();
    assert!(matches!(end, ParentEnd::Finished));
    let saved = app.store.load_task(&parent.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Queued);
    assert_eq!(saved.depends_on, [dependency.id]);
    assert!(!app.controls.lock().unwrap().contains_key(&child.id));
    assert!(app
        .store
        .load_task(&child.id)
        .unwrap()
        .unwrap()
        .attempts
        .is_empty());
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
    assert_eq!(
        saved.criteria,
        vec!["old"],
        "the store is the loop's to write"
    );
    assert!(saved.decisions.is_empty());

    let mut copy = saved;
    apply_pending_amendment(&app, &mut copy, &pending, &CancelToken::new()).await;
    assert_eq!(copy.criteria, vec!["second"]);
    assert_eq!(copy.verify, vec!["v"]);
    assert_eq!(copy.decisions, vec!["Amended: criteria, verify"]);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, vec!["second"]);
}

#[tokio::test]
async fn helpers_folder_task_log_and_timeline_read_helpers_without_a_legacy_fallback() {
    let (app, _dir) = test_app();
    let set_mtime = |path: &std::path::Path, ms: u64| {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + Duration::from_millis(ms))
            .unwrap();
    };
    let mut task = task_with_status(TaskStatus::Done);
    task.attempts = vec![attempt_with_failure(1, "failure")];
    task.attempts[0].ended_at = Some(10_000);
    task.attempts[0].review_cost_usd = Some(0.5);
    task.attempts[0].advisor_cost_usd = Some(0.25);
    app.store.save_task(&task).unwrap();
    for stage in ["review", "advisor"] {
        let legacy = app.store.run_dir(&task.id, 1).join(stage);
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("brief.md"), "legacy prompt").unwrap();
        std::fs::write(legacy.join("events.jsonl"), "legacy").unwrap();
        set_mtime(&legacy.join("brief.md"), 500);
        set_mtime(&legacy.join("events.jsonl"), 5_000);
        let err = app
            .dispatch(
                "task.log",
                json!({"id":task.id, "attempt":1, "stage":stage}),
            )
            .await
            .unwrap_err();
        assert_eq!(err, "no stored log for this stage");
    }
    let timeline = crate::timeline::task_timeline(&app.store, &task, &Default::default(), now_ms());
    for (stage, cost) in [("review", 0.5), ("advisor", 0.25)] {
        let segments: Vec<_> = timeline.iter().filter(|s| s.stage == stage).collect();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].attempt, 1);
        assert_eq!(segments[0].cost_usd, cost);
        assert_eq!(segments[0].started_at, 10_000);
        assert_eq!(segments[0].ended_at, 10_000);
    }
    for stage in ["review", "advisor"] {
        for (i, text) in ["first", "second"].into_iter().enumerate() {
            let dir = app
                .store
                .claim_helper_run_dir(&task.id, stage, Some(1))
                .unwrap();
            std::fs::write(dir.join("brief.md"), "prompt").unwrap();
            std::fs::write(dir.join("events.jsonl"), json!({"type":"assistant", "message":{"content":[{"type":"tool_use", "name":"Bash", "input":{"command":text}}]}}).to_string()).unwrap();
            let start = if stage == "review" { 6_000 } else { 11_000 } + i as u64 * 3_000;
            set_mtime(&dir.join("brief.md"), start);
            set_mtime(&dir.join("events.jsonl"), start + (i as u64 + 1) * 1_000);
        }
        let unrelated = app
            .store
            .claim_helper_run_dir(&task.id, stage, Some(2))
            .unwrap();
        std::fs::write(unrelated.join("brief.md"), "unrelated prompt").unwrap();
        std::fs::write(unrelated.join("events.jsonl"), json!({"type":"assistant", "message":{"content":[{"type":"tool_use", "name":"Bash", "input":{"command":"unrelated"}}]}}).to_string()).unwrap();
        set_mtime(&unrelated.join("brief.md"), 1_000);
        set_mtime(&unrelated.join("events.jsonl"), 6_000);
        let log = app
            .dispatch(
                "task.log",
                json!({"id":task.id, "attempt":1, "stage":stage}),
            )
            .await
            .unwrap();
        let lines = log["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 2, "{log}");
        assert!(lines[0].as_str().unwrap().contains("first"), "{log}");
        assert!(lines[1].as_str().unwrap().contains("second"), "{log}");
    }
    let timeline = crate::timeline::task_timeline(&app.store, &task, &Default::default(), now_ms());
    for (stage, start, cost) in [("review", 7_000, 0.5), ("advisor", 11_000, 0.25)] {
        let segments: Vec<_> = timeline.iter().filter(|s| s.stage == stage).collect();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].attempt, 1);
        assert_eq!(segments[0].cost_usd, cost);
        assert_eq!(segments[0].started_at, start);
        assert_eq!(segments[0].ended_at, start + 3_000);
    }
}
