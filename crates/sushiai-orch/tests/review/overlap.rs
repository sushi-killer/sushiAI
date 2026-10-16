use super::*;

#[test]
fn review_and_final_checks_overlap_and_passing_finals_run_once() {
    let (settled, log) = overlapping_review("pass");
    assert_eq!(settled["status"], "done", "{settled}");
    // The fake reviewer cannot finish before the final-start marker exists.
    assert!(log.path().join("review-end").exists());
    assert!(log.path().join("final-done").exists());
    assert_eq!(
        std::fs::read_to_string(log.path().join("final-runs"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert_eq!(
        settled["attempts"][0]["review"]["verdict"], "PASS",
        "{settled}"
    );
}

#[test]
fn a_preflight_ruling_drops_a_failing_final_before_review_starts() {
    let log = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        log.path(),
        "preflight.sh",
        r###"#!/bin/sh
brief=$(cat)
case "$brief" in
*"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
*sushi-plan*)
  cat "$LOG_DIR/plan.jsonl" ;;
*"You check a coding task brief"*)
  printf '%s\n' '{"type":"result","result":"{\"contradiction\":false}"}' ;;
*"You judge whether a check command"*)
  while [ ! -f "$LOG_DIR/implemented" ]; do sleep 0.02; done
  printf '%s\n' '{"type":"result","result":"{\"verdict\":\"unrelated\",\"reason\":\"broken on the base\"}"}' ;;
'## Route the review'*) exit 0 ;;
"## Review"*)
  echo reviewed >> "$LOG_DIR/review-runs"
  printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*)
  echo changed > CHANGED_MARKER.txt
  touch "$LOG_DIR/implemented"
  cp "$LOG_DIR/check.sh" check.sh
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###,
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("LOG_DIR", log.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["briefCheckRoute"] = json!("claude-sonnet");
    settings["review"] = json!("claude-opus");
    settings["maxAttempts"] = json!(1);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    // Fails on the base, passes once the implementer wrote the marker.
    let command = "test -f CHANGED_MARKER.txt".to_string();
    let plan = json!({
        "title": "Preflight before review", "goal": "Write the marker",
        "tier": "standard", "criteria": ["The marker exists"],
        "verify": [command], "finalVerify": [command],
    });
    let reply = json!({
        "type": "result",
        "result": format!("```sushi-plan\n{plan}\n```"),
    });
    std::fs::write(log.path().join("plan.jsonl"), format!("{reply}\n")).unwrap();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "write the marker", "start": true,
        }),
    );
    let settled = poll_until(&daemon, task["id"].as_str().unwrap(), |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    assert_eq!(settled["status"], "done", "{settled}");
    let implements: Vec<&serde_json::Value> = settled["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert_eq!(implements.len(), 1);
    let attempt = implements[0];
    assert_eq!(attempt["review"]["verdict"], "PASS", "{settled}");
    assert_eq!(
        settled["briefCheck"]["nonGating"],
        json!([command]),
        "{settled}"
    );
    assert!(
        settled["finalVerify"].as_array().is_none_or(Vec::is_empty),
        "{settled}"
    );
    assert_eq!(
        std::fs::read_to_string(log.path().join("review-runs"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(!settled["decisions"]
        .to_string()
        .contains("review discarded"));
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_failed_final_discards_the_running_review_and_counts_its_cost() {
    let (settled, log) = overlapping_review("final-fails");
    assert_eq!(settled["status"], "failed", "{settled}");
    let attempt = &settled["attempts"][0];
    assert_eq!(attempt["failure"]["kind"], "verify", "{settled}");
    assert!(attempt.get("review").is_none(), "{settled}");
    assert!(!log.path().join("review-end").exists());
    assert!(
        (attempt["reviewCostUsd"].as_f64().unwrap() - 0.04).abs() < 1e-9,
        "{settled}"
    );
    assert!(
        (attempt["costUsd"].as_f64().unwrap() - 0.01).abs() < 1e-9,
        "{settled}"
    );
    // Later attempts of the ladder add their own spend.
    assert!(
        settled["costUsd"].as_f64().unwrap() >= 0.05 - 1e-9,
        "{settled}"
    );
    assert!(
        settled["decisions"]
            .to_string()
            .contains("review discarded because a final check failed"),
        "{settled}"
    );
    assert_ne!(
        settled["question"]["kind"], "review_no_verdict",
        "{settled}"
    );
}

#[test]
fn a_failed_review_cancels_the_running_final_check() {
    let (settled, log) = overlapping_review("review-fails");
    assert_eq!(settled["status"], "failed", "{settled}");
    assert_eq!(
        settled["attempts"][0]["failure"]["kind"], "review",
        "{settled}"
    );
    assert!(log.path().join("final-start").exists());
    assert!(!log.path().join("final-done").exists());
    assert!(
        settled["decisions"]
            .to_string()
            .contains("review failed; cancelled the final checks"),
        "{settled}"
    );
}

#[test]
fn final_checks_run_once_alongside_review_and_gate_the_commit() {
    // The failing check passes on the base and fails once the marker exists.
    for (final_check, status) in [("test ! -f CHANGED_MARKER.txt", "failed"), ("true", "done")] {
        let scripts_dir = tempfile::tempdir().unwrap();
        // Implements, and as the (Claude) reviewer answers a plain PASS.
        let script = fake_harness_script(
            scripts_dir.path(),
            "fake-contract.sh",
            FAKE_SCREENSHOT_SCRIPT,
        );
        let ran_log = scripts_dir.path().join("final.log");
        let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["maxAttempts"] = serde_json::json!(1);
        settings["review"] = serde_json::json!("claude-opus");
        daemon.request("settings.set", serde_json::json!({"settings": settings}));

        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "Final checks",
                "goal": "Write the marker",
                "verify": ["true"],
                "finalVerify": [format!("echo ran >> {}; {final_check}", ran_log.display())],
                "start": true,
            }),
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let settled = poll_task_status(&daemon, &task_id);
        assert_eq!(settled["status"], status, "{final_check}: {settled}");
        let attempt = &settled["attempts"][0];
        if status == "done" {
            assert_eq!(attempt["review"]["verdict"], "PASS", "{settled}");
        } else {
            assert!(attempt.get("review").is_none(), "{settled}");
            assert!(
                settled["decisions"]
                    .to_string()
                    .contains("review discarded because a final check failed"),
                "{settled}"
            );
        }
        if status == "failed" {
            assert!(
                attempt["failure"]["detail"]
                    .as_str()
                    .unwrap()
                    .starts_with("Final check echo ran"),
                "{settled}"
            );
        }
        let worktree = task["worktree"].as_str().unwrap().to_string();
        let ran = std::fs::read_to_string(&ran_log).unwrap();
        // Once on the base before the attempt, once alongside each review;
        // a failing final runs on three attempts (the budget of one, then
        // the changed approach) before the ladder ends.
        let expected = if status == "done" { 2 } else { 4 };
        assert_eq!(ran.lines().count(), expected, "{final_check}: {ran}");
        let brief = std::fs::read_to_string(
            daemon
                .data_dir()
                .join("tasks")
                .join(&task_id)
                .join("runs/1/brief.md"),
        )
        .unwrap();
        assert!(
            brief.contains("## Final checks") && brief.contains("do not run them yourself"),
            "{brief}"
        );

        daemon.shutdown_and_wait();
        remove_dir_retrying(&worktree);
    }
}

#[test]
fn a_failed_final_check_shows_the_failing_test_not_the_bundler_noise() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-contract.sh",
        FAKE_SCREENSHOT_SCRIPT,
    );
    let check = fake_harness_script(
        scripts_dir.path(),
        "noisy-check.sh",
        "#!/bin/sh\n[ -f CHANGED_MARKER.txt ] || exit 0\ni=0\nwhile [ $i -lt 300 ]; do echo \"WARNING bundler chunk $i is large\" >&2; i=$((i+1)); done\necho 'test a_broken_thing ... FAILED'\necho \"thread 'a_broken_thing' panicked at t.rs:3:5:\"\necho 'assertion failed: boom_message'\nj=0\nwhile [ $j -lt 300 ]; do echo \"test fine_$j ... ok\"; j=$((j+1)); done\nexit 1\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(1);
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Noisy final check",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [check.to_str().unwrap()],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "failed", "{settled}");
    let detail = settled["attempts"][0]["failure"]["detail"]
        .as_str()
        .unwrap();
    assert!(detail.contains("a_broken_thing ... FAILED"), "{detail}");
    assert!(detail.contains("boom_message"), "{detail}");
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(&worktree);
}
