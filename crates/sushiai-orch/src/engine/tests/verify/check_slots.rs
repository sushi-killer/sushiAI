async fn check_slots_settings(app: &App, parallel: u32, heavy: Option<u32>) {
    let mut settings = app.settings.read().unwrap().clone();
    settings.parallel = parallel;
    settings.heavy_checks = heavy;
    settings.sandbox = SandboxMode::Host;
    app.handle_settings_set(json!({"settings": settings}))
        .await
        .unwrap();
}

async fn check_slots_wait_for(ready: impl FnMut() -> bool) {
    crate::engine::test_support::wait_async("the condition to become ready", ready).await;
}

fn check_slots_spawn(
    app: &Arc<App>,
    task: &Task,
    cwd: &Path,
    command: &str,
    timeout: Duration,
) -> tokio::task::JoinHandle<VerifyOutcome> {
    let (app, id, cwd, command) = (
        app.clone(),
        task.id.clone(),
        cwd.to_path_buf(),
        command.to_string(),
    );
    tokio::spawn(async move {
        run_one_verify_command(
            crate::engine::verification::runner::CheckRunner::from_context(
                CheckContext::new(&app, Some(&id)),
                &cwd,
                None,
                "",
                &CancelToken::new(),
            )
            .with_timeout(timeout),
            &command,
        )
        .await
    })
}

#[tokio::test]
async fn check_slots_two_checks_run_and_third_waits_with_a_saved_broadcast_decision() {
    let (app, dir) = test_app();
    check_slots_settings(&app, 10, Some(2)).await;
    *app.check_load.write().unwrap() = || (0.0, 8);
    let mut events = app.subscribe();
    let command = "touch started; while [ ! -f release ]; do sleep 0.02; done";
    let mut tasks = Vec::new();
    let mut paths = Vec::new();
    for i in 0..3 {
        let task = task_with_status(TaskStatus::Running);
        app.store.save_task(&task).unwrap();
        let path = dir.path().join(format!("check-{i}"));
        std::fs::create_dir(&path).unwrap();
        tasks.push(task);
        paths.push(path);
    }
    let first = check_slots_spawn(&app, &tasks[0], &paths[0], command, Duration::from_secs(10));
    let second = check_slots_spawn(&app, &tasks[1], &paths[1], command, Duration::from_secs(10));
    check_slots_wait_for(|| paths[0].join("started").exists() && paths[1].join("started").exists())
        .await;
    let queued_at = std::time::Instant::now();
    let third = check_slots_spawn(&app, &tasks[2], &paths[2], command, Duration::from_secs(10));
    let line = format!("Orchestrator: waiting for a check slot (2 of 2): {command}");
    check_slots_wait_for(|| {
        app.store
            .load_task(&tasks[2].id)
            .unwrap()
            .unwrap()
            .decisions
            .contains(&line)
    })
    .await;
    assert!(!paths[2].join("started").exists());
    assert_eq!(app.check_slots.usage(), (2, 2));
    assert!(
        std::iter::from_fn(|| events.try_recv().ok()).any(|event| matches!(event,
        Event::Task { task } if task.id == tasks[2].id && task.decisions.contains(&line)))
    );
    // The window is deliberately longer than the third check's eventual run.
    crate::engine::test_support::holds_for_async(
        "the third check stays unstarted",
        Duration::from_millis(150),
        || !paths[2].join("started").exists(),
    )
    .await;
    *app.check_load.write().unwrap() = || (0.5, 4);
    std::fs::write(paths[0].join("release"), "").unwrap();
    assert_eq!(first.await.unwrap().code, Some(0));
    check_slots_wait_for(|| paths[2].join("started").exists()).await;
    std::fs::write(paths[1].join("release"), "").unwrap();
    std::fs::write(paths[2].join("release"), "").unwrap();
    assert_eq!(second.await.unwrap().code, Some(0));
    let result = third.await.unwrap();
    assert_eq!(result.code, Some(0));
    assert_eq!((result.load, result.cores), (0.5, 4));
    assert!(
        // timing: the wall time since queueing covers the held window plus the run, never less
        queued_at.elapsed().as_millis() >= u128::from(result.ms) + 150,
        "execution excludes the slot wait"
    );
    // This is the loop's original copy, taken before the runner saved the line.
    record_check_outcomes(&app, &mut tasks[2], std::slice::from_ref(&result));
    record_check_outcomes(&app, &mut tasks[2], std::slice::from_ref(&result));
    app.store.save_task(&tasks[2]).unwrap();
    assert_eq!(
        app.store
            .load_task(&tasks[2].id)
            .unwrap()
            .unwrap()
            .decisions
            .iter()
            .filter(|d| *d == &line)
            .count(),
        1
    );
}

