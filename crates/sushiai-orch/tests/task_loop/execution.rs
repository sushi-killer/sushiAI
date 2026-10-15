use super::*;

#[test]
fn red_tool_results_from_different_commands_do_not_stop_the_runner() {
    for (harness, bin_env, stream) in [
        (
            "claude",
            "ORCHD_CLAUDE_BIN",
            include_str!("../fixtures/loop_detect/claude-identical-red-results.jsonl"),
        ),
        (
            "codex",
            "ORCHD_CODEX_BIN",
            include_str!("../fixtures/loop_detect/codex-identical-red-results.jsonl"),
        ),
    ] {
        let scripts = tempfile::tempdir().unwrap();
        let stream = stream
            .lines()
            .filter(|line| {
                let event: serde_json::Value = serde_json::from_str(line).unwrap();
                !matches!(event["type"].as_str(), Some("result" | "turn.completed"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let report = "```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```";
        let result = if harness == "claude" {
            serde_json::json!({"type": "result", "result": report})
        } else {
            serde_json::json!({"type": "item.completed", "item": {"type": "agent_message", "text": report}})
        };
        let completed = if harness == "codex" {
            "{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}\n"
        } else {
            ""
        };
        let script = fake_harness_script(
            scripts.path(),
            "red-results.sh",
            &format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'synthetic 1'; exit 0; fi\ncat > /dev/null\ncat <<'STREAM'\n{stream}\n{result}\n{completed}STREAM\necho changed > CHANGED_MARKER.txt\n"),
        );
        let daemon = Daemon::spawn(&[(bin_env, script.to_str().unwrap())]);
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["review"] = serde_json::json!("");
        settings["routes"].as_array_mut().unwrap().push(
            serde_json::json!({"id": "stream-test", "label": "Stream test", "harness": harness}),
        );
        settings["tiers"] = serde_json::json!({"mechanical": "stream-test", "standard": "stream-test", "hard": "stream-test"});
        daemon.request("settings.set", serde_json::json!({"settings": settings}));
        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path(), "title": "Stream recovery", "goal": "Finish after red results",
                "verify": ["test -f CHANGED_MARKER.txt"],
                "variant": {"maxAttempts": 1, "loopDetect": true}, "land": false,
            }),
        );
        let settled = poll_task_status(&daemon, task["id"].as_str().unwrap());
        assert_eq!(settled["status"], "done", "{harness}: {settled}");
        assert_eq!(
            settled["attempts"].as_array().unwrap().len(),
            1,
            "{settled}"
        );
        daemon.shutdown_and_wait();
        remove_dir_retrying(task["worktree"].as_str().unwrap());
    }
}

#[test]
fn engine_loop_passes_when_verify_succeeds() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"changed\" > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );

    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    // This test only exercises the verify gate, not review; turn review off
    // so it never shells out to a real `codex`/`claude` binary that might
    // happen to be on this machine's PATH.
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Pass case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    // No `task.start`: the full form starts by default, like the request form.

    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(settled["attempts"][0]["status"], "passed");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn project_secrets_reach_the_agent_but_not_settings_logs_or_task_mcp() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("observed-env");
    let captured_mcp = scripts_dir.path().join("observed-mcp.json");
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &format!("#!/bin/sh\nfor arg in \"$@\"; do\n  [ \"$prev\" = --mcp-config ] && cp \"$arg\" '{}'\n  prev=\"$arg\"\ndone\nprintf '%s|%s' \"$PROJECT_TOKEN\" \"${{SETUP_ONLY-unset}}\" > '{}'\necho \"observed $PROJECT_TOKEN\"\necho changed > CHANGED_MARKER.txt\ncat > /dev/null\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-project\"}}'\necho '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"done\"}}'\n", captured_mcp.display(), marker.display()),
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    daemon.request(
        "secrets.set",
        serde_json::json!({
            "projects": {"project-test": {"PROJECT_TOKEN": "invented-project-secret-42"}},
            "projectRepos": {repo.path().to_str().unwrap(): "project-test"}
        }),
    );
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "projectId": "project-test",
            "title": "Use project environment",
            "goal": "Verify project environment delivery",
            "criteria": [],
            "verify": ["true"],
            "mcp": {"mcpServers": {"example": {"command": "server", "env": {"TOKEN": "${PROJECT_TOKEN}"}}}}
        }),
    );
    let task_id = task["id"].as_str().unwrap();
    let settled = poll_task_status(&daemon, task_id);
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(
        std::fs::read_to_string(marker).unwrap(),
        "invented-project-secret-42|unset"
    );
    let task_dir = daemon.data_dir().join("tasks").join(task_id);
    let task_mcp = std::fs::read_to_string(task_dir.join("mcp.json")).unwrap();
    assert!(task_mcp.contains("${PROJECT_TOKEN}"));
    assert!(!task_mcp.contains("invented-project-secret-42"));
    let run_mcp = std::fs::read_to_string(captured_mcp).unwrap();
    assert!(run_mcp.contains("invented-project-secret-42"));
    assert!(!task_dir.join("runs/1/mcp.json").exists());
    let events = std::fs::read_to_string(task_dir.join("runs/1/events.jsonl")).unwrap();
    assert!(!events.contains("invented-project-secret-42"));
    assert!(events.contains("[REDACTED]"));
    let settings = std::fs::read_to_string(task_dir.join("runs/1/settings.json")).unwrap();
    assert!(!settings.contains("invented-project-secret-42"));
}

