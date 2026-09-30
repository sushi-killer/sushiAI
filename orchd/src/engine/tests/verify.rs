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
        SandboxMode::Host,
        &[],
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
    let outcome = run_one_verify_command(
        tmp.path(),
        &cmd,
        Duration::from_secs(60),
        SandboxMode::Host,
        &[],
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
        paths: ["src/app/**", "src/extensions/**", "electron/**", "src/styles/**", "*.html"]
            .map(String::from)
            .to_vec(),
    }]
}

#[test]
fn a_scoped_command_is_skipped_when_no_changed_file_is_under_its_paths() {
    let commands = vec!["npm run test:desktop".to_string(), "npm test".to_string()];
    let (run, lines) = filter_scoped(&desktop_scope(), "/repo", &commands, &["orchd/src/x.rs".into()]);
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
    for file in ["electron/main.cjs", "src/app/Shell.tsx", "index.html", "web/x.html"] {
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
    assert!(filter_scoped(&here, "/repo", &commands, &["app/x".into()]).0.is_empty());
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
