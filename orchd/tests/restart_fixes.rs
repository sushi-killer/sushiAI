//! Black-box integration tests: what a daemon restart does to a final check
//! and to a dependency question.

mod common;

use common::*;
use std::time::{Duration, Instant};

const PASS_SCRIPT: &str = "#!/bin/sh\ninput=\"$(cat)\"\ncase \"$input\" in *SLOW*) sleep 30 ;; esac\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n";

/// A first daemon with review off, on a data dir the test keeps.
fn first_daemon(
    data: &std::path::Path,
    env: &[(&str, &str)],
) -> (OrchdChild, std::path::PathBuf, String) {
    let socket = data.join("orchd1.sock");
    let child = spawn_orchd_raw(data, &socket, env);
    wait_for_socket(&socket);
    let token = read_control_token(data);
    let mut settings = request_on(&socket, "settings.get", serde_json::json!({}), Some(&token));
    settings["review"] = serde_json::json!("");
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    fit_sandbox(&mut settings);
    request_on(
        &socket,
        "settings.set",
        serde_json::json!({"settings": settings}),
        Some(&token),
    );
    (child, socket, token)
}

fn wait_status(
    socket: &std::path::Path,
    token: &str,
    id: &str,
    want: &str,
    secs: u64,
) -> serde_json::Value {
    let start = Instant::now();
    loop {
        let t = request_on(
            socket,
            "task.get",
            serde_json::json!({"id": id}),
            Some(token),
        );
        if t["status"] == want {
            return t;
        }
        assert!(
            start.elapsed() < Duration::from_secs(secs),
            "never {want}: {t}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_final_check_killed_by_a_shutdown_is_rerun_not_failed() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", PASS_SCRIPT);
    let marker = scripts_dir.path().join("final-started");
    let final_check = format!(
        "if [ ! -f {m} ]; then touch {m}; sleep 60; fi",
        m = marker.display()
    );
    let env = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let data = data_holder.path().to_path_buf();
    let (first, socket1, token1) = first_daemon(&data, &env);
    let repo = init_git_repo();
    let task = request_on(
        &socket1,
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Slow final check",
            "goal": "g",
            "verify": ["true"],
            "finalVerify": [final_check],
            "start": true,
        }),
        Some(&token1),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    while !marker.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "final check never ran"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = request_on(&socket1, "shutdown", serde_json::json!({}), Some(&token1));
    let _ = wait_for_exit(first, Duration::from_secs(15));

    let stopped: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(data.join("tasks").join(&id).join("task.json")).unwrap(),
    )
    .unwrap();
    for a in stopped["attempts"].as_array().unwrap() {
        assert!(a.get("failure").is_none_or(|f| f.is_null()), "{stopped}");
        assert_eq!(a["status"], "interrupted", "{stopped}");
    }

    let socket2 = data.join("orchd2.sock");
    let second = spawn_orchd_raw(&data, &socket2, &env);
    wait_for_socket(&socket2);
    let token2 = read_control_token(&data);
    let done = wait_status(&socket2, &token2, &id, "done", 30);
    for a in done["attempts"].as_array().unwrap() {
        assert!(a.get("failure").is_none_or(|f| f.is_null()), "{done}");
    }
    let _ = request_on(&socket2, "shutdown", serde_json::json!({}), Some(&token2));
    let _ = wait_for_exit(second, Duration::from_secs(15));
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}