#[test]
fn engine_loop_waits_after_verify_keeps_failing_and_attempts_are_exhausted() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"changed\" > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );

    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(2);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Fail case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["false"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    // maxAttempts=2 is spent, the ladder changes the approach once (two more
    // attempts), then the task fails: nobody is asked.
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "failed", "task JSON: {settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(
        attempts.len(),
        3,
        "maxAttempts=2, then one changed approach"
    );
    for a in attempts {
        assert_eq!(a["status"], "failed");
        assert_eq!(a["failure"]["kind"], "verify");
    }
    assert!(settled["question"].is_null(), "{settled}");
    let decisions = settled["decisions"].to_string();
    assert!(
        decisions.contains("Ladder: changed approach"),
        "{decisions}"
    );
    assert!(decisions.contains("Ladder: stopped after"), "{decisions}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_task_waiting_for_its_owner_does_not_hold_a_parallel_slot() {
    let scripts_dir = tempfile::tempdir().unwrap();
    // One task asks the owner a question (its brief names it); the other passes.
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        r##"#!/bin/sh
brief=$(cat)
echo changed > CHANGED_MARKER.txt
echo '{"type":"system","subtype":"init","session_id":"sess-fake"}'
case "$brief" in
*"Always asks"*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"blocked\",\"summary\":\"stuck\",\"decisions\":[],\"question\":\"Which one should it be?\"}\n```"}' ;;
*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"##,
    );
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let home = data_holder.path().to_path_buf();
    // `parallel` is read at startup: save it with a first daemon, then
    // restart onto the saved settings in the same data dir.
    let socket1 = socket_of(&home);
    let first = spawn_daemon(&home, &fake_bins);
    wait_for_socket(&socket1);
    let mut settings = request_on(&socket1, "settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(1);
    settings["parallel"] = serde_json::json!(1);
    settings["review"] = serde_json::json!("");
    fit_sandbox(&mut settings);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["planReview"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    request_on(
        &socket1,
        "settings.set",
        serde_json::json!({"settings": settings}),
    );
    stop_daemon(&socket1);
    let _ = wait_for_exit(first);

    let socket = socket_of(&home);
    let child = spawn_daemon(&home, &fake_bins);
    wait_for_socket(&socket);
    let call = |m: &str, p: serde_json::Value| request_on(&socket, m, p);

    let repo = init_git_repo();
    let waiting = call(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Always asks", "goal": "g", "criteria": [], "verify": ["true"], "start": true}),
    );
    let waiting_id = waiting["id"].as_str().unwrap().to_string();
    wait_until("the first task to reach waiting", || {
        (call("task.get", serde_json::json!({"id": waiting_id}))["status"] == "waiting")
            .then_some(())
    });

    let passing = call(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Passes", "goal": "g", "criteria": [], "verify": ["true"], "start": true}),
    );
    let passing_id = passing["id"].as_str().unwrap().to_string();
    wait_until("the second task to finish, not starve", || {
        (call("task.get", serde_json::json!({"id": passing_id}))["status"] == "done").then_some(())
    });
    stop_daemon(&socket);
    let _ = wait_for_exit(child);
    for t in [&waiting, &passing] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn maxattempts_one_still_gives_the_implement_stage_its_own_one_attempt() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-planner-failing-verify.sh",
        FAILING_VERIFY_PLANNER_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["maxAttempts"] = serde_json::json!(1);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    // If the plan attempt wrongly counted against maxAttempts, the implement
    // stage would already be "exhausted" before it ever got to try -- this
    // proves it actually got its own single attempt instead.
    let settled = poll_until(&daemon, &task_id, |s| s == "failed");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts[0]["stage"], "plan");
    assert_eq!(attempts[1]["stage"], "implement");
    assert_eq!(attempts[1]["failure"]["kind"], "verify");
    // The plan attempt did not count: exactly one implement attempt came
    // before the ladder changed the approach (one more budget of two).
    let implements = attempts
        .iter()
        .filter(|a| a["stage"] == "implement")
        .count();
    assert_eq!(implements, 3, "{settled}");
    assert!(settled["question"].is_null(), "{settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_task_is_carried_onto_its_base_when_the_base_moves_mid_attempt() {
    // The fake agent commits to the base branch of the main checkout while
    // it works, as another merged task would.
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\nroot=\"$(git rev-parse --git-common-dir)/..\"\ngit -C \"$root\" -c user.email=t@example.com -c user.name=t commit -q --allow-empty -m 'base moves'\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Carry case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["test -f CHANGED_MARKER.txt"],
        }),
    );
    assert!(task["baseRef"].is_string(), "task JSON: {task}");
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    assert_eq!(settled["baseSha"], head.as_str(), "task JSON: {settled}");
    assert!(
        settled["decisions"]
            .to_string()
            .contains("carried the work onto"),
        "task JSON: {settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
