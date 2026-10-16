use super::*;

#[test]
fn a_review_without_a_verdict_is_committed_marked_unreviewed_and_nobody_is_asked() {
    // One fake plays both roles: the implementer edits a file and reports;
    // the reviewer (its brief opens with "## Review") answers in prose with
    // no sushi-review block, which used to count as PASS.
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\nbrief=$(cat)\ncase \"$brief\" in\n'## Route the review'*) exit 0 ;;\n\"## Review\"*) echo '{\"type\":\"result\",\"result\":\"Looks fine to me.\"}' ;;\n*) echo changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}' ;;\nesac\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let final_runs = scripts_dir.path().join("final-runs");
    // Review by the Claude hard route, which is the same fake (`auto` would
    // pick a route on the other harness).
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    assert_eq!(settings["review"], "auto");
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Review case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["true"],
            "finalVerify": [format!("if [ -f CHANGED_MARKER.txt ]; then echo ran >> '{}'; fi", final_runs.display())],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_until(&daemon, &task_id, |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    // No route read the attempt: it is committed, marked unreviewed, and the
    // owner is not asked to approve it.
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert!(settled["question"].is_null(), "task JSON: {settled}");
    let decisions = settled["decisions"].to_string();
    assert!(decisions.contains("committed unreviewed"), "{decisions}");
    // Flagged for the pull request, and never landed by itself.
    assert_eq!(settled["unreviewed"], true, "{settled}");
    assert!(!decisions.contains("Land:"), "{decisions}");
    let marked = settled["assumptions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["kind"] == "review_no_verdict" && a["by"] == "policy");
    assert!(marked, "the unreviewed commit is recorded: {settled}");
    assert_eq!(
        std::fs::read_to_string(&final_runs)
            .unwrap()
            .lines()
            .count(),
        1
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn the_advisor_runs_once_before_a_retry_and_its_advice_reaches_the_next_brief() {
    for (advisor, mode) in [(true, "ok"), (true, "fail"), (false, "ok")] {
        let scripts_dir = tempfile::tempdir().unwrap();
        let script =
            fake_harness_script(scripts_dir.path(), "fake-advisor.sh", FAKE_ADVISOR_SCRIPT);
        let daemon = Daemon::spawn(&[
            ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
            ("LOG_DIR", scripts_dir.path().to_str().unwrap()),
            ("ADVISOR_MODE", mode),
        ]);
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["review"] = serde_json::json!("");
        daemon.request("settings.set", serde_json::json!({"settings": settings}));

        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "Two tries",
                "goal": "Needs a second attempt",
                "verify": ["test -f SECOND"],
                "variant": {"advisor": advisor},
                "start": true,
            }),
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let settled = poll_task_status(&daemon, &task_id);
        let label = format!("advisor={advisor} mode={mode}: {settled}");
        assert_eq!(settled["status"], "done", "{label}");
        let attempts = settled["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 2, "{label}");
        let runs = std::fs::read_to_string(scripts_dir.path().join("advisor.log"))
            .map(|t| t.lines().count())
            .unwrap_or(0);
        let brief = std::fs::read_to_string(
            daemon
                .data_dir()
                .join("tasks")
                .join(&task_id)
                .join("runs/2/brief.md"),
        )
        .unwrap();
        assert!(
            brief.contains("## Failed checks") && !brief.contains("## Task"),
            "{brief}"
        );
        assert!(
            brief.contains("test -f SECOND") && brief.contains("exited 1."),
            "{brief}"
        );
        let cost = settled["costUsd"].as_f64().unwrap();
        if advisor && mode == "ok" {
            assert_eq!(runs, 1, "{label}");
            assert_eq!(
                attempts[0]["advice"], "Create SECOND, not FIRST.",
                "{label}"
            );
            assert!(attempts[1].get("advice").is_none(), "{label}");
            assert!(
                brief.contains("## Advisor") && brief.contains("Create SECOND, not FIRST."),
                "{brief}"
            );
            // Two implement runs plus the advisor; the advisor's cost is on
            // no attempt.
            assert!((cost - 0.05).abs() < 1e-9, "{label}");
            assert_eq!(attempts[0]["costUsd"], 0.01, "{label}");
        } else if advisor {
            assert_eq!(runs, 1, "{label}");
            assert!(attempts[0].get("advice").is_none(), "{label}");
            assert!(!brief.contains("## Advisor"), "{brief}");
        } else {
            assert_eq!(runs, 0, "{label}");
            assert!(!brief.contains("## Advisor"), "{brief}");
            assert!((cost - 0.02).abs() < 1e-9, "{label}");
        }

        let worktree = task["worktree"].as_str().unwrap().to_string();
        daemon.shutdown_and_wait();
        remove_dir_retrying(worktree);
    }
}

#[test]
fn the_reviewer_s_unmet_criterion_fails_the_attempt() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-contract.sh", FAKE_CONTRACT_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(1);
    // Review by the Claude hard route, which is the same fake.
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Contract",
            "goal": "Write the marker",
            "criteria": ["Marker exists -- check: CHANGED_MARKER.txt"],
            "verify": ["true"],
                "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "failed", "{settled}");
    let attempt = &settled["attempts"][0];
    assert_eq!(attempt["failure"]["kind"], "review", "{settled}");
    assert_eq!(
        attempt["review"]["findings"][0],
        "Unmet criterion: Marker exists -- check: CHANGED_MARKER.txt (wrong file)",
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn an_explicit_codex_review_route_sends_claude_work_to_codex() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let marker = scripts_dir.path().join("codex-reviewed");
    let codex = fake_harness_script(
        scripts_dir.path(),
        "fake-codex.sh",
        "#!/bin/sh\ncat > /dev/null\ntouch \"$CODEX_MARKER\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"```sushi-review\\n{\\\"verdict\\\":\\\"PASS\\\",\\\"findings\\\":[]}\\n```\"}}'\n",
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
        ("CODEX_MARKER", marker.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("codex");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Other family",
            "goal": "Write the marker",
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(
        settled["attempts"][0]["routeId"], "claude-sonnet",
        "{settled}"
    );
    assert!(marker.exists(), "the review never ran on Codex: {settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_repeated_review_finding_calls_the_advisor_and_tiers_up_then_the_second_is_handled() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-repeat.sh", FAKE_REPEAT_SCRIPT);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("LOG_DIR", scripts_dir.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(6);
    settings["answerPolicy"] = serde_json::json!(false);
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Repeat",
            "goal": "Write the marker",
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "failed", "{settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    let implements: Vec<&serde_json::Value> = attempts
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    // Attempt 1: a first finding. Attempt 2: the same finding, marked as a
    // repeat -> advisor and a tier up. Attempt 3: repeated a second time ->
    // handled, not the owner; the budget of 6 and one changed approach (two more) end the task.
    assert_eq!(implements.len(), 8, "{settled}");
    assert!(
        implements[0]["review"].get("repeated").is_none(),
        "{settled}"
    );
    assert_eq!(
        implements[1]["review"]["repeated"][0]
            .as_str()
            .unwrap()
            .split(" [F-")
            .next(),
        Some("P1: a.rs:1 - null check missing"),
        "{settled}"
    );
    assert_eq!(implements[0]["routeId"], "claude-sonnet", "{settled}");
    assert_eq!(implements[1]["routeId"], "claude-sonnet", "{settled}");
    assert_eq!(implements[2]["routeId"], "claude-opus", "{settled}");
    assert_eq!(implements[1]["advice"], "Fix the null check.", "{settled}");
    assert!(implements[0].get("advice").is_none(), "{settled}");
    let runs = std::fs::read_to_string(scripts_dir.path().join("advisor.log"))
        .map(|t| t.lines().count())
        .unwrap_or(0);
    // One for the repeated finding, one the ladder forces when it changes the approach.
    assert_eq!(runs, 2, "{settled}");
    let reviews: Vec<String> = std::fs::read_dir(scripts_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("review."))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert_eq!(reviews.len(), 8, "one review per attempt");
    assert_eq!(
        reviews
            .iter()
            .filter(|b| b.contains("## Previous review") && b.contains("null check missing"))
            .count(),
        7,
        "attempts 2 to 6 carry the previous findings"
    );
    let decisions = settled["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(|d| d
            .as_str()
            .unwrap()
            .contains("a review finding repeated -> advisor, tier hard")),
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
