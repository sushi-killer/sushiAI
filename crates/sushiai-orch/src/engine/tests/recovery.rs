#[tokio::test]
async fn recover_on_start_skips_archived_tasks() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.archived = true;
    app.store.save_task(&task).unwrap();

    app.recover_on_start().unwrap();

    assert!(
        !app.controls.lock().unwrap().contains_key(&task.id),
        "an archived task must not get a loop started on recovery"
    );
}

fn running_implement_task(app: &App, dir: &Path) -> (Task, PathBuf) {
    let mut task = task_with_status(TaskStatus::Running);
    task.attempts.push(attempt_with_failure(1, "x"));
    let attempt = &mut task.attempts[0];
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

    let recovered = app
        .store
        .recover_interrupted_with(
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

    let recovered = app
        .store
        .recover_interrupted_with(
            |task, idx| app.run_fate(task, idx),
            |task, idx| {
                let run_dir = app.store.run_dir(&task.id, task.attempts[idx].n);
                settle_unfinished_cost(task, idx, &run_dir, &Default::default());
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
    task.attempts[0].pgid = Some(stranger);
    app.store.save_task(&task).unwrap();

    app.store
        .recover_interrupted_with(|task, idx| app.run_fate(task, idx), |_, _| {})
        .unwrap();

    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.attempts[0].status, AttemptStatus::Interrupted);
    assert!(group_alive(stranger), "a stranger's group must not be killed");
    kill_group(stranger);
}

#[tokio::test]
async fn a_live_group_of_the_run_is_resumed() {
    let (app, dir) = test_app();
    let (mut task, run_dir) = running_implement_task(&app, dir.path());
    std::fs::write(run_dir.join("events.raw"), "").unwrap();
    let run = spawn_group(&run_dir.join("events.exit"), false);
    task.attempts[0].pgid = Some(run);
    app.store.save_task(&task).unwrap();

    assert!(matches!(app.run_fate(&task, 0), crate::store::RunFate::Resume));
    kill_group(run);
}

/// An adopted run whose raw output is `raw` and whose group still lives.
fn adopted(app: &App, dir: &Path, raw: &str) -> (Task, i32) {
    let (mut task, run_dir) = running_implement_task(app, dir);
    std::fs::write(run_dir.join("events.raw"), raw).unwrap();
    let pgid = spawn_group(&run_dir.join("events.exit"), false);
    task.attempts[0].pgid = Some(pgid);
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
    task.attempts[0].pgid = Some(pgid);
    app.store.save_task(&task).unwrap();
    let variant = Variant {
        stall_timeout_secs: 1,
        ..Default::default()
    };

    let outcome = follow_within(&app, &task, Guard::for_variant(&variant, None)).await;

    assert!(outcome.stalled, "{:?}", outcome.error);
    // Any member, not just the leader (which is reaped by now).
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
    // Five seconds of stderr lines and no stdout, then a clean exit.
    let pgid = spawn_group_running(
        "for i in 1 2 3 4 5 6 7 8 9 10; do echo x >> \"${0%.exit}.stderr.log\"; sleep 0.5; done; echo 0 > \"$0\"",
        &run_dir.join("events.exit"),
        false,
    );
    task.attempts[0].pgid = Some(pgid);
    app.store.save_task(&task).unwrap();
    let variant = Variant {
        stall_timeout_secs: 2,
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
    assert!(!group_alive(pgid), "the adopted run must be stopped");
}

#[test]
fn a_run_that_ends_while_its_fate_is_decided_is_resumed_not_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let exit = dir.path().join("events.exit");
    // The group is seen dead, and the exit file lands in the same instant:
    // the wrapper writes it before its group can die.
    let fate = recovery::fate_of_run(&exit, || {
        std::fs::write(&exit, "0\n").unwrap();
        false
    });
    assert!(matches!(fate, crate::store::RunFate::Resume));
    let gone = dir.path().join("never.exit");
    assert!(matches!(
        recovery::fate_of_run(&gone, || false),
        crate::store::RunFate::Interrupted
    ));
}
