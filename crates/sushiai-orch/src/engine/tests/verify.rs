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
    let native = verify::build_verify_command(root, "true", Some(&policy), "base");
    let host = verify::build_verify_command(root, "true", None, "base");
    let electron_env = |command: &tokio::process::Command| {
        command
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "ELECTRON_DISABLE_SANDBOX")
            .and_then(|(_, value)| value.map(|value| value.to_os_string()))
    };
    assert_eq!(electron_env(&native), Some("1".into()));
    assert_eq!(electron_env(&host), None);
    assert_eq!(policy.check_env_remove_prefixes, vec!["SCCACHE_"]);
    let rustc_wrapper = |command: &tokio::process::Command| {
        command
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "RUSTC_WRAPPER")
            .and_then(|(_, value)| value.map(|value| value.to_os_string()))
    };
    assert_eq!(
        rustc_wrapper(&native).as_deref(),
        Some(std::ffi::OsStr::new(""))
    );
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
        CheckContext::new(&app, None),
        tmp.path(),
        &cmd,
        Duration::from_millis(300),
        None,
        "",
        &cancel,
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
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel2.cancel();
    });
    let (app, _app_dir) = test_app();
    *app.check_load.write().unwrap() = || (0.0, 4);
    let outcome = run_one_verify_command(
        CheckContext::new(&app, None),
        tmp.path(),
        &cmd,
        Duration::from_secs(60),
        None,
        "",
        &cancel,
    )
    .await;
    assert_eq!(outcome.tail, "cancelled");
}

#[test]
fn verify_tail_keeps_stdout_failures_behind_a_long_stderr() {
    let stderr = "   Compiling crate\n".repeat(500);
    let tail = verify_tail(
        b"test foo ... FAILED\nfailures:\n    foo\n",
        stderr.as_bytes(),
    );
    assert!(tail.contains("test foo ... FAILED"));
    assert!(tail.len() < 4200);
}

#[test]
fn verify_tail_surfaces_failing_test_names_and_assertions() {
    let stderr = "WARNING bundler chunk too large\n".repeat(300);
    let stdout = format!(
        "test a_broken_thing ... FAILED\n\nthread 'a_broken_thing' panicked at t.rs:3:5:\nassertion `left == right` failed: boom\n  left: 1\n right: 2\n{}",
        "test ok_one ... ok\n".repeat(400)
    );
    let tail = verify_tail(stdout.as_bytes(), stderr.as_bytes());
    assert!(tail.contains("a_broken_thing ... FAILED"));
    assert!(tail.contains("assertion `left == right` failed: boom"));
    assert!(tail.len() < 4500);
    assert!(tail.trim_end().ends_with("right: 2"));
}

