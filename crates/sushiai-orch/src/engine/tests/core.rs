#[test]
fn cancel_token_is_visible_synchronously_after_cancel() {
    let cancel = CancelToken::new();
    assert!(!cancel.is_cancelled());
    cancel.cancel();
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn cancel_token_wakes_a_waiter_registered_after_cancel() {
    let cancel = CancelToken::new();
    cancel.cancel();
    // Must resolve immediately, not hang -- this is exactly the "check
    // between every step" use case.
    tokio::time::timeout(Duration::from_millis(200), cancel.cancelled())
        .await
        .expect("cancelled() must resolve immediately once already cancelled");
}

#[tokio::test]
async fn cancel_token_children_follow_the_parent_without_cancelling_siblings() {
    let parent = CancelToken::new();
    let child = parent.child();
    let sibling = parent.child();
    let descendant = child.child();
    let waiting = descendant.clone();
    let waiter = tokio::spawn(async move { waiting.cancelled().await });
    tokio::task::yield_now().await;
    child.cancel();
    tokio::time::timeout(Duration::from_millis(200), waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(descendant.is_cancelled());
    assert!(!parent.is_cancelled());
    assert!(!sibling.is_cancelled());
    let waiting = sibling.child();
    let waiter = tokio::spawn(async move { waiting.cancelled().await });
    tokio::task::yield_now().await;
    parent.cancel();
    tokio::time::timeout(Duration::from_millis(200), waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(sibling.is_cancelled());
    tokio::time::timeout(Duration::from_millis(200), parent.child().cancelled())
        .await
        .unwrap();
}

#[test]
fn validate_task_id_rejects_path_traversal_and_non_uuid() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    assert!(validate_task_id(&store, "../../etc/passwd").is_err());
    assert!(validate_task_id(&store, "not-a-uuid").is_err());
    assert!(validate_task_id(&store, "").is_err());
    let real_id = uuid::Uuid::new_v4().to_string();
    assert!(validate_task_id(&store, &real_id).is_ok());
}

#[test]
fn simple_hash_is_stable_and_sensitive_to_content() {
    assert_eq!(simple_hash("abc"), simple_hash("abc"));
    assert_ne!(simple_hash("abc"), simple_hash("abd"));
}

#[tokio::test]
async fn helpers_folder_implement_attempt_keeps_a_unique_number_after_planning() {
    let (app, _dir, mut task, _) = slow_helper_task();
    task.fixture_data().status = TaskStatus::Queued;
    task.fixture_data().attempts[0].stage = Stage::Plan;
    task.fixture_data().attempts[0].status = AttemptStatus::Passed;
    task.fixture_data().attempts[0].failure = None;
    task.fixture_data().brief_check.done = true;
    {
        let mut settings = app.settings.write().unwrap();
        settings.brief_check_route.clear();
        settings.review.clear();
        settings.auto_answer = false;
        settings.answer_policy = false;
    }
    app.store.save_task(&task).unwrap();
    let mut events = app.subscribe();
    app.start_task_loop(task.id.clone());
    let started = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Event::Task { task: fresh } = events.recv().await.unwrap() {
                if fresh.id == task.id && fresh.attempts.len() > 1 {
                    break fresh;
                }
            }
        }
    })
    .await;
    app.handle_task_stop(json!({"id":task.id})).await.unwrap();
    crate::engine::test_support::wait_async("the fixture loop to finish after stopping", || {
        !app.controls.lock().unwrap().contains_key(&task.id)
    })
    .await;
    let started = started.expect("the implement attempt must start");
    assert_eq!(started.attempts[0].n, 1);
    assert_eq!(started.attempts[1].n, 2);
    assert_eq!(started.attempts[1].stage, Stage::Implement);
}

#[tokio::test]
async fn concurrent_task_updates_preserve_every_decision() {
    const N: usize = 12;
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Queued);
    app.store.save_task(&task).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(N));
    let threads: Vec<_> = (0..N)
        .map(|i| {
            let app = app.clone();
            let id = task.id.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                app.update_task(&id, |task| {
                    std::thread::sleep(Duration::from_millis(5)); // wait-allow: widens the read-modify-write window the test races twelve writers through
                    task.fixture_data().decisions.push(i.to_string());
                })
                .unwrap();
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.decisions.len(), N);
    for i in 0..N {
        assert!(task.decisions.contains(&i.to_string()));
    }
    assert!(task.updated_at > task.created_at);
}

#[tokio::test]
async fn concurrent_task_cost_updates_preserve_every_charge() {
    const N: usize = 12;
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Queued);
    app.store.save_task(&task).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(N));
    let threads: Vec<_> = (0..N)
        .map(|_| {
            let app = app.clone();
            let id = task.id.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                add_task_cost(&app, &id, 0.25);
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.cost_usd, task.cost_usd + N as f64 * 0.25);
}

#[tokio::test]
async fn save_task_logged_reports_task_id_on_failure() {
    const CHILD_ENV: &str = "ORCH_TEST_SAVE_TASK_LOGGED_CHILD";
    const ID: &str = "ee7f0a3a-6b8e-4487-98eb-741fdf6f987f";
    if std::env::var_os(CHILD_ENV).is_some() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Queued);
        task.fixture_data().id = ID.to_string();
        std::fs::create_dir_all(app.store.task_path(ID)).unwrap();
        app.save_task_logged(&task);
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "engine::tests::save_task_logged_reports_task_id_on_failure",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!("orch: saving task {ID} failed:")),
        "{stderr}"
    );
}