#[tokio::test]
async fn check_slots_settings_set_raises_heavy_checks_and_wakes_a_real_waiting_check() {
    let (app, dir) = test_app();
    check_slots_settings(&app, 1, Some(1)).await;
    let task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let holder = check_slots_spawn(
        &app,
        &task,
        dir.path(),
        "touch started; while [ ! -f release ]; do sleep 0.02; done",
        Duration::from_secs(10),
    );
    check_slots_wait_for(|| dir.path().join("started").exists()).await;
    let waiting = check_slots_spawn(
        &app,
        &task,
        dir.path(),
        "touch second",
        Duration::from_secs(5),
    );
    check_slots_wait_for(|| {
        app.store
            .load_task(&task.id)
            .unwrap()
            .unwrap()
            .decisions
            .iter()
            .any(|d| d.contains("waiting for a check slot"))
    })
    .await;
    assert!(!dir.path().join("second").exists());
    check_slots_settings(&app, 1, Some(2)).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap()
            .code,
        Some(0)
    );
    assert!(dir.path().join("second").exists());
    assert!(!holder.is_finished());
    std::fs::write(dir.path().join("release"), "").unwrap();
    assert_eq!(holder.await.unwrap().code, Some(0));
}

#[tokio::test]
async fn check_slots_settings_set_lowers_both_limits_and_retires_held_permits() {
    let (app, _dir) = test_app();
    check_slots_settings(&app, 3, Some(3)).await;
    let task_permits: Vec<_> = (0..3)
        .map(|_| app.slots.clone().try_acquire_owned().unwrap())
        .collect();
    // A zero-command test controls returning check permits without process timing.
    let check_permits: Vec<_> = (0..3)
        .map(|_| app.check_slots.clone().try_acquire_owned().unwrap())
        .collect();
    check_slots_settings(&app, 1, Some(1)).await;
    let mut task_permits = task_permits;
    let mut check_permits = check_permits;
    for _ in 0..2 {
        assert!(app.slots.clone().try_acquire_owned().is_err());
        assert!(app.check_slots.clone().try_acquire_owned().is_err());
        task_permits.pop();
        check_permits.pop();
    }
    assert!(app.slots.clone().try_acquire_owned().is_err());
    assert!(app.check_slots.clone().try_acquire_owned().is_err());
    task_permits.pop();
    check_permits.pop();
    let task_permit = app.slots.clone().try_acquire_owned().unwrap();
    let check_permit = app.check_slots.clone().try_acquire_owned().unwrap();
    assert!(app.slots.clone().try_acquire_owned().is_err());
    assert!(app.check_slots.clone().try_acquire_owned().is_err());
    // Growth first pays unretired shrink debt, including repeated changes.
    check_slots_settings(&app, 3, Some(3)).await;
    let extra_task = app.slots.clone().try_acquire_owned().unwrap();
    let extra_check = app.check_slots.clone().try_acquire_owned().unwrap();
    check_slots_settings(&app, 1, Some(1)).await;
    check_slots_settings(&app, 2, Some(2)).await;
    drop(extra_task);
    drop(extra_check);
    assert!(app.slots.clone().try_acquire_owned().is_ok());
    assert!(app.check_slots.clone().try_acquire_owned().is_ok());
    drop(task_permit);
    drop(check_permit);
}

#[tokio::test]
async fn check_slots_settings_set_raises_parallel_and_starts_a_waiting_task_loop() {
    let (app, dir) = test_app_parallel_one();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_git_repo(&repo);
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().repo = repo.to_string_lossy().into_owned();
    task.fixture_data().worktree = task.repo.clone();
    task.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    task.fixture_data().criteria = vec!["requires a tool".into()];
    task.fixture_data().brief_check.done = true;
    task.fixture_data().brief_check.missing_tool = Some(MissingTool {
        criterion: 0,
        capability: "test capability".into(),
        tool_id: Some("asana".into()),
    });
    app.store.save_task(&task).unwrap();
    let held = app.slots.clone().try_acquire_owned().unwrap();
    app.spawn_task_loop(task.id.clone(), true);
    crate::engine::test_support::holds_for_async(
        "the task stays queued while the slot is held",
        Duration::from_millis(100),
        || app.store.load_task(&task.id).unwrap().unwrap().status == TaskStatus::Queued,
    )
    .await;
    check_slots_settings(&app, 2, Some(2)).await;
    check_slots_wait_for(|| {
        app.store
            .load_task(&task.id)
            .unwrap()
            .unwrap()
            .question
            .is_some()
    })
    .await;
    // The missing-tool question is after slot admission and requires no agent run.
    let ctrl = app
        .controls
        .lock()
        .unwrap()
        .get(&task.id)
        .unwrap()
        .signals
        .cancel
        .clone();
    ctrl.cancel();
    check_slots_wait_for(|| !app.controls.lock().unwrap().contains_key(&task.id)).await;
    drop(held);
}

