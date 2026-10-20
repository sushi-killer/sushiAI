#[test]
fn native_check_command_gets_electron_env_and_host_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let policy = TaskSandbox::build(crate::sandbox::Inputs {
        repo_root: root,
        worktree: root,
        run_dir: &root.join("run"),
        data_dir: &root.join("data"),
        sushiai_home: &root.join("sushiai"),
        worktree_root: "",
        home: &root.join("home"),
        daemon_socket: &root.join("daemon.sock"),
        cargo_home: None,
        rustup_home: None,
        tmp_dir: None,
        npm_cache: None,
        known_project_folders: &[],
        known_task_worktrees: &[],
    });
    let native = crate::verify::command::build_verify_command(root, "true", Some(&policy), "base");
    let host = crate::verify::command::build_verify_command(root, "true", None, "base");
    let electron_env = |command: &tokio::process::Command| {
        command
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "ELECTRON_DISABLE_SANDBOX")
            .and_then(|(_, value)| value.map(|value| value.to_os_string()))
    };
    assert_eq!(electron_env(&native), Some("1".into()));
    assert_eq!(electron_env(&host), None);
    let wrapper = |command: &tokio::process::Command, name: &str| {
        command
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == name)
            .and_then(|(_, value)| value.map(|value| value.to_os_string()))
    };
    for name in ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"] {
        assert_eq!(
            wrapper(&native, name).as_deref(),
            Some(std::ffi::OsStr::new(""))
        );
        assert_eq!(wrapper(&host, name), None);
    }
    let cargo_rustc_wrapper = |command: &tokio::process::Command| {
        command
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "CARGO_BUILD_RUSTC_WRAPPER")
            .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
    };
    assert_eq!(cargo_rustc_wrapper(&native), None);
    #[cfg(target_os = "macos")]
    assert_eq!(native.as_std().get_program(), "sandbox-exec");
    assert_eq!(host.as_std().get_program(), "/bin/sh");
}

#[tokio::test]
async fn run_one_verify_command_kills_the_whole_process_group_on_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("child.pid");
    // `wait` keeps the `sh -c` process itself alive for the full sleep,
    // so the 300ms timeout fires while both it and the backgrounded
    // `sleep` (if the shell got that far) are still running and share its
    // process group (killpg's target). A naive `timeout(...)` around `Command::output()` would
    // stop waiting here but leave the whole group running.
    let cmd = format!("sleep 60 & echo $! > {}; wait", pidfile.display());

    let cancel = CancelToken::new();
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
        .with_timeout(Duration::from_millis(300)),
        &cmd,
    )
    .await;
    assert!(outcome.tail.contains("timed out"));

    // The timeout can fire before the shell started the sleep: then there is nothing to
    // outlive it. Whatever the shell did start was in the group and must be gone.
    let pid = std::fs::read_to_string(&pidfile)
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok());
    if let Some(pid) = pid {
        assert_pid_gone_within(pid).await;
    }
}

#[tokio::test]
async fn run_one_verify_command_kills_the_group_on_cancellation_too() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("child.pid");
    let cmd = format!("sleep 60 & echo $! > {}; wait", pidfile.display());
    let cancel = CancelToken::new();
    let cancel2 = cancel.clone();
    // Cancel once the command has started its background child.
    let started = pidfile.clone();
    tokio::spawn(async move {
        crate::engine::test_support::wait_async("the command to start its child", || {
            std::fs::read_to_string(&started).is_ok_and(|pid| !pid.trim().is_empty())
        })
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
}

#[test]
fn diff_hash_changes_when_an_untracked_file_is_edited() {
    let tmp = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(tmp.path())
            .output()
            .unwrap()
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.email=t@example.com",
        "-c",
        "user.name=t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "init",
    ]);
    std::fs::write(tmp.path().join("new.rs"), "broken").unwrap();
    let before = diff_hash(tmp.path(), "HEAD");
    std::fs::write(tmp.path().join("new.rs"), "fixed").unwrap();
    assert_ne!(before, diff_hash(tmp.path(), "HEAD"));
    // A moved base is a different tree to verify, even when the task's
    // own diff reads the same.
    git(&[
        "-c",
        "user.email=t@example.com",
        "-c",
        "user.name=t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "base moves",
    ]);
    assert_ne!(
        diff_hash(tmp.path(), "HEAD~1"),
        diff_hash(tmp.path(), "HEAD")
    );
    // Never followed: reading /dev/zero through it would never finish.
    std::os::unix::fs::symlink("/dev/zero", tmp.path().join("zero")).unwrap();
    diff_hash(tmp.path(), "HEAD");
}
