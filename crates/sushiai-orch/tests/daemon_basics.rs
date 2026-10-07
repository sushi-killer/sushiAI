//! Black-box integration tests (daemon basics): spawn the real `sushiai daemon` with the
//! orchestrator module enabled and drive it over its socket.

mod common;

use common::*;
use std::path::Path;
use std::time::{Duration, Instant};

#[test]
fn settings_task_create_and_list_round_trip() {
    let daemon = Daemon::spawn(&[]);

    let settings = daemon.request("settings.get", serde_json::json!({}));
    assert_eq!(settings["maxAttempts"], 4);
    assert_eq!(settings["tiers"]["standard"], "claude-sonnet");

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Add a widget",
            "goal": "Add a widget to the page",
            "criteria": ["widget visible"],
            "verify": ["true"],
        }),
    );
    assert_eq!(task["status"], "queued");
    assert_eq!(task["branch"], "task/add-a-widget");
    let worktree = task["worktree"].as_str().unwrap();
    assert!(
        Path::new(worktree).is_dir(),
        "worktree directory should exist"
    );

    let list = daemon.request("task.list", serde_json::json!({}));
    let tasks = list.as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], task["id"]);

    let get = daemon.request("task.get", serde_json::json!({"id": task["id"]}));
    assert_eq!(get["title"], "Add a widget");

    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn settings_defaults_reports_the_built_in_defaults_not_the_saved_settings() {
    let daemon = Daemon::spawn(&[]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["planner"] = serde_json::json!("claude-sonnet");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let defaults = daemon.request("settings.defaults", serde_json::json!({}));
    assert_eq!(defaults["planner"], "claude-opus");
    let saved = daemon.request("settings.get", serde_json::json!({}));
    assert_eq!(saved["planner"], "claude-sonnet");

    daemon.shutdown_and_wait();
}

/// Regression test for the bug where `App::shutdown()` and `main.rs`'s
/// drain loop only ever looked at `controls` (task attempts), never
/// `chat_turns`: a `shutdown` mid-conversation left the orchestrator chat's
/// `setsid`'d harness child running forever, orphaned, with the daemon gone
/// and nothing left to kill it.
#[test]
fn shutdown_kills_a_live_orchestrator_chat_turn_s_child() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("chat-pid");
    // Records its own pid (== the pid tokio spawned, since a shebang script
    // is exec'd in place, never forked again) then hangs without ever
    // printing a `result` line -- exactly what a slow chat turn looks like
    // from orchd's side. The rename keeps the test from reading the marker
    // between its creation and the pid landing in it.
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\necho $$ > {m}.tmp\nmv {m}.tmp {m}\nsleep 30\n",
        m = marker.display()
    );
    let script = fake_harness_script(scripts_dir.path(), "fake-claude-chat.sh", &body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    // The connected tools are on by default and probing an http one runs the
    // fake harness too; this test is about the turn's own child.
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["chatTools"] = serde_json::json!([]);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = tempfile::tempdir().unwrap();
    daemon.request(
        "chat.send",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "text": "hello"}),
    );

    let start = Instant::now();
    while !marker.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "chat harness never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let pid: i32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .expect("marker should contain the harness pid");
    assert!(
        is_alive(pid),
        "the fake harness should still be running before shutdown"
    );

    // Same call the acceptance criteria names: `shutdown` mid-conversation.
    daemon.shutdown_and_wait();

    assert!(
        !is_alive(pid),
        "chat turn's child (pid {pid}) leaked past the daemon's own shutdown"
    );
}

#[test]
fn task_create_rejects_request_form_when_the_planner_is_disabled() {
    let daemon = Daemon::spawn(&[]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["planner"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let result = raw_request_with_params(
        &daemon.socket,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "request": "add a widget"}),
    );
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("planner is disabled"),
        "{result}"
    );

    daemon.shutdown_and_wait();
}

#[test]
fn task_create_rejects_neither_request_nor_title_and_goal() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let result = raw_request_with_params(
        &daemon.socket,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap()}),
    );
    assert!(result.get("error").is_some(), "{result}");

    daemon.shutdown_and_wait();
}

#[test]
fn an_old_settings_json_with_a_classifier_still_loads() {
    let home = tempfile::tempdir().unwrap();
    let data_dir = home.path().join("orchestrator");
    std::fs::create_dir_all(&data_dir).unwrap();
    let old = serde_json::json!({
        "routes": [], "tiers": {}, "review": "",
        "classifier": {"backend": "openrouter", "model": "old-model", "providerId": ""},
        "sandbox": "host", "allowedDomains": [], "protectedPaths": [],
        "maxAttempts": 4, "parallel": 2,
    });
    std::fs::write(data_dir.join("settings.json"), old.to_string()).unwrap();
    let socket = socket_of(home.path());
    let child = spawn_daemon(home.path(), &[]);
    wait_for_socket(&socket);

    let settings = request_on(&socket, "settings.get", serde_json::json!({}));
    assert_eq!(settings["maxAttempts"], 4, "{settings}");
    assert!(settings.get("classifier").is_none(), "{settings}");

    request_on(
        &socket,
        "secrets.set",
        serde_json::json!({"classifier": {"key": "k", "baseUrl": "http://x"}, "profiles": {}}),
    );

    stop_daemon(&socket);
    let _ = wait_for_exit(child, Duration::from_secs(5));
}
