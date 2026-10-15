#[tokio::test]
async fn recover_on_start_skips_archived_tasks() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().archived = true;
    app.store.save_task(&task).unwrap();

    app.recover_on_start().unwrap();

    assert!(
        !app.controls.lock().unwrap().contains_key(&task.id),
        "an archived task must not get a loop started on recovery"
    );
}

#[tokio::test]
async fn recover_on_start_continues_after_unreadable_task_and_failed_saves() {
    use std::os::unix::fs::PermissionsExt;

    const CHILD_ENV: &str = "ORCH_TEST_RECOVERY_ERRORS_CHILD";
    if std::env::var_os(CHILD_ENV).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "engine::tests::recover_on_start_continues_after_unreadable_task_and_failed_saves",
                "--nocapture",
            ])
            .env(CHILD_ENV, "1")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("corrupt/task.json failed:"), "{stderr}");
        assert!(stderr.contains("orch: saving task"), "{stderr}");
        return;
    }

    let (app, _dir) = test_app();
    let healthy = task_with_status(TaskStatus::Queued);
    app.store.save_task(&healthy).unwrap();
    let mut interrupted = task_with_status(TaskStatus::Running);
    interrupted.fixture_data().created_at = 2;
    let mut attempt = attempt_with_failure(1, "interrupted");
    attempt.status = AttemptStatus::Running;
    attempt.failure = None;
    interrupted.fixture_data().attempts.push(attempt);
    app.store.save_task(&interrupted).unwrap();

    let mut unsavable = interrupted.clone();
    unsavable.fixture_data().id = uuid::Uuid::new_v4().to_string();
    unsavable.fixture_data().created_at = 3;
    app.store.save_task(&unsavable).unwrap();
    let unsavable_dir = app.store.task_dir(&unsavable.id);
    let unsavable_permissions = std::fs::metadata(&unsavable_dir).unwrap().permissions();

    let mut advisor = task_with_status(TaskStatus::Queued);
    advisor.fixture_data().created_at = 4;
    advisor.fixture_data().attempts.push(attempt_with_failure(1, "advisor"));
    app.store.save_task(&advisor).unwrap();
    let helper = app
        .store
        .claim_helper_run_dir(&advisor.id, "advisor", Some(1))
        .unwrap();
    std::fs::write(
        helper.join("events.jsonl"),
        "{\"type\":\"result\",\"total_cost_usd\":0.25}\n",
    )
    .unwrap();
    let advisor_dir = app.store.task_dir(&advisor.id);
    let advisor_permissions = std::fs::metadata(&advisor_dir).unwrap().permissions();

    std::fs::create_dir_all(app.store.task_path("unreadable")).unwrap();
    std::fs::create_dir_all(app.store.task_dir("corrupt")).unwrap();
    std::fs::write(app.store.task_path("corrupt"), "not task JSON").unwrap();
    std::fs::set_permissions(&unsavable_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    std::fs::set_permissions(&advisor_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let failed_save = app.store.save_task(&unsavable);
    app.recover_on_start().unwrap();
    std::fs::set_permissions(&unsavable_dir, unsavable_permissions).unwrap();
    std::fs::set_permissions(&advisor_dir, advisor_permissions).unwrap();

    assert!(
        failed_save.is_err(),
        "the recovery fixture must prevent atomic writes"
    );
    assert!(app.recovery_timers_started());
    let controls = app.controls.lock().unwrap();
    for task in [&healthy, &interrupted, &unsavable, &advisor] {
        assert!(
            controls.contains_key(&task.id),
            "{} has no recovery loop",
            task.id
        );
    }
    drop(controls);
    let recovered = app.store.load_task(&interrupted.id).unwrap().unwrap();
    assert_eq!(recovered.status, TaskStatus::Queued);
    assert_eq!(recovered.attempts[0].status, AttemptStatus::Interrupted);
    let failed = app.store.load_task(&unsavable.id).unwrap().unwrap();
    assert_eq!(failed.attempts[0].status, AttemptStatus::Running);
}

#[tokio::test]
async fn recover_on_start_starts_timers_after_listing_failures() {
    let (app, _dir) = test_app();
    let tasks = app.data_dir.join("tasks");
    std::fs::remove_dir(&tasks).unwrap();
    std::fs::write(&tasks, "not a directory").unwrap();
    std::fs::create_dir_all(app.store.audit_dir("unreadable").join("audit.json")).unwrap();

    app.recover_on_start().unwrap();

    std::fs::remove_file(&tasks).unwrap();
    std::fs::create_dir(&tasks).unwrap();
    assert!(app.recovery_timers_started());
}

fn running_implement_task(app: &App, dir: &Path) -> (Task, PathBuf) {
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().attempts.push(attempt_with_failure(1, "x"));
    let attempt = &mut task.fixture_data().attempts[0];
    attempt.status = AttemptStatus::Running;
    attempt.failure = None;
    // A process group that is gone: a short child, reaped.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    attempt.pgid = Some(child.id() as i32);
    child.wait().unwrap();
    app.store.save_task(&task).unwrap();
    let run_dir = dir.join("data/tasks").join(&task.id).join("runs/1");
    std::fs::create_dir_all(&run_dir).unwrap();
    (task, run_dir)
}

#[tokio::test]
async fn a_run_that_ended_while_the_daemon_was_down_is_resumed_from_its_files() {
    let (app, dir) = test_app();
    let (task, run_dir) = running_implement_task(&app, dir.path());
    std::fs::write(
        run_dir.join("events.raw"),
        "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\"}\n{\"type\":\"result\",\"total_cost_usd\":0.5,\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}\n",
    )
    .unwrap();
    std::fs::write(run_dir.join("events.exit"), "1\n").unwrap();
    std::fs::write(run_dir.join("events.stderr.log"), "boom\n").unwrap();

    let recovered = crate::engine::recovery::interrupted::tasks(
        &app.store,
        |task, idx| app.run_fate(task, idx),
        |_, _| panic!("a run with its files is not interrupted"),
    )
    .unwrap();
    assert!(recovered.is_empty());
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.attempts[0].status, AttemptStatus::Running);
    assert_eq!(recovery::resumable_attempt(&app, &task), Some(0));

    // The loop follows it and gets the outcome run_harness would have given.
    let outcome = recovery::follow_run(&app.arc(), &task, 0, &CancelToken::new(), Guard::off())
        .await
        .ok()
        .expect("outcome");
    assert_eq!(outcome.error.as_deref(), Some("boom"));
    assert_eq!(outcome.session_id.as_deref(), Some("s1"));
    assert_eq!(outcome.cost_usd, Some(0.5));
    let log = std::fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
    assert!(log.contains("total_cost_usd"), "{log}");
}