#[test]
fn split_verify_commands_moves_prose_to_review() {
    let tmp = tempfile::tempdir().unwrap();
    let cmds: Vec<String> = [
        "true",
        "FOO=1 sh -c true",
        "(cd . && true)",
        "./scripts/created-by-the-task.sh",
        "npm run test:desktop (only if src/app/X.tsx ends up touched)",
        "ui-evidence skill: screenshot the panel's Archive section",
    ]
    .map(String::from)
    .to_vec();
    let (run, judged) = split_verify_commands(tmp.path(), &cmds);
    assert_eq!(
        run,
        vec![
            "true",
            "FOO=1 sh -c true",
            "(cd . && true)",
            "./scripts/created-by-the-task.sh"
        ]
    );
    assert_eq!(judged.len(), 2);
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

#[test]
fn verify_tail_surfaces_a_conventions_block_over_bundler_noise() {
    let stderr = format!(
        "{}Repository conventions failed:\n\n  - problem one\n  - problem two\nnext line\n  - not part of it\n",
        "WARNING bundler chunk too large\n".repeat(300)
    );
    let tail = verify_tail(b"", stderr.as_bytes());
    let failures = tail.split("--- failures ---\n").nth(1).unwrap();
    assert!(failures.contains("Repository conventions failed:"));
    assert!(failures.contains("- problem one"));
    assert!(failures.contains("- problem two"));
    assert!(!failures.contains("not part of it"));
}

#[test]
fn verify_tail_highlights_fail_lines_and_eslint_errors_only() {
    let tail = verify_tail(
        b"  FAIL tests/a.test.ts\n\xE2\x9C\x96 3 problems (2 errors, 1 warning)\n\xE2\x9C\x96 1 problem (0 errors, 1 warning)\n",
        b"",
    );
    let failures = tail.split("--- failures ---\n").nth(1).unwrap();
    assert!(failures.contains("FAIL tests/a.test.ts"));
    assert!(failures.contains("3 problems (2 errors"));
    assert!(!failures.contains("(0 errors"));
}

fn desktop_scope() -> Vec<ScopedCheck> {
    vec![ScopedCheck {
        repo: None,
        command: "npm run test:desktop".into(),
        paths: [
            "src/app/**",
            "src/extensions/**",
            "electron/**",
            "src/styles/**",
            "*.html",
        ]
        .map(String::from)
        .to_vec(),
    }]
}

#[test]
fn a_scoped_command_is_skipped_when_no_changed_file_is_under_its_paths() {
    let commands = vec!["npm run test:desktop".to_string(), "npm test".to_string()];
    let (run, lines) = filter_scoped(
        &desktop_scope(),
        "/repo",
        &commands,
        &["orchd/src/x.rs".into()],
    );
    assert_eq!(run, vec!["npm test"]);
    assert_eq!(
        lines,
        vec![
            "Orchestrator: skipped npm run test:desktop: no change under src/app/**, src/extensions/**, electron/**, src/styles/**, *.html"
        ]
    );
}

#[test]
fn a_scoped_command_runs_when_a_changed_file_matches() {
    let commands = vec!["npm run test:desktop".to_string()];
    for file in [
        "electron/main.cjs",
        "src/app/Shell.tsx",
        "index.html",
        "web/x.html",
    ] {
        let (run, lines) = filter_scoped(&desktop_scope(), "/repo", &commands, &[file.into()]);
        assert_eq!(run, commands, "{file}");
        assert!(lines.is_empty(), "{file}");
    }
}

#[test]
fn a_scoped_entry_for_another_repo_or_without_paths_does_not_scope() {
    let commands = vec!["make slow".to_string()];
    let other = vec![ScopedCheck {
        repo: Some("/other".into()),
        command: "make *".into(),
        paths: vec!["lib/**".into()],
    }];
    assert_eq!(filter_scoped(&other, "/repo", &commands, &[]).0, commands);
    let empty = vec![ScopedCheck {
        repo: None,
        command: "make *".into(),
        paths: vec![],
    }];
    assert_eq!(filter_scoped(&empty, "/repo", &commands, &[]).0, commands);
    let here = vec![ScopedCheck {
        repo: Some("/repo".into()),
        command: "make *".into(),
        paths: vec!["lib/**".into()],
    }];
    assert!(filter_scoped(&here, "/repo", &commands, &["app/x".into()])
        .0
        .is_empty());
}

#[test]
fn the_base_preflight_scopes_by_planned_paths() {
    let commands = vec!["npm run test:desktop".to_string(), "npm test".to_string()];
    let scope = desktop_scope();
    let run = |planned: &[&str]| {
        let planned: Vec<String> = planned.iter().map(|p| p.to_string()).collect();
        filter_scoped_planned(&scope, "/repo", &commands, &planned)
    };
    assert_eq!(run(&["orchd/src/model.rs", "docs/x.md"]), vec!["npm test"]);
    assert_eq!(run(&["electron/main.cjs"]), commands);
    // A planned directory counts when a glob falls under it.
    assert_eq!(run(&["src/app"]), commands);
    assert_eq!(run(&["src/"]), commands);
    assert_eq!(run(&["src/agents"]), vec!["npm test"]);
    assert_eq!(run(&["docs"]), vec!["npm test"]);
    assert_eq!(run(&["page.html"]), commands);
    // No planned paths: no way to tell, so it runs.
    assert_eq!(run(&[]), commands);
}

/// Starts a long-lived process whose executable the test placed in `dir`,
/// detached into its own session; writes its pid to `pidfile`.
fn detached_sleep_command(dir: &Path, pidfile: &Path) -> String {
    let exe = crate::procs::testing::place(dir);
    crate::procs::testing::detach(&exe, pidfile)
}

/// Waits until the process is gone (a killed process may stay a zombie until its parent
/// reaps it).
async fn assert_pid_gone_within(pid: i32) {
    let start = std::time::Instant::now();
    while unsafe { libc::kill(pid, 0) == 0 } {
        assert!(
            start.elapsed() < crate::procs::testing::deadline(),
            "process {pid} is still alive"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
        CheckContext::new(&app, None),
        tmp.path(),
        "sleep 60",
        Duration::from_millis(300),
        None,
        "",
        &CancelToken::new(),
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
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        cancel2.cancel();
    });
    let (app, _app_dir) = test_app();
    *app.check_load.write().unwrap() = || (0.0, 4);
    let outcome = run_one_verify_command(
        CheckContext::new(&app, None),
        tmp.path(),
        &cmd,
        Duration::from_secs(60),
        None,
        "",
        &cancel,
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
        CheckContext::new(&app, None),
        tmp.path(),
        &cmd,
        Duration::from_secs(60),
        None,
        "",
        &CancelToken::new(),
    )
    .await;
    assert_eq!(outcome.code, Some(0));
    assert_pid_gone(&pidfile).await;
}

fn suite(cwd: &std::path::Path, command: &str) -> bool {
    whole_repo_suite(cwd, command)
}

#[test]
fn whole_repo_suite_matches_the_package_manager_forms() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    for c in [
        "npm run ci",
        "npm run test",
        "npm run check",
        "npm test",
        "pnpm ci",
        "pnpm test",
        "pnpm run check",
        "yarn test",
        "yarn run ci",
        "yarn check",
        "make ci",
        "make test",
        "make check",
        "go test ./...",
        "pytest",
        "cd x && npm run ci",
        "true; npm test",
    ] {
        assert!(suite(p, c), "{c}");
    }
}

