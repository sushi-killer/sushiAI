fn graph_task(id: &str, status: TaskStatus, depends_on: &[&str], parent: Option<&str>) -> Task {
    let mut t = task_with_status(status);
    t.id = id.to_string();
    t.depends_on = depends_on.iter().map(|d| d.to_string()).collect();
    t.parent = parent.map(str::to_string);
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
    tasks[2].status = TaskStatus::Failed;
    assert_eq!(
        waits_state(&tasks[0], &tasks),
        Waits::Ended(vec!["b".into()])
    );
    // An archived child no longer holds its parent back.
    tasks[2].archived = true;
    assert_eq!(waits_state(&tasks[0], &tasks), Waits::Ready);
}

#[tokio::test]
async fn an_amendment_made_while_a_parent_loop_is_live_is_applied_before_its_check() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    let mut parent = graph_task("p", TaskStatus::Running, &[], None);
    parent.repo = repo.to_string_lossy().into();
    parent.worktree = worktree.to_string_lossy().into();
    parent.verify = vec!["exit 1".into()];
    let mut child = graph_task("c", TaskStatus::Done, &[], Some("p"));
    child.repo = parent.repo.clone();
    app.store.save_task(&parent).unwrap();
    app.store.save_task(&child).unwrap();

    let pending = Arc::new(StdMutex::new(Some(Amendment {
        verify: Some(vec!["touch amended-ran".into()]),
        ..Default::default()
    })));
    let answers = Arc::new(StdMutex::new(None));
    let end = run_parent(&app, "p", &answers, &pending, &CancelToken::new()).await;

    // The old command would have failed the parent into an agent attempt.
    assert!(matches!(end, ParentEnd::Finished));
    assert!(pending.lock().unwrap().is_none());
    assert!(worktree.join("amended-ran").exists(), "the amended command ran");
    let saved = app.store.load_task("p").unwrap().unwrap();
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
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&worktree).unwrap();
    let mut parent = graph_task("p", TaskStatus::Running, &[], None);
    parent.repo = repo.to_string_lossy().into();
    parent.worktree = worktree.to_string_lossy().into();
    parent.verify = vec!["exit 1".into()];
    app.store.save_task(&parent).unwrap();

    let pending = Arc::new(StdMutex::new(Some(Amendment {
        verify: Some(vec!["touch amended-ran".into()]),
        ..Default::default()
    })));
    let answers = Arc::new(StdMutex::new(None));
    let end = run_parent(&app, "p", &answers, &pending, &CancelToken::new()).await;

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
}

#[tokio::test]
async fn advance_graph_starts_no_part_of_a_parent_in_the_backlog() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut parent = graph_task("p", TaskStatus::Queued, &[], None);
    parent.repo = repo.to_string_lossy().into();
    parent.queue.backlog = Some(Backlog {
        bucket: BacklogBucket::Later,
        order: 0,
    });
    let mut child = graph_task("c", TaskStatus::Queued, &[], Some("p"));
    child.repo = parent.repo.clone();
    app.store.save_task(&parent).unwrap();
    app.store.save_task(&child).unwrap();

    app.advance_graph(&parent.repo);
    assert!(!app.controls.lock().unwrap().contains_key("c"));
}
