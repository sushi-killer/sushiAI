//! Black-box integration tests: what a daemon restart does to a final check
//! and to a dependency question.

mod common;

use common::process::Daemon;
use common::repository::git_out;

use common::harness::fake_harness_script;
use common::native_sandbox::fit_sandbox;
use common::polling::{poll_task_status, remove_dir_retrying, wait_until};
use common::process::{socket_of, spawn_daemon, wait_for_exit, DaemonChild};
use common::protocol::{request_on, stop_daemon, wait_for_socket};
use common::repository::init_git_repo;

const PASS_SCRIPT: &str = "#!/bin/sh\ninput=\"$(cat)\"\ncase \"$input\" in *SLOW*) sleep 30 ;; esac\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n";

/// A first daemon with review off, on a data dir the test keeps.
fn first_daemon(home: &std::path::Path, env: &[(&str, &str)]) -> (DaemonChild, std::path::PathBuf) {
    let socket = socket_of(home);
    let child = spawn_daemon(home, env);
    wait_for_socket(&socket);
    let mut settings = request_on(&socket, "settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["planReview"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    fit_sandbox(&mut settings);
    request_on(
        &socket,
        "settings.set",
        serde_json::json!({"settings": settings}),
    );
    (child, socket)
}

fn wait_status(socket: &std::path::Path, id: &str, want: &str) -> serde_json::Value {
    wait_until(&format!("task {id} to be {want}"), || {
        let t = request_on(socket, "task.get", serde_json::json!({"id": id}));
        (t["status"] == want).then_some(t)
    })
}

#[test]
fn a_final_check_killed_by_a_shutdown_is_rerun_not_failed() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", PASS_SCRIPT);
    let marker = scripts_dir.path().join("final-started");
    // The daemon also runs every check on the base commit in the background,
    // in a scratch checkout under `base-runs`. Only the task's own run may
    // touch the marker, or the test would stop the daemon at the wrong moment.
    let final_check = format!(
        "case \"$PWD\" in */base-runs/*) exit 0 ;; esac; if [ ! -f {m} ]; then touch {m}; sleep 60; fi",
        m = marker.display()
    );
    let env = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let home = data_holder.path().to_path_buf();
    let data = home.join("orchestrator");
    let (first, socket1) = first_daemon(&home, &env);
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
    );
    let id = task["id"].as_str().unwrap().to_string();
    wait_until("the final check to run", || marker.exists().then_some(()));
    stop_daemon(&socket1);
    let _ = wait_for_exit(first);

    let stopped: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(data.join("tasks").join(&id).join("task.json")).unwrap(),
    )
    .unwrap();
    for a in stopped["attempts"].as_array().unwrap() {
        assert!(a.get("failure").is_none_or(|f| f.is_null()), "{stopped}");
        // A run still going at the shutdown stays `running` for the next daemon to follow;
        // one past its run is interrupted by the cancelled final check.
        assert!(
            matches!(a["status"].as_str(), Some("running" | "interrupted")),
            "{stopped}"
        );
    }

    let socket2 = socket_of(&home);
    let second = spawn_daemon(&home, &env);
    wait_for_socket(&socket2);
    let done = wait_status(&socket2, &id, "done");
    for a in done["attempts"].as_array().unwrap() {
        assert!(a.get("failure").is_none_or(|f| f.is_null()), "{done}");
    }
    stop_daemon(&socket2);
    let _ = wait_for_exit(second);
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}

#[test]
fn a_dependency_question_that_became_moot_while_down_clears_on_start() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", PASS_SCRIPT);
    let env = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let home = data_holder.path().to_path_buf();
    let data = home.join("orchestrator");
    let (first, socket1) = first_daemon(&home, &env);
    let repo = init_git_repo();
    let call1 = |m: &str, p: serde_json::Value| request_on(&socket1, m, p);
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
    wait_status(&socket1, &dep_id, "running");
    call1("task.stop", serde_json::json!({"id": dep_id}));
    let asked = wait_status(&socket1, &dependent_id, "waiting");
    assert!(asked["question"]["text"]
        .as_str()
        .unwrap()
        .contains("stopped"));
    stop_daemon(&socket1);
    let _ = wait_for_exit(first);

    // While the daemon is down the dependency ends up done.
    let path = data.join("tasks").join(&dep_id).join("task.json");
    let mut t: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    t["status"] = serde_json::json!("done");
    std::fs::write(&path, t.to_string()).unwrap();

    let socket2 = socket_of(&home);
    let second = spawn_daemon(&home, &env);
    wait_for_socket(&socket2);
    let done = wait_status(&socket2, &dependent_id, "done");
    assert!(done.get("question").is_none_or(|q| q.is_null()), "{done}");
    stop_daemon(&socket2);
    let _ = wait_for_exit(second);
    for w in [&dep, &dependent] {
        remove_dir_retrying(w["worktree"].as_str().unwrap());
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
    let done = poll_task_status(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(
        done["baseSha"],
        git_out(repo.path(), &["rev-parse", "HEAD"]).trim()
    );
    daemon.shutdown_and_wait();
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}
