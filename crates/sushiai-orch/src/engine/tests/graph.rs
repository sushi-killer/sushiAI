fn graph_task(id: &str, status: TaskStatus, depends_on: &[&str], parent: Option<&str>) -> Task {
    let mut t = task_with_status(status);
    t.fixture_data().id = id.to_string();
    t.fixture_data().depends_on = depends_on.iter().map(|d| d.to_string()).collect();
    t.fixture_data().parent = parent.map(str::to_string);
    t
}

#[test]
fn a_parent_waiting_for_a_child_that_depends_on_it_is_a_cycle() {
    let tasks = vec![
        graph_task("p", TaskStatus::Queued, &[], None),
        graph_task("c", TaskStatus::Queued, &["p"], Some("p")),
    ];
    assert!(has_cycle(&wait_edges(&tasks)));
    let tasks = vec![
        graph_task("p", TaskStatus::Queued, &[], None),
        graph_task("a", TaskStatus::Queued, &[], Some("p")),
        graph_task("b", TaskStatus::Queued, &["a"], Some("p")),
    ];
    assert!(!has_cycle(&wait_edges(&tasks)));
}

#[test]
fn waits_state_reads_dependencies_and_children() {
    let mut tasks = vec![
        graph_task("p", TaskStatus::Running, &[], None),
        graph_task("a", TaskStatus::Done, &[], Some("p")),
        graph_task("b", TaskStatus::Queued, &["a", "gone"], Some("p")),
    ];
    assert_eq!(waits_state(&tasks[2], &tasks), Waits::Ready);
    assert_eq!(waits_state(&tasks[0], &tasks), Waits::Pending);
    assert_eq!(waits_state(&tasks[1], &tasks), Waits::Nothing);
    tasks[2].fixture_data().status = TaskStatus::Failed;
    assert_eq!(
        waits_state(&tasks[0], &tasks),
        Waits::Ended(vec!["b".into()])
    );
    // An archived child no longer holds its parent back.
    tasks[2].fixture_data().archived = true;
    assert_eq!(waits_state(&tasks[0], &tasks), Waits::Ready);
}

#[tokio::test]
async fn an_amendment_made_while_a_parent_loop_is_live_is_applied_before_its_check() {
    let (app, dir) = test_app_fitting_sandbox();
    let repo = dir.path().join("repo");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let mut parent = graph_task("p", TaskStatus::Running, &[], None);
    let created =
        crate::git::worktree::create_worktree(&repo, &parent.branch, &worktree, "HEAD").unwrap();
    parent.fixture_data().repo = repo.to_string_lossy().into();
    parent.fixture_data().worktree = worktree.to_string_lossy().into();
    parent.fixture_data().base_sha = created.base_sha;
    parent.fixture_data().base_ref = crate::git::branches::branch_of(&repo, "HEAD");
    parent.fixture_data().verify = vec!["exit 1".into()];
    let mut child = graph_task("c", TaskStatus::Done, &[], Some("p"));
    child.fixture_data().repo = parent.repo.clone();
    app.store.save_task(&parent).unwrap();
    app.store.save_task(&child).unwrap();

    let pending = Arc::new(StdMutex::new(Some(PendingAmendment::capture(
        &parent,
        Amendment {
            verify: Some(vec!["touch amended-ran".into()]),
            ..Default::default()
        },
        Origin::Owner,
    ))));
    let mut queued = app.store.load_task(&parent.id).unwrap().unwrap();
    queued.fixture_data().pending_amendment = pending.lock().unwrap().clone();
    app.store.save_task_with_pending(&queued, None).unwrap();
    let answers = Arc::new(StdMutex::new(None));
    let signals = crate::engine::run_control::TaskSignals {
        resume: Default::default(),
        cancel: CancelToken::new(),
        pending_answer: answers,
        pending_amend: pending.clone(),
    };
    let end = run_parent(&app, "p", &signals).await;

    // The old command would have failed the parent into an agent attempt.
    assert!(matches!(end, ParentEnd::Finished));
    assert!(pending.lock().unwrap().is_none());
    let saved = app.store.load_task("p").unwrap().unwrap();
    assert!(
        crate::git::history::tree_files(&repo, &saved.branch)
            .unwrap()
            .contains(&"amended-ran".to_string()),
        "the amended command ran and its result was committed"
    );
    assert_eq!(saved.verify, vec!["touch amended-ran"]);
    assert!(
        saved.decisions.contains(&"Amended: verify".to_string()),
        "{:?}",
        saved.decisions
    );
    assert_eq!(saved.status, TaskStatus::Done);
}

