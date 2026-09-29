//! Black-box integration tests (task loop): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::process::Command;
use std::time::{Duration, Instant};

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

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(settled["attempts"][0]["status"], "passed");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
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

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(settled["status"], "waiting", "task JSON: {settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "should stop after maxAttempts=2");
    for a in attempts {
        assert_eq!(a["status"], "failed");
        assert_eq!(a["failure"]["kind"], "verify");
    }
    assert!(settled["question"]["text"]
        .as_str()
        .unwrap()
        .starts_with("Attempts keep failing with"));

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn a_task_waiting_for_its_owner_does_not_hold_a_parallel_slot() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"changed\" > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let data = data_holder.path().to_path_buf();
    // `parallel` is read at startup: save it with a first daemon, then
    // restart onto the saved settings in the same data dir.
    let socket1 = data.join("orchd1.sock");
    let first = spawn_orchd_raw(&data, &socket1, &fake_bins);
    wait_for_socket(&socket1);
    let token1 = read_control_token(&data);
    let mut settings = request_on(
        &socket1,
        "settings.get",
        serde_json::json!({}),
        Some(&token1),
    );
    settings["maxAttempts"] = serde_json::json!(1);
    settings["parallel"] = serde_json::json!(1);
    settings["review"] = serde_json::json!("");
    fit_sandbox(&mut settings);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    request_on(
        &socket1,
        "settings.set",
        serde_json::json!({"settings": settings}),
        Some(&token1),
    );
    let _ = request_on(&socket1, "shutdown", serde_json::json!({}), Some(&token1));
    let _ = wait_for_exit(first, Duration::from_secs(5));

    let socket = data.join("orchd2.sock");
    let child = spawn_orchd_raw(&data, &socket, &fake_bins);
    wait_for_socket(&socket);
    let token = read_control_token(&data);
    let call = |m: &str, p: serde_json::Value| request_on(&socket, m, p, Some(&token));

    let repo = init_git_repo();
    let waiting = call(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Always fails", "goal": "g", "criteria": [], "verify": ["false"], "start": true}),
    );
    let waiting_id = waiting["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    while call("task.get", serde_json::json!({"id": waiting_id}))["status"] != "waiting" {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "first task never reached waiting"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(call("ping", serde_json::json!({}))["running"], 0);

    let passing = call(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Passes", "goal": "g", "criteria": [], "verify": ["true"], "start": true}),
    );
    let passing_id = passing["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    loop {
        let status = call("task.get", serde_json::json!({"id": passing_id}))["status"].clone();
        if status == "done" {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "second task starved: {status}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = call("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(child, Duration::from_secs(5));
    for t in [&waiting, &passing] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

/// Like [`FAKE_PLANNER_SCRIPT`] but the draft's own `verify` always fails,
/// for exercising the implement loop's own attempt budget on top of a
/// drafted task (spec review item P1-3: the plan attempt must never count
/// against `maxAttempts`).
const FAILING_VERIFY_PLANNER_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"false\"],\"questions\":[]}\n```"}'
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

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
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(
        attempts.len(),
        2,
        "one plan attempt, one implement attempt: {settled}"
    );
    assert_eq!(attempts[0]["stage"], "plan");
    assert_eq!(attempts[1]["stage"], "implement");
    assert_eq!(attempts[1]["failure"]["kind"], "verify");
    assert!(settled["question"]["text"]
        .as_str()
        .unwrap()
        .starts_with("Attempts keep failing with"));

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
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

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
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
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn a_task_interrupted_by_a_daemon_crash_resumes_on_restart() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("first-run");
    // The first run hangs (the daemon is killed under it); any later run
    // finishes the task.
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\nif [ ! -f {m} ]; then touch {m}; sleep 30; fi\necho changed > CHANGED_MARKER.txt\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}}'\necho '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"done\"}}'\n",
        m = marker.display()
    );
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &body);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let data = data_holder.path().to_path_buf();

    let socket1 = data.join("orchd1.sock");
    let mut first = spawn_orchd_raw(&data, &socket1, &fake_bins);
    wait_for_socket(&socket1);
    let token = read_control_token(&data);
    let mut settings = request_on(
        &socket1,
        "settings.get",
        serde_json::json!({}),
        Some(&token),
    );
    settings["review"] = serde_json::json!("");
    fit_sandbox(&mut settings);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    request_on(
        &socket1,
        "settings.set",
        serde_json::json!({"settings": settings}),
        Some(&token),
    );
    let repo = init_git_repo();
    let task = request_on(
        &socket1,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Survives a crash", "goal": "g", "criteria": [], "verify": ["true"], "start": true}),
        Some(&token),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    while !marker.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "harness never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    first.kill().unwrap();
    let _ = first.wait();

    let socket2 = data.join("orchd2.sock");
    let second = spawn_orchd_raw(&data, &socket2, &fake_bins);
    wait_for_socket(&socket2);
    let token = read_control_token(&data);
    let call = |m: &str, p: serde_json::Value| request_on(&socket2, m, p, Some(&token));
    let start = Instant::now();
    let settled = loop {
        let t = call("task.get", serde_json::json!({"id": id}));
        if t["status"] == "done" {
            break t;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "task did not resume after restart: {t}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(settled["attempts"][0]["status"], "interrupted");
    let _ = call("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(second, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}

/// Starts a task whose first harness run hangs, then ends the daemon with
/// `end` (the `shutdown` RPC or `task.stop` followed by `shutdown`), restarts
/// it and returns the task as the second daemon sees it once it settled or
/// after a short quiet period.
fn hang_shutdown_and_restart(stop_first: bool) -> serde_json::Value {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("first-run");
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\nif [ ! -f {m} ]; then touch {m}; sleep 30; fi\necho changed > CHANGED_MARKER.txt\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}}'\necho '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"done\"}}'\n",
        m = marker.display()
    );
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &body);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let data = data_holder.path().to_path_buf();

    let socket1 = data.join("orchd1.sock");
    let first = spawn_orchd_raw(&data, &socket1, &fake_bins);
    wait_for_socket(&socket1);
    let token = read_control_token(&data);
    let mut settings = request_on(
        &socket1,
        "settings.get",
        serde_json::json!({}),
        Some(&token),
    );
    settings["review"] = serde_json::json!("");
    fit_sandbox(&mut settings);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    request_on(
        &socket1,
        "settings.set",
        serde_json::json!({"settings": settings}),
        Some(&token),
    );
    let repo = init_git_repo();
    let task = request_on(
        &socket1,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Survives a shutdown", "goal": "g", "criteria": [], "verify": ["true"], "start": true}),
        Some(&token),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    while !marker.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "harness never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    if stop_first {
        request_on(
            &socket1,
            "task.stop",
            serde_json::json!({"id": id}),
            Some(&token),
        );
        let start = Instant::now();
        loop {
            let t = request_on(
                &socket1,
                "task.get",
                serde_json::json!({"id": id}),
                Some(&token),
            );
            if t["status"] == "stopped" {
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "not stopped: {t}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    request_on(&socket1, "shutdown", serde_json::json!({}), Some(&token));
    let _ = wait_for_exit(first, Duration::from_secs(10));

    let socket2 = data.join("orchd2.sock");
    let second = spawn_orchd_raw(&data, &socket2, &fake_bins);
    wait_for_socket(&socket2);
    let token = read_control_token(&data);
    let call = |m: &str, p: serde_json::Value| request_on(&socket2, m, p, Some(&token));
    let start = Instant::now();
    let settled = loop {
        let t = call("task.get", serde_json::json!({"id": id}));
        let limit = if stop_first { 3 } else { 20 };
        if t["status"] == "done" || (stop_first && start.elapsed() > Duration::from_secs(limit)) {
            break t;
        }
        assert!(
            start.elapsed() < Duration::from_secs(limit),
            "task did not resume after restart: {t}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let _ = call("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(second, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
    settled
}

#[test]
fn a_shutdown_leaves_a_running_task_resumable_and_the_next_daemon_finishes_it() {
    let settled = hang_shutdown_and_restart(false);
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(settled["attempts"][0]["status"], "interrupted");
}

#[test]
fn an_owner_stop_stays_stopped_across_a_restart() {
    let settled = hang_shutdown_and_restart(true);
    assert_eq!(settled["status"], "stopped", "{settled}");
}

#[test]
fn a_silent_harness_is_stopped_as_stalled_when_the_variant_sets_a_stall_timeout() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude-silent.sh",
        "#!/bin/sh\necho $$ > \"$PID_FILE\"\ncat > /dev/null\nexec sleep 90\n",
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

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(90));
    assert_eq!(settled["status"], "waiting", "task JSON: {settled}");
    assert_eq!(
        settled["attempts"][0]["failure"]["kind"], "stall",
        "{settled}"
    );
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let start = Instant::now();
    while is_alive(pid) {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "stalled harness {pid} is still running"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn a_fresh_retry_starts_a_new_session_that_reads_the_earlier_handoff() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-retry.sh", FAKE_RETRY_SCRIPT);
    let args_log = scripts_dir.path().join("args.log");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("ARGS_LOG", args_log.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Two tries",
            "goal": "Needs a second attempt",
            "verify": ["test -f SECOND"],
                "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{settled}");
    assert_eq!(attempts[0]["handoff"], "tried the quick fix", "");

    let argv: Vec<String> = std::fs::read_to_string(&args_log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert!(!argv[1].contains("--resume"), "{argv:?}");
    {
        let brief = std::fs::read_to_string(
            daemon
                .data_dir()
                .join("tasks")
                .join(&task_id)
                .join("runs/2/brief.md"),
        )
        .unwrap();
        assert!(brief.contains("## Task"), "not the full brief: {brief}");
        assert!(brief.contains("handoff: tried the quick fix"), "{brief}");
        assert!(brief.contains("test -f SECOND exited"), "{brief}");
    }

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
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
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(60));
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(
        settled["attempts"].as_array().unwrap().len(),
        1,
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// The prompt-prefix trim (`--disallowedTools`) used to be gated behind
/// `variant.leanContext`; it is now unconditional for every Claude implement
/// run, and mcp.json carries exactly the configured servers -- no Jev
/// filtering, no skills hook wired into `settings.json`.
#[test]
fn a_claude_implement_run_trims_delegation_tools_and_keeps_the_configured_mcp_servers() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", FAKE_CLAUDE_PASS);
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
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");

    let argv = std::fs::read_to_string(&args).unwrap();
    assert!(argv.contains("--disallowedTools"), "{argv}");
    let settings: serde_json::Value =
        serde_json::from_str(&run_file(&daemon, &task_id, "settings.json")).unwrap();
    let hooks: Vec<&String> = settings["hooks"].as_object().unwrap().keys().collect();
    assert_eq!(hooks, ["PreToolUse", "Stop"], "{settings}");
    let mcp: serde_json::Value =
        serde_json::from_str(&run_file(&daemon, &task_id, "mcp.json")).unwrap();
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
    let _ = std::fs::remove_dir_all(worktree);
}

/// Emits the same `Bash` call three times, a different one on the second
/// run so the two failures do not share a signature, then hangs.
const FAKE_LOOPING_CLAUDE: &str = r#"#!/bin/sh
cat > /dev/null
if [ -f RAN ]; then word=beta; else word=alpha; fi
touch RAN
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-loop"}'
for i in 1 2 3; do
  printf '%s\n' "{\"type\":\"assistant\",\"message\":{\"id\":\"m$i\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t$i\",\"name\":\"Bash\",\"input\":{\"command\":\"echo $word\"}}]}}"
done
exec sleep 60
"#;

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
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(60));
    assert_eq!(settled["status"], "waiting", "task JSON: {settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 3, "{settled}");
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
    let _ = std::fs::remove_dir_all(worktree);
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
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
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
    let _ = std::fs::remove_dir_all(worktree);
}