#[test]
fn check_slots_automatic_unlimited_and_legacy_settings_and_results() {
    assert_eq!(Settings::auto_heavy_checks(15), 3);
    assert_eq!(Settings::auto_heavy_checks(4), 2);
    assert_eq!(Settings::auto_heavy_checks(20), 4);
    let mut old = serde_json::to_value(Settings::default()).unwrap();
    assert!(old["heavyChecks"].is_null());
    old.as_object_mut().unwrap().remove("heavyChecks");
    let legacy: Settings = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(legacy.heavy_checks, None);
    assert_eq!(
        legacy.heavy_check_limit(),
        Settings::auto_heavy_checks(std::thread::available_parallelism().unwrap().get() as u32)
    );
    old["heavyChecks"] = json!(0);
    assert_eq!(
        serde_json::from_value::<Settings>(old)
            .unwrap()
            .heavy_check_limit(),
        0
    );
    let result: VerifyOutcome =
        serde_json::from_value(json!({"command": "true", "code": 0, "tail": "", "ms": 42}))
            .unwrap();
    assert_eq!(
        (result.ms, result.load, result.cores, result.load_timeout),
        (42, 0.0, 0, false)
    );
}

#[tokio::test]
async fn check_slots_zero_disables_the_limit_live_and_apps_are_isolated() {
    let (app, _dir) = test_app();
    let (other, _other_dir) = test_app();
    check_slots_settings(&app, 1, Some(1)).await;
    check_slots_settings(&other, 1, Some(1)).await;
    let held = app.check_slots.clone().try_acquire_owned().unwrap();
    assert!(other.check_slots.clone().try_acquire_owned().is_ok());
    assert!(app.check_slots.clone().try_acquire_owned().is_err());
    check_slots_settings(&app, 1, Some(0)).await;
    let permits: Vec<_> = (0..20)
        .map(|_| app.check_slots.clone().try_acquire_owned().unwrap())
        .collect();
    check_slots_settings(&app, 1, Some(1)).await;
    assert!(app.check_slots.clone().try_acquire_owned().is_err());
    drop(permits);
    assert!(app.check_slots.clone().try_acquire_owned().is_err());
    drop(held);
    assert!(app.check_slots.clone().try_acquire_owned().is_ok());
}