#[test]
fn whole_repo_suite_matches_cargo_test_without_a_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    assert!(suite(p, "cargo test"));
    assert!(suite(p, "cargo test --workspace"));
    assert!(suite(p, "cargo test -p x --workspace"));
    assert!(suite(p, "cargo test --all-targets -- --nocapture"));
    assert!(!suite(p, "cargo test -p x"));
    assert!(!suite(p, "cargo test --package x"));
    assert!(!suite(p, "cargo test --test lifecycle"));
    assert!(!suite(p, "cargo test --lib"));
    assert!(!suite(p, "cargo test --bin x"));
    assert!(!suite(p, "cargo test some_filter"));
}

#[test]
fn whole_repo_suite_leaves_narrow_commands_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path();
    for c in [
        "npm test -- foo",
        "npm ci",
        "npm run build",
        "pnpm test foo",
        "make build",
        "go test ./pkg/x",
        "go test -run TestX ./...",
        "pytest tests/test_x.py",
        "cargo fmt --check",
        "grep -q x file",
    ] {
        assert!(!suite(p, c), "{c}");
    }
}

#[test]
fn whole_repo_suite_reads_a_package_json_script_one_level_deep() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("package.json"),
        r#"{"scripts":{
            "test:daemon":"cargo fmt --check && cargo test --workspace",
            "test:one":"cargo test -p sushiai-orch --lib x",
            "wrapper":"npm run test:daemon",
            "lint":"eslint ."
        }}"#,
    )
    .unwrap();
    let p = tmp.path();
    assert!(suite(p, "npm run test:daemon"));
    assert!(suite(p, "pnpm run test:daemon"));
    assert!(!suite(p, "npm run test:one"));
    assert!(!suite(p, "npm run lint"));
    // One level only: the script it calls is not expanded.
    assert!(!suite(p, "npm run wrapper"));
    assert!(!suite(p, "npm run missing"));
    let empty = tempfile::tempdir().unwrap();
    assert!(!suite(empty.path(), "npm run test:daemon"));
}

