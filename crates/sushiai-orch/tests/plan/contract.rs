use super::*;

#[test]
fn a_planned_task_routes_by_the_planners_tier() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
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

    let settled = poll_until(&daemon, &task_id, |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["plannedTier"], "hard", "{settled}");
    let decisions = settled["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(|d| d
            .as_str()
            .unwrap_or("")
            .starts_with("Planner: tier hard -> route ")),
        "expected a planner tier decision: {decisions:?}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn the_plan_brief_asks_for_a_contract() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
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
    poll_until(&daemon, &task_id, |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    let brief = std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(&task_id)
            .join("runs/1/plan/brief.md"),
    )
    .unwrap();
    assert!(brief.contains("-- check:"), "{brief}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_visual_flag_on_a_command_checked_criterion_does_not_demand_an_image() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-planner.sh",
        VISUAL_COMMAND_CRITERION_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "make the parser handle X",
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_until(&daemon, &task_id, |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    let implement: Vec<_> = attempts
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert_eq!(implement.len(), 1, "{settled}");
    assert_eq!(implement[0]["n"], 2, "{settled}");
    assert!(implement[0]["failure"].is_null(), "{settled}");
    assert!(
        attempts.iter().all(|a| a["failure"]["kind"] != "evidence"),
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_planned_screenshot_command_reaches_the_task_and_the_plan_brief_asks_for_it() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-planner.sh",
        SCREENSHOT_PLAN_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    std::fs::write(repo.path().join(".gitignore"), "artifacts/\n").unwrap();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "change the panel",
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_until(&daemon, &task_id, |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    assert_eq!(
        settled["briefCheck"]["screenshot"], "mkdir -p artifacts && echo png > artifacts/panel.png",
        "{settled}"
    );
    // The command ran: the image is the attempt's evidence.
    assert_eq!(settled["status"], "done", "{settled}");
    let brief = std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(&task_id)
            .join("runs/1/plan/brief.md"),
    )
    .unwrap();
    assert!(brief.contains("\"screenshot\":\"\""), "{brief}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
