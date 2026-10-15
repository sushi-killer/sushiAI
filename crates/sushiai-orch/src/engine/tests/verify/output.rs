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
