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
    // `sleep` are still running and share its process group (killpg's
    // target). A naive `timeout(...)` around `Command::output()` would
    // stop waiting here but leave the whole group running.
    let cmd = format!("sleep 60 & echo $! > {}; wait", pidfile.display());

    let cancel = CancelToken::new();
    let outcome = run_one_verify_command(
        tmp.path(),
        &cmd,
        Duration::from_millis(300),
        None,
        "",
        &cancel,
    )
    .await;
    assert!(outcome.tail.contains("timed out"));

    let pid_text = std::fs::read_to_string(&pidfile).expect("child wrote its pid");
    let pid: i32 = pid_text.trim().parse().expect("valid pid");
    let start = std::time::Instant::now();
    loop {
        let alive = unsafe { libc::kill(pid, 0) == 0 };
        if !alive {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "backgrounded sleep {pid} is still alive after the verify command timed out"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
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
    let outcome =
        run_one_verify_command(tmp.path(), &cmd, Duration::from_secs(60), None, "", &cancel).await;
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

async fn assert_pid_gone(pidfile: &Path) {
    let pid: i32 = std::fs::read_to_string(pidfile)
        .expect("the check wrote the pid")
        .trim()
        .parse()
        .expect("valid pid");
    let start = std::time::Instant::now();
    while unsafe { libc::kill(pid, 0) == 0 } {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "process {pid} is still alive"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn verify_detached_grandchild_inside_the_directory_is_stopped_on_timeout() {
    let tmp = tempfile::tempdir().unwrap();
    let pidfile = tmp.path().join("child.pid");
    let cmd = format!("{}; wait", detached_sleep_command(tmp.path(), &pidfile));
    let outcome = run_one_verify_command(
        tmp.path(),
        &cmd,
        Duration::from_millis(1500),
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
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        cancel2.cancel();
    });
    let outcome =
        run_one_verify_command(tmp.path(), &cmd, Duration::from_secs(60), None, "", &cancel).await;
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
    let outcome = run_one_verify_command(
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