#[tokio::test]
async fn an_amendment_pending_before_the_base_preflight_replaces_the_checked_command() {
    let (app, dir) = test_app_fitting_sandbox();
    let repo = dir.path().join("repo");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let mut parent = graph_task("p", TaskStatus::Running, &[], None);
    let created =
        crate::git::worktree::create_worktree(&repo, &parent.branch, &worktree, "HEAD").unwrap();
    parent.fixture_data().repo = repo.to_string_lossy().into();
    parent.fixture_data().worktree = worktree.to_string_lossy().into();
    parent.fixture_data().base_sha = created.base_sha;
    parent.fixture_data().base_ref = crate::git::branches::branch_of(&repo, "HEAD");
    parent.fixture_data().verify = vec!["exit 1".into()];
    app.store.save_task(&parent).unwrap();

    let pending = Arc::new(StdMutex::new(Some(PendingAmendment::capture(
        &parent,
        Amendment {
            verify: Some(vec!["touch amended-ran".into()]),
            ..Default::default()
        },
        Origin::Owner,
    ))));
    let mut queued = app.store.load_task(&parent.id).unwrap().unwrap();
    queued.fixture_data().pending_amendment = pending.lock().unwrap().clone();
    app.store.save_task_with_pending(&queued, None).unwrap();
    let answers = Arc::new(StdMutex::new(None));
    let signals = crate::engine::run_control::TaskSignals {
        resume: Default::default(),
        cancel: CancelToken::new(),
        pending_answer: answers,
        pending_amend: pending.clone(),
    };
    let end = run_parent(&app, "p", &signals).await;

    // The old command would have parked the parent on a base-check question.
    assert!(matches!(end, ParentEnd::Finished));
    let saved = app.store.load_task("p").unwrap().unwrap();
    assert!(saved.question.is_none(), "{:?}", saved.question);
    assert_eq!(saved.verify, vec!["touch amended-ran"]);
    assert!(
        saved.decisions.contains(&"Amended: verify".to_string()),
        "{:?}",
        saved.decisions
    );
    assert_eq!(saved.status, TaskStatus::Done);
    assert!(
        crate::git::history::tree_files(&repo, &saved.branch)
            .unwrap()
            .contains(&"amended-ran".to_string()),
        "the amended command ran and its result was committed"
    );
}

#[tokio::test]
async fn advance_graph_starts_no_part_of_a_parent_in_the_backlog() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut parent = graph_task("p", TaskStatus::Queued, &[], None);
    parent.fixture_data().repo = repo.to_string_lossy().into();
    parent.fixture_data().queue.backlog = Some(Backlog {
        bucket: BacklogBucket::Later,
        order: 0,
    });
    let mut child = graph_task("c", TaskStatus::Queued, &[], Some("p"));
    child.fixture_data().repo = parent.repo.clone();
    app.store.save_task(&parent).unwrap();
    app.store.save_task(&child).unwrap();

    app.advance_graph(&parent.repo);
    assert!(!app.controls.lock().unwrap().contains_key("c"));
}

/// A dependency that failed an hour ago with one check that exited `check_code`.
fn failed_dependency(repo: &str, check_code: Option<i32>) -> Task {
    let mut dep = graph_task("dep", TaskStatus::Failed, &[], None);
    dep.fixture_data().repo = repo.into();
    dep.fixture_data().updated_at = now_ms() - 3_600_000;
    let mut attempt = attempt_with_failure(1, "verify:npm test failed");
    attempt.verify = vec![VerifyOutcome {
        command: "npm test".into(),
        code: check_code,
        tail: "no output".into(),
        ms: 1,
        load: 0.0,
        cores: 1,
        load_timeout: false,
        decisions: vec![],
        ..Default::default()
    }];
    dep.fixture_data().attempts = vec![attempt];
    dep
}

#[tokio::test]
async fn a_dependency_that_failed_on_a_timeout_is_restarted_once_by_itself() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo = repo.to_string_lossy().to_string();
    let dep = failed_dependency(&repo, None);
    let mut task = graph_task("t", TaskStatus::Queued, &["dep"], None);
    task.fixture_data().repo = repo.clone();
    app.store.save_task(&dep).unwrap();
    app.store.save_task(&task).unwrap();

    app.ask_dependency_question(&task, &[dep.clone(), task.clone()], &["dep".to_string()]);

    let dependent = app.store.load_task("t").unwrap().unwrap();
    assert!(dependent.question.is_none(), "{:?}", dependent.question);
    assert!(dependent
        .decisions
        .contains(&ladder::DEPENDENCY_RESTART_LINE.to_string()));
    assert_ne!(
        app.store.load_task("dep").unwrap().unwrap().status,
        TaskStatus::Failed,
        "the dependency was started again"
    );

    // Failing on the environment again: no second automatic restart.
    assert!(!app.restart_environment_dependency(&dependent, &[dep], &["dep".to_string()]));
}

#[tokio::test]
async fn a_dependency_that_failed_for_real_still_asks_the_owner() {
    let (app, dir) = test_app();
    app.settings.write().unwrap().answer_policy = false;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let repo = repo.to_string_lossy().to_string();
    let dep = failed_dependency(&repo, Some(1));
    let mut task = graph_task("t", TaskStatus::Queued, &["dep"], None);
    task.fixture_data().repo = repo.clone();
    app.store.save_task(&dep).unwrap();
    app.store.save_task(&task).unwrap();

    app.ask_dependency_question(&task, &[dep, task.clone()], &["dep".to_string()]);

    let dependent = app.store.load_task("t").unwrap().unwrap();
    assert_eq!(dependent.status, TaskStatus::Waiting);
    assert_eq!(
        dependent.question.clone().map(|q| q.kind),
        Some(QuestionKind::DependencyEnded)
    );
}
