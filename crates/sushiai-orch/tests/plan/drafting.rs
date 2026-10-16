use super::*;

#[test]
fn drafting_task_asks_its_planner_question_then_implements_and_passes() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PLAN_ASK_QUESTION", "1"),
    ]);

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
            "variant": {"batchQuestions": false},
            "start": true,
        }),
    );
    assert_eq!(task["status"], "drafting");
    let task_id = task["id"].as_str().unwrap().to_string();

    let waiting = poll_until(&daemon, &task_id, |s| s == "waiting");
    assert_eq!(waiting["question"]["text"], "Which default theme?");
    assert_eq!(
        waiting["question"]["options"],
        serde_json::json!(["light", "dark"])
    );
    // The plan attempt is recorded but must never count against maxAttempts
    // or show up as an implement attempt.
    assert_eq!(waiting["attempts"][0]["stage"], "plan");

    daemon.request(
        "task.answer",
        serde_json::json!({"id": task_id, "answer": "dark"}),
    );

    let settled = poll_until(&daemon, &task_id, |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["title"], "Add dark mode");
    assert_eq!(
        settled["criteria"],
        serde_json::json!(["Toggle visible in settings"])
    );
    assert!(settled["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d.as_str().unwrap().contains("Which default theme? -> dark")));
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "one plan attempt, one implement attempt");
    assert_eq!(attempts[0]["stage"], "plan");
    assert_eq!(attempts[1]["stage"], "implement");
    assert_eq!(attempts[0]["n"], 1);
    assert_eq!(attempts[1]["n"], 2);

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn drafting_task_with_no_questions_goes_straight_to_done() {
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
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["title"], "Add dark mode");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn drafting_task_created_with_start_false_stops_after_planning_for_review() {
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
            "start": false,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    // Planning runs regardless of `start: false` -- only whether it then
    // proceeds into implementing depends on it.
    let settled = poll_until(&daemon, &task_id, |s| {
        s == "stopped" || s == "done" || s == "failed"
    });
    assert_eq!(settled["status"], "stopped", "task JSON: {settled}");
    assert_eq!(settled["title"], "Add dark mode");
    assert_eq!(
        settled["criteria"],
        serde_json::json!(["Toggle visible in settings"])
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_task_waiting_on_its_plan_question_does_not_hold_a_parallel_slot() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let fake_bins = [
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PLAN_ASK_QUESTION", "1"),
    ];
    let data_holder = tempfile::tempdir().unwrap();
    let home = data_holder.path().to_path_buf();
    // `parallel` is read at startup: save it with a first daemon, then
    // restart onto the saved settings in the same data dir.
    let socket1 = socket_of(&home);
    let first = spawn_daemon(&home, &fake_bins);
    wait_for_socket(&socket1);
    let mut settings = request_on(&socket1, "settings.get", serde_json::json!({}));
    settings["parallel"] = serde_json::json!(1);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["planReview"] = serde_json::json!("");
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    fit_sandbox(&mut settings);
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
    let drafting = call(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "request": "add dark mode", "start": true}),
    );
    let drafting_id = drafting["id"].as_str().unwrap().to_string();
    wait_until("the drafting task to wait on its plan question", || {
        (call("task.get", serde_json::json!({"id": drafting_id}))["status"] == "waiting")
            .then_some(())
    });

    let passing = call(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Passes", "goal": "g", "criteria": [], "verify": ["true"], "start": true}),
    );
    let passing_id = passing["id"].as_str().unwrap().to_string();
    wait_until(
        "the second task to finish while the first sits on a plan question",
        || {
            (call("task.get", serde_json::json!({"id": passing_id}))["status"] == "done")
                .then_some(())
        },
    );
    stop_daemon(&socket);
    let _ = wait_for_exit(child);
    for t in [&drafting, &passing] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn stop_then_start_while_drafting_replans_from_scratch() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-planner-garbage.sh",
        GARBAGE_PLANNER_SCRIPT,
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
            "request": "add dark mode to the settings screen",
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    let first_wait = poll_until(&daemon, &task_id, |s| s == "waiting");
    assert!(first_wait["question"]["text"]
        .as_str()
        .unwrap()
        .contains("could not draft"));
    let plan_attempts_before = first_wait["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "plan")
        .count();
    assert_eq!(plan_attempts_before, 1);

    daemon.request("task.stop", serde_json::json!({"id": task_id}));
    let stopped = poll_until(&daemon, &task_id, |s| s == "stopped" || s == "failed");
    assert_eq!(stopped["status"], "stopped", "task JSON: {stopped}");

    daemon.request("task.start", serde_json::json!({"id": task_id}));
    let second_wait = poll_until(&daemon, &task_id, |s| s == "waiting");
    let plan_attempts_after = second_wait["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "plan")
        .count();
    assert_eq!(
        plan_attempts_after, 2,
        "task.start on a stopped, never-passed draft should re-run the plan stage from scratch: {second_wait}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn unparseable_plan_then_owner_clarification_leads_to_a_successful_replan() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-planner-clarify.sh",
        CLARIFY_THEN_SUCCEED_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["planner"] = serde_json::json!("claude-opus");
    settings["tiers"]["standard"] = serde_json::json!("claude-sonnet");
    settings["smallPlanMaxChars"] = serde_json::json!(40);
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

    let waiting = poll_until(&daemon, &task_id, |s| s == "waiting");
    assert!(waiting["question"]["text"]
        .as_str()
        .unwrap()
        .contains("could not draft"));

    daemon.request(
        "task.answer",
        serde_json::json!({"id": task_id, "answer": "it's the settings screen's theme picker"}),
    );

    let settled = poll_until(&daemon, &task_id, |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["title"], "Add dark mode");
    let plan_attempts = settled["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "plan")
        .count();
    assert_eq!(
        plan_attempts, 2,
        "the first (unparseable) round and the successful replan: {settled}"
    );

    assert!(settled["request"].as_str().unwrap().chars().count() > 40);
    assert!(
        settled["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["stage"] == "plan")
            .all(|a| a["routeId"] == "claude-sonnet"),
        "{settled}"
    );
    let route_lines: Vec<_> = settled["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d.as_str().unwrap().starts_with("Plan route:"))
        .collect();
    assert_eq!(
        route_lines,
        vec![&serde_json::json!(
            "Plan route: claude-sonnet (small request: 36 chars <= 40, 0 paths <= 3, no list)"
        )]
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