#[tokio::test]
async fn a_dead_run_without_an_exit_file_is_interrupted_and_its_spend_kept() {
    let (app, dir) = test_app();
    let (task, run_dir) = running_implement_task(&app, dir.path());
    std::fs::write(
        run_dir.join("events.raw"),
        "{\"type\":\"result\",\"total_cost_usd\":0.25,\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}\n",
    )
    .unwrap();

    let recovered = crate::engine::recovery::interrupted::tasks(
        &app.store,
        |task, idx| app.run_fate(task, idx),
        |task, idx| {
            settle_unfinished_cost(task, idx, &app.store, &Default::default());
        },
    )
    .unwrap();

    assert_eq!(recovered.len(), 1);
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Queued);
    assert_eq!(task.attempts[0].status, AttemptStatus::Interrupted);
    assert_eq!(task.attempts[0].cost_usd, Some(0.25));
}

/// A process group that looks like a run's wrapper shell: the exit file is
/// its `$0`. A thread reaps it, as init would reap a run of an earlier daemon.
fn spawn_group(exit: &Path, foreign: bool) -> i32 {
    spawn_group_running("sleep 30; echo $? > \"$0\"", exit, foreign)
}

fn spawn_group_running(script: &str, exit: &Path, foreign: bool) -> i32 {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new("/bin/sh");
    cmd.arg("-c").arg(script);
    cmd.arg(if foreign { Path::new("/no-run") } else { exit });
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().unwrap();
    let pgid = child.id() as i32;
    std::thread::spawn(move || child.wait());
    pgid
}

fn kill_group(pgid: i32) {
    unsafe {
        libc::killpg(pgid, libc::SIGKILL);
    }
}

#[tokio::test]
async fn a_live_group_that_is_not_the_run_is_treated_as_dead_and_left_alone() {
    let (app, dir) = test_app();
    let (mut task, run_dir) = running_implement_task(&app, dir.path());
    std::fs::write(run_dir.join("events.raw"), "").unwrap();
    // The saved pgid now belongs to a stranger's process.
    let stranger = spawn_group(&run_dir.join("events.exit"), true);
    task.fixture_data().attempts[0].pgid = Some(stranger);
    app.store.save_task(&task).unwrap();

    crate::engine::recovery::interrupted::tasks(
        &app.store,
        |task, idx| app.run_fate(task, idx),
        |_, _| {},
    )
    .unwrap();

    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.attempts[0].status, AttemptStatus::Interrupted);
    assert!(
        group_alive(stranger),
        "a stranger's group must not be killed"
    );
    kill_group(stranger);
}

#[tokio::test]
async fn a_live_group_of_the_run_is_resumed() {
    let (app, dir) = test_app();
    let (mut task, run_dir) = running_implement_task(&app, dir.path());
    std::fs::write(run_dir.join("events.raw"), "").unwrap();
    let run = spawn_group(&run_dir.join("events.exit"), false);
    task.fixture_data().attempts[0].pgid = Some(run);
    app.store.save_task(&task).unwrap();

    assert!(matches!(
        app.run_fate(&task, 0),
        crate::harness::run_files::RunFate::Resume
    ));
    kill_group(run);
}

/// An adopted run whose raw output is `raw` and whose group still lives.
fn adopted(app: &App, dir: &Path, raw: &str) -> (Task, i32) {
    let (mut task, run_dir) = running_implement_task(app, dir);
    std::fs::write(run_dir.join("events.raw"), raw).unwrap();
    let pgid = spawn_group(&run_dir.join("events.exit"), false);
    task.fixture_data().attempts[0].pgid = Some(pgid);
    app.store.save_task(&task).unwrap();
    (task, pgid)
}

