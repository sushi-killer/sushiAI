/// Starts a long-lived process whose executable the test placed in `dir`,
/// detached into its own session; writes its pid to `pidfile`.
fn detached_sleep_command(dir: &Path, pidfile: &Path) -> String {
    let exe = crate::procs::testing::place(dir);
    crate::procs::testing::detach(&exe, pidfile)
}

/// Waits until the process is gone (a killed process may stay a zombie until its parent
/// reaps it).
async fn assert_pid_gone_within(pid: i32) {
    crate::engine::test_support::wait_async(&format!("process {pid} to be gone"), || unsafe {
        libc::kill(pid, 0) != 0
    })
    .await;
}

async fn assert_pid_gone(pidfile: &Path) {
    let pid: i32 = std::fs::read_to_string(pidfile)
        .expect("the check wrote the pid")
        .trim()
        .parse()
        .expect("valid pid");
    assert_pid_gone_within(pid).await;
}

/// Starts the detached helper from a shell of its own and returns once it runs.
fn start_detached_sleeper(dir: &Path, pidfile: &Path) {
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(detached_sleep_command(dir, pidfile))
        .status()
        .unwrap();
    assert!(status.success());
    crate::procs::testing::wait_for_file(&crate::procs::testing::ready_file(pidfile));
}

#[tokio::test]
async fn verify_detached_process_inside_the_directory_is_stopped_on_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("child.pid");
    // The helper runs before the check starts, so the timeout cannot fire before its
    // executable is in place (a timer cannot wait for it).
    start_detached_sleeper(tmp.path(), &pidfile);
    let (app, _app_dir) = test_app();
    *app.check_load.write().unwrap() = || (0.0, 4);
    let outcome = run_one_verify_command(
        crate::engine::verification::runner::CheckRunner::from_context(
            CheckContext::new(&app, None),
            tmp.path(),
            None,
            "",
            &CancelToken::new(),
        )
        .with_timeout(Duration::from_millis(300)),
        "sleep 60",
    )
    .await;
    assert!(outcome.tail.contains("timed out"), "{}", outcome.tail);
    assert_pid_gone(&pidfile).await;
}

#[tokio::test]
async fn verify_detached_grandchild_inside_the_directory_is_stopped_on_cancel() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("child.pid");
    let cmd = format!("{}; wait", detached_sleep_command(tmp.path(), &pidfile));
    let cancel = CancelToken::new();
    let cancel2 = cancel.clone();
    // Cancel once the detached helper runs from the directory.
    let ready = crate::procs::testing::ready_file(&pidfile);
    tokio::spawn(async move {
        crate::engine::test_support::wait_async("the detached helper to run", || ready.exists())
            .await;
        cancel2.cancel();
    });
    let (app, _app_dir) = test_app();
    *app.check_load.write().unwrap() = || (0.0, 4);
    let outcome = run_one_verify_command(
        crate::engine::verification::runner::CheckRunner::from_context(
            CheckContext::new(&app, None),
            tmp.path(),
            None,
            "",
            &cancel,
        )
        .with_timeout(Duration::from_secs(60)),
        &cmd,
    )
    .await;
    assert_eq!(outcome.tail, "cancelled");
    assert_pid_gone(&pidfile).await;
}

#[tokio::test]
async fn verify_stops_a_process_left_in_the_group_after_a_normal_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("child.pid");
    let cmd = format!(
        "sleep 60 >/dev/null 2>&1 </dev/null & echo $! > {}",
        pidfile.display()
    );
    let (app, _app_dir) = test_app();
    *app.check_load.write().unwrap() = || (0.0, 4);
    let outcome = run_one_verify_command(
        crate::engine::verification::runner::CheckRunner::from_context(
            CheckContext::new(&app, None),
            tmp.path(),
            None,
            "",
            &CancelToken::new(),
        )
        .with_timeout(Duration::from_secs(60)),
        &cmd,
    )
    .await;
    assert_eq!(outcome.code, Some(0));
    assert_pid_gone(&pidfile).await;
}
