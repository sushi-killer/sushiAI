use super::*;

#[test]
fn a_task_whose_daemon_crashed_resumes_its_surviving_run_on_restart() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("first-run");
    // The first run is slow and outlives the daemon killed under it; the next daemon
    // follows that same run and finishes the task without a second attempt.
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\nif [ ! -f {m} ]; then touch {m}; sleep 4; fi\necho changed > CHANGED_MARKER.txt\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}}'\necho '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"done\"}}'\n",
        m = marker.display()
    );
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &body);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let home = data_holder.path().to_path_buf();

    let socket1 = socket_of(&home);
    let mut first = spawn_daemon(&home, &fake_bins);
    wait_for_socket(&socket1);
    let mut settings = request_on(&socket1, "settings.get", serde_json::json!({}));
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
    let repo = init_git_repo();
    let task = request_on(
        &socket1,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Survives a crash", "goal": "g", "criteria": [], "verify": ["true"], "start": true}),
    );
    let id = task["id"].as_str().unwrap().to_string();
    wait_until("the harness to start", || marker.exists().then_some(()));
    wait_for_saved_pgid(&socket1, &id);
    first.kill().unwrap();
    let _ = first.wait();

    let socket2 = socket_of(&home);
    let second = spawn_daemon(&home, &fake_bins);
    wait_for_socket(&socket2);
    let call = |m: &str, p: serde_json::Value| request_on(&socket2, m, p);
    let settled = wait_until("the task to resume and finish after the restart", || {
        let t = call("task.get", serde_json::json!({"id": id}));
        (t["status"] == "done").then_some(t)
    });
    assert_eq!(
        settled["attempts"].as_array().unwrap().len(),
        1,
        "{settled}"
    );
    assert_eq!(settled["attempts"][0]["status"], "passed", "{settled}");
    stop_daemon(&socket2);
    let _ = wait_for_exit(second);
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}

#[test]
fn a_shutdown_leaves_the_run_going_and_the_next_daemon_finishes_the_task() {
    let settled = hang_shutdown_and_restart(false);
    assert_eq!(settled["status"], "done", "{settled}");
    // The run was left going by the shutdown and followed by the next daemon.
    assert_eq!(
        settled["attempts"].as_array().unwrap().len(),
        1,
        "{settled}"
    );
    assert_eq!(settled["attempts"][0]["status"], "passed", "{settled}");
}

#[test]
fn an_owner_stop_stays_stopped_across_a_restart() {
    let settled = hang_shutdown_and_restart(true);
    assert_eq!(settled["status"], "stopped", "{settled}");
}

#[test]
fn a_run_whose_wrapper_is_killed_before_its_exit_file_does_not_hang_the_task() {
    let scripts_dir = tempfile::tempdir().unwrap();
    // The harness SIGKILLs the wrapper shell that is its parent (and the
    // daemon's own unreaped child), so no exit file is ever written. The
    // advisor run is a direct child of the daemon and must not be killed.
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude-orphaned.sh",
        "#!/bin/sh\nbrief=$(cat)\ncase \"$brief\" in *'An implement attempt at this task failed'*) exit 0;; esac\nkill -9 $PPID\nexit 0\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(1);
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Wrapper killed",
            "goal": "The wrapper dies at once",
            "verify": ["true"],
            "variant": {"stallTimeoutSecs": 0},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    let settled = poll_task_status(&daemon, &task_id);
    assert_ne!(settled["status"], "running", "{settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