#[tokio::test]
async fn check_slots_high_load_timeout_retries_once_and_low_load_keeps_baseline_behavior() {
    let (app, dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    *app.check_load.write().unwrap() = || (8.0, 8);
    // 1 s, not 100 ms: a loaded CI runner may not reach the echo sooner.
    let command = "echo run >> count; exec sleep 5";
    let result = run_one_verify_command(
        crate::engine::verification::runner::CheckRunner::from_context(
            CheckContext::new(&app, Some(&task.id)),
            dir.path(),
            None,
            "",
            &CancelToken::new(),
        )
        .with_timeout(Duration::from_secs(1)),
        command,
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("count"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(classify_baseline(&result), Some(Baseline::Env));
    assert!(result.load_timeout);
    assert!(result.ms >= 1000);
    assert_eq!((result.load, result.cores), (8.0, 8));
    assert!(result.tail.contains("timed out under load 8.00 on 8 cores"));
    record_check_outcomes(&app, &mut task, std::slice::from_ref(&result));
    assert!(task
        .decisions
        .iter()
        .any(|d| d.contains(command) && d.contains("ms, timed out under load 8.00 on 8 cores")));
    assert!(task.question.is_none());
    std::fs::remove_file(dir.path().join("count")).unwrap();
    *app.check_load.write().unwrap() = || (7.9, 8);
    let low = run_one_verify_command(
        crate::engine::verification::runner::CheckRunner::from_context(
            CheckContext::new(&app, Some(&task.id)),
            dir.path(),
            None,
            "",
            &CancelToken::new(),
        )
        .with_timeout(Duration::from_secs(1)),
        command,
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(dir.path().join("count"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert!(!low.load_timeout);
    assert_eq!(classify_baseline(&low), Some(Baseline::Fail));
}

#[tokio::test]
async fn check_slots_high_load_final_retry_failure_never_looks_up_or_blames_the_base() {
    let (app, dir) = test_app();
    check_slots_settings(&app, 2, Some(2)).await;
    app.settings.write().unwrap().verify_timeout_secs = 1;
    *app.check_load.write().unwrap() = || (12.0, 8);
    let mut task = task_with_status(TaskStatus::Running);
    let command = "echo run >> count; if [ -f first ]; then exit 1; fi; touch first; exec sleep 5";
    task.fixture_data().final_verify = vec![command.into()];
    task.fixture_data()
        .attempts
        .push(attempt_with_failure(1, "old"));
    task.fixture_data().attempts[0].status = AttemptStatus::Running;
    task.fixture_data().attempts[0].failure = None;
    app.base_runs.lock().unwrap().insert(
        (task.base_sha.clone(), command.into()),
        VerifyOutcome {
            command: command.into(),
            code: Some(1),
            ..Default::default()
        },
    );
    app.store.save_task(&task).unwrap();
    let settings = app.settings.read().unwrap().clone();
    let change_load = {
        let (app, first) = (app.clone(), dir.path().join("first"));
        tokio::spawn(async move {
            check_slots_wait_for(|| first.exists()).await;
            *app.check_load.write().unwrap() = || (0.0, 8);
        })
    };
    let tail = tokio::time::timeout(
        Duration::from_secs(5),
        crate::engine::lifecycle::completion::finish::ReviewedAttempt::new(
            crate::engine::lifecycle::attempt_files::AttemptFiles::new(
                0,
                1,
                (dir.path()).to_path_buf(),
                (dir.path()).to_path_buf(),
            ),
            &settings,
            None,
        )
        .finish(
            &app,
            &mut task,
            &mut 3,
            &mut QuestionSlot::new(
                &crate::engine::run_control::TaskSignals {
                    resume: Default::default(),
                    cancel: CancelToken::new(),
                    pending_answer: Arc::new(StdMutex::new(None)),
                    pending_amend: Default::default(),
                },
                &mut None,
            ),
        ),
    )
    .await
    .expect("environment failure does not park on a base question");
    change_load.await.unwrap();
    assert_eq!(task.attempts[0].verify[0].load, 0.0);
    assert!(task.attempts[0].verify[0].load_timeout);
    assert!(matches!(
        tail,
        crate::engine::lifecycle::completion::Tail::Continue
    ));
    assert!(task.question.is_none());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("count"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    let failure = task.attempts[0].failure.as_ref().unwrap();
    assert_eq!(failure.kind, FailureKind::Verify);
    assert!(failure.detail.contains(command));
    assert!(failure
        .detail
        .contains("ms, timed out under load 12.00 on 8 cores"));
    assert!(broken_by_the_new_base(
        &app,
        crate::engine::verification::base::jobs::BaseCheckJob::new(
            Some(&task.id),
            &task.repo,
            &task.base_sha,
            dir.path(),
            false
        ),
        &task.attempts[0].verify[0],
        "old",
        &CancelToken::new()
    )
    .await
    .is_none());
    assert!(app.base_jobs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn check_slots_queued_permit_assigned_before_shrink_is_retired_before_admission() {
    use std::future::Future;
    let (app, _dir) = test_app();
    check_slots_settings(&app, 2, Some(2)).await;
    let first = app.slots.clone().try_acquire_owned().unwrap();
    let second = app.slots.clone().try_acquire_owned().unwrap();
    let mut waiting = std::pin::pin!(app.slots.clone().acquire_owned());
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(waiting.as_mut().poll(&mut context).is_pending());
    // Tokio assigns the returning permit to the waiter before it is polled.
    drop(first);
    check_slots_settings(&app, 1, Some(1)).await;
    assert!(waiting.as_mut().poll(&mut context).is_pending());
    assert_eq!(app.slots.usage(), (1, 1));
    drop(second);
    let permit = waiting.await.unwrap();
    assert_eq!(app.slots.usage(), (1, 1));
    assert!(app.slots.clone().try_acquire_owned().is_err());
    drop(permit);
}

#[tokio::test]
async fn check_slots_environment_retry_results_never_count_as_base_failures() {
    let (app, dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    let command = "true";
    let env = VerifyOutcome {
        command: command.into(),
        code: Some(1),
        load_timeout: true,
        ..Default::default()
    };
    let ordinary = VerifyOutcome {
        command: command.into(),
        code: Some(1),
        ..Default::default()
    };
    app.base_runs
        .lock()
        .unwrap()
        .insert((task.base_sha.clone(), command.into()), env);
    assert!(
        failing_twice_on_base(&app, &mut task, &[command.into()], &CancelToken::new())
            .await
            .is_empty()
    );
    assert!(
        failing_on_base(&app, &mut task, &ordinary, dir.path(), &CancelToken::new())
            .await
            .is_none()
    );
    assert!(broken_by_the_new_base(
        &app,
        crate::engine::verification::base::jobs::BaseCheckJob::new(
            Some(&task.id),
            &task.repo,
            &task.base_sha,
            dir.path(),
            false
        ),
        &ordinary,
        "old",
        &CancelToken::new()
    )
    .await
    .is_none());
    assert!(task.question.is_none());
    assert!(app.base_jobs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn check_slots_low_load_final_timeout_still_retries_then_asks_about_a_known_base_failure() {
    let _ = crate::engine::chat::test_support::fake_overseer_harness();
    let (app, dir) = test_app();
    check_slots_settings(&app, 2, Some(2)).await;
    app.settings.write().unwrap().verify_timeout_secs = 1;
    app.settings.write().unwrap().answer_policy = false;
    *app.check_load.write().unwrap() = || (0.0, 8);
    let mut task = task_with_status(TaskStatus::Running);
    let command = "echo run >> count; if [ -f first ]; then exit 1; fi; touch first; exec sleep 5";
    task.fixture_data().final_verify = vec![command.into()];
    task.fixture_data()
        .attempts
        .push(attempt_with_failure(1, "old"));
    task.fixture_data().attempts[0].status = AttemptStatus::Running;
    task.fixture_data().attempts[0].failure = None;
    app.base_runs.lock().unwrap().insert(
        (task.base_sha.clone(), command.into()),
        VerifyOutcome {
            command: command.into(),
            code: Some(1),
            ..Default::default()
        },
    );
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    let cancel = CancelToken::new();
    let run = {
        let (app, dir, cancel) = (app.clone(), dir.path().to_path_buf(), cancel.clone());
        tokio::spawn(async move {
            let settings = app.settings.read().unwrap().clone();
            crate::engine::lifecycle::completion::finish::ReviewedAttempt::new(
                crate::engine::lifecycle::attempt_files::AttemptFiles::new(
                    0,
                    1,
                    dir.to_path_buf(),
                    dir.to_path_buf(),
                ),
                &settings,
                None,
            )
            .finish(
                &app,
                &mut task,
                &mut 3,
                &mut QuestionSlot::new(
                    &crate::engine::run_control::TaskSignals {
                        resume: Default::default(),
                        cancel: cancel.clone(),
                        pending_answer: Arc::new(StdMutex::new(None)),
                        pending_amend: Default::default(),
                    },
                    &mut None,
                ),
            )
            .await;
            task
        })
    };
    check_slots_wait_for(|| {
        app.store
            .load_task(&id)
            .unwrap()
            .unwrap()
            .question
            .is_some()
    })
    .await;
    let stored = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(
        stored.question.clone().unwrap().kind,
        QuestionKind::PreexistingFailure
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("count"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    cancel.cancel();
    let task = run.await.unwrap();
    assert!(!task.attempts[0].verify[0].load_timeout);
}

#[tokio::test]
async fn check_slots_baseline_wait_decisions_do_not_reveal_the_held_out_command() {
    let (app, dir) = test_app();
    check_slots_settings(&app, 2, Some(1)).await;
    let mut task = task_with_status(TaskStatus::Running);
    let command = "printf secret-held-out";
    task.fixture_data().held_out = Some(Check {
        criterion: 0,
        run: command.into(),
        baseline: None,
    });
    app.store.save_task(&task).unwrap();
    let held = app.check_slots.clone().try_acquire_owned().unwrap();
    let check = check_slots_spawn(&app, &task, dir.path(), command, Duration::from_secs(5));
    check_slots_wait_for(|| {
        !app.store
            .load_task(&task.id)
            .unwrap()
            .unwrap()
            .decisions
            .is_empty()
    })
    .await;
    let line = "Orchestrator: waiting for a check slot (1 of 1): held-out check (criterion 0)";
    assert_eq!(
        app.store.load_task(&task.id).unwrap().unwrap().decisions,
        vec![line]
    );
    drop(held);
    let result = check.await.unwrap();
    assert_eq!(result.code, Some(0));
    assert_eq!(result.decisions, vec![line]);
}
