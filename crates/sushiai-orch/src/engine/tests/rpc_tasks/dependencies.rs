#[tokio::test]
async fn task_amend_depends_on_refuses_cycles_and_invalid_dependencies_without_changes() {
    let (app, _dir) = test_app();
    let mut a = task_with_status(TaskStatus::Stopped);
    let b = task_with_status(TaskStatus::Stopped);
    a.fixture_data().depends_on = vec![b.id.clone()];
    let mut child = task_with_status(TaskStatus::Stopped);
    child.fixture_data().parent = Some(b.id.clone());
    let mut other_repo = task_with_status(TaskStatus::Stopped);
    other_repo.fixture_data().repo = "/other-repo".into();
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

fn live_control(app: &App, id: &str) {
    app.controls.lock().unwrap().insert(
        id.to_string(),
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
}

#[tokio::test]
async fn task_amend_depends_on_accepts_tasks_that_already_ran_attempts() {
    let (app, _dir) = test_app();
    for status in [TaskStatus::Stopped, TaskStatus::Queued] {
        let mut task = task_with_status(status);
        task.fixture_data()
            .attempts
            .push(attempt_with_failure(1, "old"));
        // A queued backlog task stays parked, so no loop starts.
        if status == TaskStatus::Queued {
            task.fixture_data().queue.backlog = Some(Backlog {
                bucket: BacklogBucket::Later,
                order: 0,
            });
        }
        app.store.save_task(&task).unwrap();
        app.dispatch("task.amend", json!({"id": task.id, "dependsOn": []}))
            .await
            .unwrap();
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.status, status);
        assert_eq!(
            saved.decisions,
            ["Amended: dependsOn", "Dependencies: waits for nothing"]
        );
    }
}

#[tokio::test]
async fn task_amend_depends_on_refuses_a_task_or_child_that_is_implementing() {
    let (app, _dir) = test_app();
    let mut parent = task_with_status(TaskStatus::Running);
    parent
        .fixture_data()
        .attempts
        .push(attempt_with_failure(1, "old"));
    app.store.save_task(&parent).unwrap();
    live_control(&app, &parent.id);
    let err = app
        .dispatch("task.amend", json!({"id": parent.id, "dependsOn": []}))
        .await
        .unwrap_err();
    assert!(err.contains("implementing"), "{err}");
    let saved = app.store.load_task(&parent.id).unwrap().unwrap();
    assert!(saved.decisions.is_empty());
    assert!(app.controls.lock().unwrap()[&parent.id]
        .signals
        .pending_amend
        .lock()
        .unwrap()
        .is_none());

    let parent = task_with_status(TaskStatus::Stopped);
    app.store.save_task(&parent).unwrap();
    let mut child = task_with_status(TaskStatus::Running);
    child.fixture_data().parent = Some(parent.id.clone());
    app.store.save_task(&child).unwrap();
    live_control(&app, &child.id);
    let err = app
        .dispatch("task.amend", json!({"id": parent.id, "dependsOn": []}))
        .await
        .unwrap_err();
    assert!(err.contains("implementing"), "{err}");
    assert!(app
        .store
        .load_task(&parent.id)
        .unwrap()
        .unwrap()
        .decisions
        .is_empty());
}

#[tokio::test]
async fn task_amend_depends_on_failed_task_still_refuses_cycles() {
    let (app, _dir) = test_app();
    let mut failed = task_with_status(TaskStatus::Failed);
    failed
        .fixture_data()
        .attempts
        .push(attempt_with_failure(1, "old"));
    let mut other = task_with_status(TaskStatus::Stopped);
    other.fixture_data().depends_on = vec![failed.id.clone()];
    app.store.save_task(&failed).unwrap();
    app.store.save_task(&other).unwrap();
    let err = app
        .dispatch(
            "task.amend",
            json!({"id": failed.id, "dependsOn": [other.id]}),
        )
        .await
        .unwrap_err();
    assert!(err.contains("dependency cycle"), "{err}");
    let saved = app.store.load_task(&failed.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Failed);
    assert!(saved.depends_on.is_empty());
    assert!(saved.decisions.is_empty());
}

#[tokio::test]
async fn task_amend_depends_on_empty_list_does_not_requeue_a_failed_task() {
    let (app, _dir) = test_app();
    let mut failed = task_with_status(TaskStatus::Failed);
    failed
        .fixture_data()
        .attempts
        .push(attempt_with_failure(1, "old"));
    app.store.save_task(&failed).unwrap();
    app.dispatch("task.amend", json!({"id": failed.id, "dependsOn": []}))
        .await
        .unwrap();
    let saved = app.store.load_task(&failed.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Failed);
}

#[tokio::test]
async fn task_amend_depends_on_requeues_a_failed_task_behind_its_dependency() {
    let (app, _dir) = test_app();
    let mut failed = task_with_status(TaskStatus::Failed);
    failed
        .fixture_data()
        .attempts
        .push(attempt_with_failure(1, "old"));
    let dependency = task_with_status(TaskStatus::Running);
    app.store.save_task(&failed).unwrap();
    app.store.save_task(&dependency).unwrap();
    app.dispatch(
        "task.amend",
        json!({"id": failed.id, "dependsOn": [dependency.id]}),
    )
    .await
    .unwrap();
    let saved = app.store.load_task(&failed.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Queued);
    assert_eq!(saved.depends_on, std::slice::from_ref(&dependency.id));
}

#[tokio::test]
async fn task_amend_depends_on_replaces_and_deduplicates_on_unstarted_tasks() {
    let (app, _dir) = test_app();
    let mut first = task_with_status(TaskStatus::Running);
    first.fixture_data().title = "First dependency".into();
    let mut second = task_with_status(TaskStatus::Running);
    second.fixture_data().title = "Second dependency".into();
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
        task.fixture_data().depends_on = vec![first.id.clone()];
        // A queued backlog task must stay parked after the replacement.
        if status == TaskStatus::Queued {
            task.fixture_data().queue.backlog = Some(Backlog {
                bucket: BacklogBucket::Later,
                order: 0,
            });
        }
        let mut plan = attempt_with_failure(1, "plan");
        plan.stage = Stage::Plan;
        task.fixture_data().attempts.push(plan);
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
            signals: crate::engine::run_control::TaskSignals {
                resume: Default::default(),
                cancel: CancelToken::new(),
                pending_answer: Arc::new(StdMutex::new(None)),
                pending_amend: pending.clone(),
            },
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
    apply_pending_amendment(&app, &mut copy, &pending, &CancelToken::new())
        .await
        .unwrap();
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
                signals: crate::engine::run_control::TaskSignals {
                    resume: Default::default(),
                    cancel: CancelToken::new(),
                    pending_answer: Arc::new(StdMutex::new(None)),
                    pending_amend: Arc::new(StdMutex::new(None)),
                },
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
        .signals
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
    parent.fixture_data().repo = repo.to_string_lossy().into();
    parent.fixture_data().worktree = parent.repo.clone();
    parent.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    parent.fixture_data().verify = vec!["false".into()];
    let mut child = task_with_status(TaskStatus::Queued);
    child.fixture_data().repo = parent.repo.clone();
    child.fixture_data().parent = Some(parent.id.clone());
    let mut dependency = task_with_status(TaskStatus::Running);
    dependency.fixture_data().repo = parent.repo.clone();
    for task in [&parent, &child, &dependency] {
        app.store.save_task(task).unwrap();
    }
    let pending = Arc::new(StdMutex::new(None));
    let answers: Arc<StdMutex<Option<oneshot::Sender<Reply>>>> = Arc::new(StdMutex::new(None));
    let deliver = async {
        let mut asked = None;
        crate::engine::test_support::wait_async("the question to be asked", || {
            asked = answers.lock().unwrap().take();
            asked.is_some()
        })
        .await;
        let tx = asked.unwrap();
        *pending.lock().unwrap() = Some(PendingAmendment::capture(
            &parent,
            Amendment {
                depends_on: Some(vec![dependency.id.clone()]),
                dependency_titles: vec![dependency.title.clone()],
                ..Default::default()
            },
            Origin::Owner,
        ));
        let mut queued = app.store.load_task(&parent.id).unwrap().unwrap();
        queued.fixture_data().pending_amendment = pending.lock().unwrap().clone();
        app.store.save_task_with_pending(&queued, None).unwrap();
        tx.send(Reply {
            text: KEEP_ALL.into(),
            default_evidence: None,
            like: None,
        })
        .unwrap();
    };
    let cancel = CancelToken::new();
    let signals = crate::engine::run_control::TaskSignals {
        resume: Default::default(),
        cancel: cancel.clone(),
        pending_answer: answers.clone(),
        pending_amend: pending.clone(),
    };
    let (end, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(run_parent(&app, &parent.id, &signals), deliver)
    })
    .await
    .unwrap();
    assert!(matches!(end, ParentEnd::Finished));
    let saved = app.store.load_task(&parent.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Queued);
    assert_eq!(saved.depends_on, [dependency.id.clone()]);
    assert!(!app.controls.lock().unwrap().contains_key(&child.id));
    assert!(app
        .store
        .load_task(&child.id)
        .unwrap()
        .unwrap()
        .attempts
        .is_empty());
}