async fn follow_within(app: &App, task: &Task, guard: Guard) -> harness::RunOutcome {
    tokio::time::timeout(
        Duration::from_secs(20),
        recovery::follow_run(&app.arc(), task, 0, &CancelToken::new(), guard),
    )
    .await
    .expect("the guard never stopped the adopted run")
    .ok()
    .expect("outcome")
}

#[tokio::test]
async fn a_stalled_adopted_run_is_killed_after_the_stall_timeout() {
    let (app, dir) = test_app();
    let init = "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\"}\n";
    let (task, pgid) = adopted(&app, dir.path(), init);
    let variant = Variant {
        stall_timeout_secs: 1,
        ..Default::default()
    };

    let outcome = follow_within(&app, &task, Guard::for_variant(&variant, None)).await;

    assert!(outcome.stalled, "{:?}", outcome.error);
    // The kill is sent at once; a loaded machine takes a moment to tear the group down.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while group_alive(pgid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!group_alive(pgid), "the adopted run must be stopped");
}

#[tokio::test]
async fn a_stalled_adopted_run_whose_member_traps_sigterm_is_still_killed() {
    let (app, dir) = test_app();
    let (mut task, run_dir) = running_implement_task(&app, dir.path());
    std::fs::write(run_dir.join("events.raw"), "").unwrap();
    // The leader dies on SIGTERM at once; a member ignores it.
    let pgid = spawn_group_running(
        "(trap '' TERM; exec sleep 60) & wait",
        &run_dir.join("events.exit"),
        false,
    );
    task.fixture_data().attempts[0].pgid = Some(pgid);
    app.store.save_task(&task).unwrap();
    let variant = Variant {
        stall_timeout_secs: 1,
        ..Default::default()
    };

    let outcome = follow_within(&app, &task, Guard::for_variant(&variant, None)).await;

    assert!(outcome.stalled, "{:?}", outcome.error);
    // Any member, not just the leader. The leader is this test's own child: reap it, or
    // its zombie still counts as a member; teardown after SIGKILL also takes a moment.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while unsafe { libc::kill(-pgid, 0) } == 0 && std::time::Instant::now() < deadline {
        unsafe { libc::waitpid(-pgid, std::ptr::null_mut(), libc::WNOHANG) };
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        unsafe { libc::kill(-pgid, 0) } != 0,
        "a member that traps SIGTERM must still be killed"
    );
}

#[tokio::test]
async fn an_adopted_run_that_only_writes_stderr_is_not_stalled() {
    let (app, dir) = test_app();
    let (mut task, run_dir) = running_implement_task(&app, dir.path());
    std::fs::write(run_dir.join("events.raw"), "").unwrap();
    // Ten seconds of stderr lines and no stdout, then a clean exit.
    let pgid = spawn_group_running(
        "for i in $(seq 20); do echo x >> \"${0%.exit}.stderr.log\"; sleep 0.5; done; echo 0 > \"$0\"",
        &run_dir.join("events.exit"),
        false,
    );
    task.fixture_data().attempts[0].pgid = Some(pgid);
    app.store.save_task(&task).unwrap();
    let variant = Variant {
        stall_timeout_secs: 4,
        ..Default::default()
    };

    let outcome = follow_within(&app, &task, Guard::for_variant(&variant, None)).await;

    assert!(!outcome.stalled, "{:?}", outcome.error);
    kill_group(pgid);
}

#[tokio::test]
async fn an_over_budget_adopted_run_is_killed() {
    let (app, dir) = test_app();
    let message = "{\"type\":\"assistant\",\"message\":{\"model\":\"claude-opus-5-5\",\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":1000000,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":0,\"output_tokens\":1000000}},\"session_id\":\"s1\"}\n";
    let (task, pgid) = adopted(&app, dir.path(), message);
    let variant = Variant {
        max_attempt_cost_usd: 0.01,
        ..Default::default()
    };

    let outcome = follow_within(&app, &task, Guard::for_variant(&variant, None)).await;

    assert!(outcome.over_budget, "{:?}", outcome.error);
    // The kill is sent at once; a loaded machine takes a moment to tear the group down.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while group_alive(pgid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!group_alive(pgid), "the adopted run must be stopped");
}

#[test]
fn a_run_that_ends_while_its_fate_is_decided_is_resumed_not_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let exit = dir.path().join("events.exit");
    // The group is seen dead, and the exit file lands in the same instant:
    // the wrapper writes it before its group can die.
    let fate = crate::harness::run_files::fate_of_run(&exit, || {
        std::fs::write(&exit, "0\n").unwrap();
        false
    });
    assert!(matches!(fate, crate::harness::run_files::RunFate::Resume));
    let gone = dir.path().join("never.exit");
    assert!(matches!(
        crate::harness::run_files::fate_of_run(&gone, || false),
        crate::harness::run_files::RunFate::Interrupted
    ));
}