#[test]
fn a_dependency_question_that_became_moot_while_down_clears_on_start() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", PASS_SCRIPT);
    let env = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let data = data_holder.path().to_path_buf();
    let (first, socket1, token1) = first_daemon(&data, &env);
    let repo = init_git_repo();
    let call1 = |m: &str, p: serde_json::Value| request_on(&socket1, m, p, Some(&token1));
    let dep = call1(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "title": "Slow dependency",
        "goal": "SLOW g", "verify": ["true"], "start": true}),
    );
    let dep_id = dep["id"].as_str().unwrap().to_string();
    let dependent = call1(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "title": "Dependent",
        "goal": "g", "verify": ["true"], "dependsOn": [dep_id]}),
    );
    let dependent_id = dependent["id"].as_str().unwrap().to_string();
    wait_status(&socket1, &token1, &dep_id, "running", 10);
    call1("task.stop", serde_json::json!({"id": dep_id}));
    let asked = wait_status(&socket1, &token1, &dependent_id, "waiting", 15);
    assert!(asked["question"]["text"]
        .as_str()
        .unwrap()
        .contains("stopped"));
    let _ = call1("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(first, Duration::from_secs(15));

    // While the daemon is down the dependency ends up done.
    let path = data.join("tasks").join(&dep_id).join("task.json");
    let mut t: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    t["status"] = serde_json::json!("done");
    std::fs::write(&path, t.to_string()).unwrap();

    let socket2 = data.join("orchd2.sock");
    let second = spawn_orchd_raw(&data, &socket2, &env);
    wait_for_socket(&socket2);
    let token2 = read_control_token(&data);
    let done = wait_status(&socket2, &token2, &dependent_id, "done", 30);
    assert!(done.get("question").is_none_or(|q| q.is_null()), "{done}");
    let _ = request_on(&socket2, "shutdown", serde_json::json!({}), Some(&token2));
    let _ = wait_for_exit(second, Duration::from_secs(15));
    for w in [&dep, &dependent] {
        let _ = std::fs::remove_dir_all(w["worktree"].as_str().unwrap());
    }
}

#[test]
fn verify_and_final_checks_see_the_tasks_base_commit_in_orchd_base_sha() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", PASS_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let check = "test \"$ORCHD_BASE_SHA\" = \"$(git rev-parse HEAD)\"";
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Base sha",
            "goal": "g",
            "verify": [check],
            "finalVerify": [check],
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let done = poll_task_status(&daemon, &id, Duration::from_secs(30));
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(
        done["baseSha"],
        git_out(repo.path(), &["rev-parse", "HEAD"]).trim()
    );
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}

#[test]
fn an_accept_answer_after_a_restart_commits_the_last_attempt() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        FAKE_FAILING_REVIEW_SCRIPT,
    );
    let env = [
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("LOG_DIR", scripts_dir.path().to_str().unwrap()),
        ("ACCEPT_REPLY", "false"),
    ];
    let data_holder = tempfile::tempdir().unwrap();
    let data = data_holder.path().to_path_buf();
    let (first, socket1, token1) = first_daemon(&data, &env);
    let mut settings = request_on(
        &socket1,
        "settings.get",
        serde_json::json!({}),
        Some(&token1),
    );
    settings["review"] = serde_json::json!("claude-opus");
    settings["maxAttempts"] = serde_json::json!(1);
    request_on(
        &socket1,
        "settings.set",
        serde_json::json!({"settings": settings}),
        Some(&token1),
    );
    let repo = init_git_repo();
    let task = request_on(
        &socket1,
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Accept after restart",
            "goal": "g",
            "verify": ["true"],
            "start": true,
        }),
        Some(&token1),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let waiting = wait_status(&socket1, &token1, &id, "waiting", 20);
    assert!(waiting["question"]["options"]
        .to_string()
        .contains("accept the last attempt as done"));
    let _ = request_on(&socket1, "shutdown", serde_json::json!({}), Some(&token1));
    let _ = wait_for_exit(first, Duration::from_secs(15));

    let socket2 = data.join("orchd2.sock");
    let second = spawn_orchd_raw(&data, &socket2, &env);
    wait_for_socket(&socket2);
    let token2 = read_control_token(&data);
    let still = wait_status(&socket2, &token2, &id, "waiting", 15);
    assert!(still["question"].is_object(), "{still}");
    request_on(
        &socket2,
        "task.answer",
        serde_json::json!({"id": id, "answer": "Accept the last attempt as done"}),
        Some(&token2),
    );
    let done = wait_status(&socket2, &token2, &id, "done", 30);
    let implement = done["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .count();
    assert_eq!(implement, 1, "{done}");
    let tree = git_out(
        repo.path(),
        &["ls-tree", "--name-only", done["branch"].as_str().unwrap()],
    );
    assert!(tree.contains("CHANGED_MARKER.txt"), "{tree}");
    assert!(
        done["decisions"]
            .to_string()
            .contains("Owner: accepted attempt 1 as done; review skipped"),
        "{done}"
    );
    let _ = request_on(&socket2, "shutdown", serde_json::json!({}), Some(&token2));
    let _ = wait_for_exit(second, Duration::from_secs(15));
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}
