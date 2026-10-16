use super::*;

#[test]
fn a_silent_harness_is_stopped_as_stalled_when_the_variant_sets_a_stall_timeout() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude-silent.sh",
        &format!(
            "#!/bin/sh\nbrief=$(cat)\ncase \"$brief\" in *'An implement attempt at this task failed'*) exit 0;; esac\necho $$ > \"$PID_FILE\"\nexec sleep {}\n",
            deadline().as_secs() * 3
        ),
    );
    let pid_file = scripts_dir.path().join("harness.pid");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PID_FILE", pid_file.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(1);
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Hangs",
            "goal": "Never answers",
            "verify": ["true"],
            "variant": {"stallTimeoutSecs": 8},
            "start": true,
        }),
    );
    assert_eq!(task["variant"]["stallTimeoutSecs"], 8, "{task}");
    let task_id = task["id"].as_str().unwrap().to_string();

    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "failed", "task JSON: {settled}");
    assert_eq!(
        settled["attempts"][0]["failure"]["kind"], "stall",
        "{settled}"
    );
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    wait_until(&format!("the stalled harness {pid} to stop"), || {
        (!is_alive(pid)).then_some(())
    });

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn the_stall_clock_waits_while_the_stop_hook_runs_verify() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude-hook.js",
        FAKE_CLAUDE_HOOK_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    // The hook's verify is silent for 12s, well past the 5s stall timeout;
    // 5s leaves room for a loaded machine to start the fake harness.
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Slow hook",
            "goal": "Make a trivial change",
            "verify": ["sleep 12 && test -f verified-marker.txt"],
            "variant": {"stallTimeoutSecs": 5},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(
        settled["attempts"].as_array().unwrap().len(),
        1,
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

/// The prompt-prefix trim (`--disallowedTools`) used to be gated behind
/// `variant.leanContext`; it is now unconditional for every Claude implement
/// run, and mcp.json carries the configured task servers. No skills hook is
/// wired into `settings.json`.
#[test]
fn a_claude_implement_run_trims_delegation_tools_and_keeps_the_configured_mcp_servers() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let captured_mcp = scripts_dir.path().join("observed-mcp.json");
    let body = FAKE_CLAUDE_PASS.replace(
        "#!/bin/sh\n",
        &format!("#!/bin/sh\nfor arg in \"$@\"; do\n  [ \"$prev\" = --mcp-config ] && cp \"$arg\" '{}'\n  prev=\"$arg\"\ndone\n", captured_mcp.display()),
    );
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &body);
    let args = scripts_dir.path().join("args");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("CLAUDE_ARGS", args.to_str().unwrap()),
    ]);
    // Skip review: FAKE_CLAUDE_PASS's report has no PASS/FAIL verdict, and
    // this test only cares about the implement attempt's argv/settings/mcp.
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Trimmed prefix",
            "goal": "Change the widget",
            "verify": ["true"],
            "mcp": {"mcpServers": {"relevant": {"command": "r"}, "irrelevant": {"command": "i"}}},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "done", "{settled}");

    let argv = std::fs::read_to_string(&args).unwrap();
    assert!(argv.contains("--disallowedTools"), "{argv}");
    let settings: serde_json::Value =
        serde_json::from_str(&run_file(&daemon, &task_id, "settings.json")).unwrap();
    // Sorted: key order follows serde_json's `preserve_order`, which another workspace crate enables.
    let mut hooks: Vec<&String> = settings["hooks"].as_object().unwrap().keys().collect();
    hooks.sort();
    assert_eq!(hooks, ["PreToolUse", "Stop"], "{settings}");
    let mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(captured_mcp).unwrap()).unwrap();
    assert!(!daemon
        .data_dir()
        .join("tasks")
        .join(&task_id)
        .join("runs/1/mcp.json")
        .exists());
    assert_eq!(
        mcp,
        serde_json::json!({"mcpServers": {
            "relevant": {"command": "r"},
            "irrelevant": {"command": "i"},
            "sushiai-messages": mcp["mcpServers"]["sushiai-messages"].clone(),
        }})
    );
    assert!(mcp["mcpServers"]["sushiai-messages"].is_object());

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_looping_harness_is_stopped_and_a_second_loop_escalates_the_tier() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude-loop.sh",
        FAKE_LOOPING_CLAUDE,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(3);
    settings["review"] = serde_json::json!("");
    assert_eq!(settings["experiments"]["loopDetect"], true, "{settings}");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Loops",
            "goal": "Repeat yourself",
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "failed", "task JSON: {settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(
        attempts.len(),
        4,
        "three, then a changed approach: {settled}"
    );
    for a in attempts {
        assert_eq!(a["failure"]["kind"], "loop", "{settled}");
    }
    let detail = attempts[0]["failure"]["detail"].as_str().unwrap();
    assert!(
        detail.contains("repeated_call") && detail.contains("echo alpha"),
        "{detail}"
    );
    let second = attempts[1]["failure"]["detail"].as_str().unwrap();
    assert!(second.contains("echo beta"), "{second}");

    let brief = std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(&task_id)
            .join("runs/2/brief.md"),
    )
    .unwrap();
    assert!(
        brief.contains("stopped as a loop") && brief.contains("echo alpha"),
        "{brief}"
    );
    // The second loop moved the task up a tier; a third attempt ran on it.
    assert_eq!(settled["tier"], "hard", "{settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn an_attempt_stores_the_model_and_harness_version_the_run_reported() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\",\"model\":\"claude-sonnet-5-5\",\"claude_code_version\":\"2.1.284\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"modelUsage\":{\"claude-sonnet-5-5\":{},\"claude-haiku-4-5-20251001\":{}},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
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
            "title": "Fingerprint",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let fp = &settled["attempts"][0]["fingerprint"];
    assert_eq!(
        fp["models"],
        serde_json::json!(["claude-sonnet-5-5", "claude-haiku-4-5-20251001"]),
        "{fp}"
    );
    assert_eq!(fp["harness"], "claude", "{fp}");
    assert_eq!(fp["harnessVersion"], "2.1.284", "{fp}");
    assert_eq!(fp["promptHash"].as_str().map(str::len), Some(8), "{fp}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