async fn check_slots_settings(app: &App, parallel: u32, heavy: Option<u32>) {
    let mut settings = app.settings.read().unwrap().clone();
    settings.parallel = parallel;
    settings.heavy_checks = heavy;
    settings.sandbox = SandboxMode::Host;
    app.handle_settings_set(json!({"settings": settings}))
        .await
        .unwrap();
}

async fn check_slots_wait_for(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition became ready");
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
            CheckContext::new(&app, Some(&id)),
            &cwd,
            &command,
            timeout,
            None,
            "",
            &CancelToken::new(),
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
    // Waiting is deliberately longer than the third check's eventual run.
    tokio::time::sleep(Duration::from_millis(150)).await;
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
    task.repo = repo.to_string_lossy().into_owned();
    task.worktree = task.repo.clone();
    task.base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    task.criteria = vec!["requires a tool".into()];
    task.brief_check.done = true;
    task.brief_check.missing_tool = Some(MissingTool {
        criterion: 0,
        capability: "test capability".into(),
        tool_id: None,
    });
    app.store.save_task(&task).unwrap();
    let held = app.slots.clone().try_acquire_owned().unwrap();
    app.spawn_task_loop(task.id.clone(), true);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        app.store.load_task(&task.id).unwrap().unwrap().status,
        TaskStatus::Queued
    );
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
        CheckContext::new(&app, Some(&task.id)),
        dir.path(),
        command,
        Duration::from_secs(1),
        None,
        "",
        &CancelToken::new(),
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
        CheckContext::new(&app, Some(&task.id)),
        dir.path(),
        command,
        Duration::from_secs(1),
        None,
        "",
        &CancelToken::new(),
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
    task.final_verify = vec![command.into()];
    task.attempts.push(attempt_with_failure(1, "old"));
    task.attempts[0].status = AttemptStatus::Running;
    task.attempts[0].failure = None;
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
        crate::engine::task_loop::finish_after_review(
            &app,
            &id,
            &mut task,
            0,
            1,
            dir.path(),
            dir.path(),
            &settings,
            None,
            &mut 3,
            &Arc::new(StdMutex::new(None)),
            &CancelToken::new(),
            &mut None,
        ),
    )
    .await
    .expect("environment failure does not park on a base question");
    change_load.await.unwrap();
    assert_eq!(task.attempts[0].verify[0].load, 0.0);
    assert!(task.attempts[0].verify[0].load_timeout);
    assert!(matches!(tail, crate::engine::task_loop::Tail::Continue));
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
        &task,
        &task.attempts[0].verify[0],
        "old",
        dir.path(),
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
        &task,
        &ordinary,
        "old",
        dir.path(),
        &CancelToken::new()
    )
    .await
    .is_none());
    assert!(task.question.is_none());
    assert!(app.base_jobs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn check_slots_low_load_final_timeout_still_retries_then_asks_about_a_known_base_failure() {
    let (app, dir) = test_app();
    check_slots_settings(&app, 2, Some(2)).await;
    app.settings.write().unwrap().verify_timeout_secs = 1;
    app.settings.write().unwrap().answer_policy = false;
    *app.check_load.write().unwrap() = || (0.0, 8);
    let mut task = task_with_status(TaskStatus::Running);
    let command = "echo run >> count; if [ -f first ]; then exit 1; fi; touch first; exec sleep 5";
    task.final_verify = vec![command.into()];
    task.attempts.push(attempt_with_failure(1, "old"));
    task.attempts[0].status = AttemptStatus::Running;
    task.attempts[0].failure = None;
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
        let (app, dir, id, cancel) = (
            app.clone(),
            dir.path().to_path_buf(),
            id.clone(),
            cancel.clone(),
        );
        tokio::spawn(async move {
            let settings = app.settings.read().unwrap().clone();
            crate::engine::task_loop::finish_after_review(
                &app,
                &id,
                &mut task,
                0,
                1,
                &dir,
                &dir,
                &settings,
                None,
                &mut 3,
                &Arc::new(StdMutex::new(None)),
                &cancel,
                &mut None,
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
        stored.question.unwrap().kind,
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
    task.held_out = Some(Check {
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
