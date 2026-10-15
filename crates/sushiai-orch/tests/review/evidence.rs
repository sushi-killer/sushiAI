use super::*;

#[test]
fn review_evidence_attaches_the_attempt_s_screenshots_to_the_codex_reviewer() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nmkdir -p artifacts\necho png > artifacts/after.png\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let args_log = scripts_dir.path().join("codex-args");
    let codex = fake_harness_script(
        scripts_dir.path(),
        "fake-codex.sh",
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$CODEX_ARGS\"\ncat > \"$CODEX_ARGS.brief\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"```sushi-review\\n{\\\"verdict\\\":\\\"PASS\\\",\\\"findings\\\":[]}\\n```\"}}'\n",
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
        ("CODEX_ARGS", args_log.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("codex");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    std::fs::write(repo.path().join(".gitignore"), "artifacts/\n").unwrap();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Evidence",
            "goal": "Save a screenshot",
            "criteria": ["the panel shows the change -- check: screenshot artifacts/after.png"],
            "verify": ["true"],
            "variant": {"reviewEvidence": true},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "done", "{settled}");
    let argv = std::fs::read_to_string(&args_log).unwrap();
    assert!(
        argv.starts_with("exec --image=") && argv.contains("artifacts/after.png"),
        "{argv}"
    );
    let brief = std::fs::read_to_string(format!("{}.brief", args_log.display())).unwrap();
    assert!(
        brief.contains("## Screenshots") && brief.contains("evidence/after.png`"),
        "{brief}"
    );
    assert!(
        brief.contains("The implementer's own account") && brief.contains("<untrusted-data>\ndone"),
        "the reviewer hears the implementer's summary: {brief}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_visual_criterion_needs_a_saved_image_and_the_copies_outlive_the_worktree() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-shot.sh", FAKE_SCREENSHOT_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_by_claude(&daemon);
    let repo = init_git_repo();
    std::fs::write(repo.path().join(".gitignore"), "artifacts/\n").unwrap();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Visual",
            "goal": "Change the panel",
            // A command that runs but leaves no image: the gate still holds.
            "screenshot": "true",
            "criteria": ["The panel shows thumbnails -- check: screenshot under artifacts/"],
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "done", "{settled}");

    let attempts = settled["attempts"].as_array().unwrap();
    let implement: Vec<_> = attempts
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert_eq!(implement[0]["failure"]["kind"], "evidence", "{settled}");
    let feedback = implement[0]["failure"]["detail"].as_str().unwrap();
    assert!(
        feedback.contains("The panel shows thumbnails")
            && feedback.contains("under artifacts/")
            && feedback.contains("screenshot command `true` ran"),
        "{feedback}"
    );
    assert!(implement[0].get("evidence").is_none(), "{settled}");
    assert_eq!(
        implement[1]["failure"],
        serde_json::Value::Null,
        "{settled}"
    );

    // Gone with the worktree, still listed and still on disk. A test action
    // while the (idle) daemon runs, not cleanup.
    let worktree = task["worktree"].as_str().unwrap().to_string();
    remove_dir_retrying(&worktree);
    let got = daemon.request("task.get", serde_json::json!({"id": task_id}));
    let listed: Vec<&str> = got["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["evidence"].as_array().into_iter().flatten())
        .filter_map(|e| e.as_str())
        .collect();
    assert_eq!(listed.len(), 1, "{got}");
    assert!(listed[0].contains("/runs/2/evidence/"), "{listed:?}");
    assert_eq!(std::fs::read_to_string(listed[0]).unwrap().trim(), "png");
    // "png\n" as a data URL.
    let url = daemon.request(
        "task.evidence",
        serde_json::json!({"id": task_id, "path": listed[0]}),
    );
    assert_eq!(url["dataUrl"], "data:image/png;base64,cG5nCg==", "{url}");

    daemon.shutdown_and_wait();
}

#[test]
fn a_retry_that_changed_only_tests_reuses_the_accepted_evidence_helpers_folder() {
    let (daemon, settled, task_id) = run_retry_task("tests/panel.test.txt");
    assert_eq!(settled["status"], "done", "{settled}");
    let implement: Vec<_> = settled["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert_eq!(implement.len(), 2, "{settled}");
    assert!(implement[1].get("failure").is_none(), "{settled}");
    let paths: Vec<&str> = implement[1]["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e.as_str())
        .collect();
    assert_eq!(paths.len(), 1, "{settled}");
    assert!(paths[0].contains("/runs/1/evidence/"), "{paths:?}");
    assert_eq!(implement[1]["evidenceFrom"], 1, "{settled}");
    assert!(implement[0].get("evidenceFrom").is_none(), "{settled}");
    let brief = std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(&task_id)
            .join("helpers/review-2/brief.md"),
    )
    .unwrap();
    assert!(brief.contains(paths[0]), "{brief}");
    assert!(brief.contains("(attempt 1)"), "{brief}");
    let decisions: Vec<&str> = settled["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect();
    assert!(
        decisions
            .iter()
            .any(|d| d.contains("attempt 2") && d.contains("attempt 1")),
        "{decisions:?}"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn a_retry_that_changed_the_same_ui_file_still_needs_new_evidence() {
    let (daemon, settled, _task_id) = run_retry_task("src/components/panel.tsx");
    let implement: Vec<_> = settled["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert!(implement.len() >= 2, "{settled}");
    assert_eq!(implement[1]["failure"]["kind"], "evidence", "{settled}");
    let detail = implement[1]["failure"]["detail"].as_str().unwrap();
    assert!(detail.contains("src/components/panel.tsx"), "{detail}");
    assert!(implement[1].get("evidenceFrom").is_none(), "{settled}");
    daemon.shutdown_and_wait();
}

#[test]
fn a_retry_that_changed_other_source_does_not_reuse_the_earlier_images() {
    let (daemon, settled, _task_id) = run_retry_task("src/other.txt");
    // Any non-test change since the earlier screenshots reruns the screenshot
    // command; the fake saves no new image, so the attempt fails on evidence.
    assert_eq!(settled["status"], "failed", "{settled}");
    let implement: Vec<_> = settled["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert!(implement.len() >= 2, "{settled}");
    assert!(implement[1].get("evidenceFrom").is_none(), "{settled}");
    assert!(
        implement[1]["evidence"]
            .as_array()
            .is_none_or(|e| e.is_empty()),
        "{settled}"
    );
    assert_eq!(implement[1]["failure"]["kind"], "evidence", "{settled}");
    let detail = implement[1]["failure"]["detail"].as_str().unwrap();
    assert!(
        detail.contains(
            "Attempt 1's images were not reused because these files changed since: src/other.txt"
        ),
        "{detail}"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn a_gif_or_svg_saved_for_a_visual_criterion_counts_as_evidence() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let body = FAKE_SCREENSHOT_SCRIPT.replace("panel.png", "panel.svg");
    let script = fake_harness_script(scripts_dir.path(), "fake-svg.sh", &body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_by_claude(&daemon);
    let repo = init_git_repo();
    std::fs::write(repo.path().join(".gitignore"), "artifacts/\n").unwrap();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Visual svg",
            "goal": "Change the panel",
            "screenshot": "true",
            "criteria": ["Shows a chart -- check: svg image under artifacts/"],
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "done", "{settled}");
    let listed: Vec<String> = settled["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["evidence"].as_array().into_iter().flatten())
        .filter_map(|e| e.as_str().map(String::from))
        .collect();
    assert_eq!(listed.len(), 1, "{settled}");
    assert!(listed[0].ends_with("panel.svg"), "{listed:?}");
    let url = daemon.request(
        "task.evidence",
        serde_json::json!({"id": task_id, "path": listed[0]}),
    );
    assert!(
        url["dataUrl"]
            .as_str()
            .unwrap()
            .starts_with("data:image/svg+xml;base64,"),
        "{url}"
    );
    daemon.shutdown_and_wait();
}
