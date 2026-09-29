//! Black-box integration tests (plan): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::time::{Duration, Instant};

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
            "start": true,
        }),
    );
    assert_eq!(task["status"], "drafting");
    let task_id = task["id"].as_str().unwrap().to_string();

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
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

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
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

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
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

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["title"], "Add dark mode");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
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
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
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
    let _ = std::fs::remove_dir_all(worktree);
}

/// The plan branch never produces a valid ```sushi-plan fence, no matter how
/// many times it's asked -- for exercising the "could not draft this task"
/// clarification path itself (as opposed to [`CLARIFY_THEN_SUCCEED_SCRIPT`],
/// which uses the clarification to recover).
const GARBAGE_PLANNER_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"no fenced block here, sorry"}'
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

/// The plan branch fails to parse until the brief it's given contains the
/// owner's clarification (which `run_plan_stage` folds into `task.request`
/// verbatim as `"(Owner clarification: ...)"`), then succeeds -- for
/// exercising "ask -> answer -> replan -> succeeds" end to end.
const CLARIFY_THEN_SUCCEED_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    case "$input" in
      *"Owner clarification"*)
        json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"true\"],\"questions\":[]}\n```"}'
        ;;
      *)
        json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"no fenced block here, sorry"}'
        ;;
    esac
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
fn a_task_waiting_on_its_plan_question_does_not_hold_a_parallel_slot() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let fake_bins = [
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PLAN_ASK_QUESTION", "1"),
    ];
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
    settings["parallel"] = serde_json::json!(1);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    fit_sandbox(&mut settings);
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
    let drafting = call(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "request": "add dark mode", "start": true}),
    );
    let drafting_id = drafting["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    while call("task.get", serde_json::json!({"id": drafting_id}))["status"] != "waiting" {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "drafting task never reached waiting on its plan question"
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
            "second task starved while the first sat on a plan question: {status}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = call("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(child, Duration::from_secs(5));
    for t in [&drafting, &passing] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
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

    let first_wait = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
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
    let stopped = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "stopped" || s == "failed"
    });
    assert_eq!(stopped["status"], "stopped", "task JSON: {stopped}");

    daemon.request("task.start", serde_json::json!({"id": task_id}));
    let second_wait = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
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
    let _ = std::fs::remove_dir_all(worktree);
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

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert!(waiting["question"]["text"]
        .as_str()
        .unwrap()
        .contains("could not draft"));

    daemon.request(
        "task.answer",
        serde_json::json!({"id": task_id, "answer": "it's the settings screen's theme picker"}),
    );

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
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

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

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

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
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
    let _ = std::fs::remove_dir_all(worktree);
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
    poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
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
    let _ = std::fs::remove_dir_all(worktree);
}

/// The planner flags a criterion visual although its check is a command.
const VISUAL_COMMAND_CRITERION_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Parser\",\"goal\":\"Handle X\",\"criteria\":[{\"text\":\"the parser handles X -- check: cargo test\",\"visual\":true}],\"verify\":[\"true\"],\"questions\":[]}\n```"}'
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
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    let implement: Vec<_> = attempts
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert_eq!(implement.len(), 1, "{settled}");
    assert_eq!(implement[0]["n"], 1, "{settled}");
    assert!(implement[0]["failure"].is_null(), "{settled}");
    assert!(
        attempts.iter().all(|a| a["failure"]["kind"] != "evidence"),
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}
