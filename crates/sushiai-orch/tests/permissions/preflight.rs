use super::*;

#[test]
fn a_criterion_with_no_tool_to_enable_is_continued_without_a_question() {
    let scripts = tempfile::tempdir().unwrap();
    let gates = tempfile::tempdir().unwrap();
    let body = CHECK_SCRIPT.replace(r#"\"toolId\": \"asana\""#, r#"\"toolId\": null"#);
    assert_ne!(body, CHECK_SCRIPT);
    let script = fake_harness_script(scripts.path(), "fake-check.sh", &body);
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("HOME", home.path().to_str().unwrap()),
        ("GATES", gates.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["tiers"]["mechanical"] = json!("claude-sonnet");
    settings["briefCheckRoute"] = json!("claude-sonnet");
    settings["planReview"] = serde_json::json!("");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let created = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Move it",
            "goal": "Ship the change",
            "criteria": ["The code compiles", "The Asana task is moved to Done"],
            "verify": ["true"],
            "land": false,
            "start": true,
        }),
    );
    let id = created["id"].as_str().unwrap().to_string();
    let done = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert_eq!(done["status"], "done", "{done}");
    assert!(decisions(&done)
        .contains("no tool exists for move the Asana task; the task goes on without it"));
    daemon.shutdown_and_wait();
}

#[test]
fn the_brief_check_flags_a_criterion_the_task_has_no_tool_for_before_the_first_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let gates = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-check.sh", CHECK_SCRIPT);
    let home = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("HOME", home.path().to_str().unwrap()),
        ("GATES", gates.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["tiers"]["mechanical"] = json!("claude-sonnet");
    settings["briefCheckRoute"] = json!("claude-sonnet");
    settings["planReview"] = serde_json::json!("");
    settings["chatTools"] = json!([
        {"id": "asana", "label": "Asana", "enabled": false,
         "server": {"type": "http", "url": "https://example.invalid/mcp"}},
    ]);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let created = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Move it",
            "goal": "Ship the change",
            "criteria": ["The code compiles", "The Asana task is moved to Done"],
            "verify": ["true"],
            "land": false,
            "start": true,
        }),
    );
    let id = created["id"].as_str().unwrap().to_string();
    let waiting = poll_until(&daemon, &id, |s| s == "waiting");

    let calls = std::fs::read_to_string(gates.path().join("calls")).unwrap();
    assert_eq!(
        calls.trim(),
        "check",
        "no attempt before the answer: {calls}"
    );
    assert!(waiting["attempts"].as_array().unwrap().is_empty());
    let prompt = std::fs::read_to_string(gates.path().join("check_prompt")).unwrap();
    assert!(prompt.contains("connectedButOffForThisTask"), "{prompt}");
    let question = &waiting["question"];
    assert_eq!(question["kind"], "permission");
    assert!(question["text"]
        .as_str()
        .unwrap()
        .contains("move the Asana task"));
    assert_eq!(
        question["options"],
        json!([
            "Enable asana for this task",
            "I will do it: drop the criterion",
            "Continue anyway"
        ])
    );

    daemon.request(
        "task.answer",
        json!({"id": id, "answer": "I will do it: drop the criterion"}),
    );
    let done = poll_until(&daemon, &id, |s| s == "done");
    assert_eq!(done["criteria"], json!(["The code compiles"]));
    assert!(decisions(&done).contains("it is an owner action, not this task's"));
    let calls = std::fs::read_to_string(gates.path().join("calls")).unwrap();
    assert_eq!(calls.lines().filter(|c| *c == "check").count(), 1);
    daemon.shutdown_and_wait();
}
