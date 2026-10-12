//! Black-box integration tests (plan): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;

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
    let home = data_holder.path().to_path_buf();
    // `parallel` is read at startup: save it with a first daemon, then
    // restart onto the saved settings in the same data dir.
    let socket1 = socket_of(&home);
    let first = spawn_daemon(&home, &fake_bins);
    wait_for_socket(&socket1);
    let mut settings = request_on(&socket1, "settings.get", serde_json::json!({}));
    settings["parallel"] = serde_json::json!(1);
    settings["briefCheckRoute"] = serde_json::json!("");
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
    assert_eq!(implement[0]["n"], 1, "{settled}");
    assert!(implement[0]["failure"].is_null(), "{settled}");
    assert!(
        attempts.iter().all(|a| a["failure"]["kind"] != "evidence"),
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

/// The planner names a screenshot command it found in the repo.
const SCREENSHOT_PLAN_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Panel\",\"goal\":\"Change the panel\",\"criteria\":[\"the panel shows X -- check: screenshot artifacts/panel.png\"],\"verify\":[\"true\"],\"screenshot\":\"  mkdir -p artifacts && echo png > artifacts/panel.png  \",\"questions\":[]}\n```"}'
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

/// The parent plan splits into two parts; `PART_B_JSON` is part B's draft.
/// A child's planner (its request names `CHILD_REQ_*`) answers with a small
/// plan of its own; an implement run writes a file per part.
const PARENT_DRAFTS_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
case "$input" in
  *"You check a coding task brief"*) out '"{\"contradiction\": false, \"conflict\": \"\"}"' ;;
  *sushi-plan*)
    case "$input" in
      *CHILD_REQ_B*) out '"```sushi-plan\n{\"title\":\"Part B\",\"goal\":\"MAKE_B: create b.txt\",\"criteria\":[\"b.txt exists\"],\"verify\":[\"test -f b.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"Both parts\",\"goal\":\"Create a.txt, then b.txt\",\"verify\":[\"true\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"goal\":\"MAKE_A: create a.txt\",\"criteria\":[\"a.txt exists\",{\"text\":\"a screenshot of a.txt\",\"visual\":true}],\"verify\":[\"test -f a.txt\"],\"finalVerify\":[\"true\"],\"paths\":[\"a.txt\"],\"tier\":\"standard\",\"request\":\"CHILD_REQ_A create a.txt\"},__PART_B__]}\n```"' ;;
    esac
    ;;
  *MAKE_B*) echo b > b.txt; out "$report" ;;
  *MAKE_A*) echo a > a.txt; out "$report" ;;
  *) out "$report" ;;
esac
"#;

const COMPLETE_PART_B: &str = r#"{\"key\":\"b\",\"title\":\"Part B\",\"goal\":\"MAKE_B: create b.txt\",\"criteria\":[\"b.txt exists\"],\"verify\":[\"test -f b.txt\"],\"paths\":[\"b.txt\"],\"dependsOn\":[\"a\"],\"request\":\"CHILD_REQ_B create b.txt\"}"#;
const INCOMPLETE_PART_B: &str =
    r#"{\"key\":\"b\",\"dependsOn\":[\"a\"],\"request\":\"CHILD_REQ_B create b.txt\"}"#;

fn run_split(
    part_b: &str,
) -> (
    Daemon,
    tempfile::TempDir,
    tempfile::TempDir,
    serde_json::Value,
) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts.path(),
        "fake-claude.sh",
        &PARENT_DRAFTS_SCRIPT.replace("__PART_B__", part_b),
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build both parts",
            "start": true,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let parent = settle(&daemon, &parent_id);
    (daemon, scripts, repo, parent)
}

fn children_of(daemon: &Daemon, parent: &serde_json::Value) -> Vec<serde_json::Value> {
    let tasks = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["parent"] == parent["id"])
        .cloned()
        .collect()
}

fn plan_attempts(task: &serde_json::Value) -> usize {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "plan")
        .count()
}

#[test]
fn a_split_drafts_children_that_skip_their_own_planner() {
    let (daemon, _scripts, _repo, parent) = run_split(COMPLETE_PART_B);
    assert_eq!(parent["status"], "done", "{parent}");
    let children = children_of(&daemon, &parent);
    assert_eq!(children.len(), 2, "{children:?}");
    for child in &children {
        assert_eq!(child["status"], "done", "{child}");
        assert_eq!(child["plannedByParent"], true, "{child}");
        assert_eq!(plan_attempts(child), 0, "{child}");
        assert!(!child["verify"].as_array().unwrap().is_empty(), "{child}");
        assert!(!child["criteria"].as_array().unwrap().is_empty(), "{child}");
        assert!(!child["goal"].as_str().unwrap().is_empty(), "{child}");
    }
    let a = children.iter().find(|c| c["title"] == "Part A").unwrap();
    assert_eq!(a["finalVerify"], serde_json::json!(["true"]));
    assert_eq!(a["paths"], serde_json::json!(["a.txt"]));
    assert_eq!(a["plannedTier"], "standard");
    assert_eq!(
        a["visualCriteria"],
        serde_json::json!(["a screenshot of a.txt"])
    );

    let costs: Vec<serde_json::Value> =
        std::fs::read_to_string(daemon.data_dir().join("costs.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
    let planned: Vec<&serde_json::Value> = costs.iter().filter(|r| r["stage"] == "plan").collect();
    assert!(!planned.is_empty(), "the parent was planned: {costs:?}");
    assert!(
        planned.iter().all(|r| r["taskId"] == parent["id"]),
        "a child was planned: {planned:?}"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn an_incomplete_part_draft_is_still_planned_by_its_own_planner() {
    let (daemon, _scripts, _repo, parent) = run_split(INCOMPLETE_PART_B);
    assert_eq!(parent["status"], "done", "{parent}");
    let children = children_of(&daemon, &parent);
    assert_eq!(children.len(), 2, "{children:?}");
    let a = children.iter().find(|c| c["title"] == "Part A").unwrap();
    assert_eq!(a["plannedByParent"], true, "{a}");
    assert_eq!(plan_attempts(a), 0, "{a}");
    let b = children.iter().find(|c| c["id"] != a["id"]).unwrap();
    assert_ne!(b["plannedByParent"], true, "{b}");
    assert_eq!(plan_attempts(b), 1, "{b}");
    assert_eq!(b["status"], "done", "{b}");
    daemon.shutdown_and_wait();
}

/// Like `PARENT_DRAFTS_SCRIPT`, part A drafted with `checks`, an
/// out-of-range check, a `heldOut` check and a `screenshot` command.
const GROUNDED_PART_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
case "$input" in
  *"You check a coding task brief"*) out '"{\"contradiction\": false, \"conflict\": \"\"}"' ;;
  *sushi-plan*) out '"```sushi-plan\n{\"title\":\"One part\",\"goal\":\"Create a.txt\",\"verify\":[\"true\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"goal\":\"MAKE_A: create a.txt\",\"criteria\":[\"a.txt exists\"],\"verify\":[\"true\"],\"checks\":[{\"criterion\":0,\"run\":\"test -f a.txt\"},{\"criterion\":9,\"run\":\"test -f nowhere.txt\"}],\"heldOut\":{\"criterion\":0,\"run\":\"test -s a.txt\"},\"screenshot\":\"  mkdir -p artifacts && echo png > artifacts/a.png  \",\"paths\":[\"a.txt\"],\"request\":\"CHILD_REQ_A\"},{\"key\":\"b\",\"title\":\"Part B\",\"goal\":\"MAKE_B: create b.txt\",\"criteria\":[\"b.txt exists\"],\"verify\":[\"test -f b.txt\"],\"paths\":[\"b.txt\"],\"dependsOn\":[\"a\"],\"request\":\"CHILD_REQ_B\"}]}\n```"' ;;
  *MAKE_A*) echo a > a.txt; out "$report" ;;
  *MAKE_B*) echo b > b.txt; out "$report" ;;
  *) out "$report" ;;
esac
"#;

#[test]
fn a_parent_drafted_part_keeps_its_drafted_checks_and_held_out() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", GROUNDED_PART_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build the part",
            "variant": {"groundedChecks": true},
            "start": true,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let parent = settle(&daemon, &parent_id);
    let children = children_of(&daemon, &parent);
    assert_eq!(children.len(), 2, "{parent}");
    let a = children.iter().find(|c| c["title"] == "Part A").unwrap();
    assert_eq!(a["plannedByParent"], true, "{a}");
    assert_eq!(plan_attempts(a), 0, "{a}");
    let checks = a["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 1, "the out-of-range check is dropped: {a}");
    assert_eq!(checks[0]["criterion"], 0);
    assert_eq!(checks[0]["run"], "test -f a.txt");
    assert_eq!(a["heldOut"]["run"], "test -s a.txt", "{a}");
    assert_eq!(
        a["briefCheck"]["screenshot"], "mkdir -p artifacts && echo png > artifacts/a.png",
        "{a}"
    );
    daemon.shutdown_and_wait();
}

fn assert_plan_route(
    request: &str,
    max_chars: usize,
    variant: serde_json::Value,
    route: &str,
    reason: &str,
) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-plan-route.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["planner"] = serde_json::json!("claude-opus");
    settings["tiers"]["standard"] = serde_json::json!("claude-sonnet");
    settings["smallPlanMaxChars"] = serde_json::json!(max_chars);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(), "request": request,
            "variant": variant, "start": false,
        }),
    );
    let id = task["id"].as_str().unwrap();
    let settled = poll_until(&daemon, id, |s| s == "stopped" || s == "failed");
    assert_eq!(settled["status"], "stopped", "{settled}");
    let attempt = &settled["attempts"][0];
    assert_eq!(attempt["stage"], "plan");
    assert_eq!(attempt["routeId"], route, "{settled}");
    let line = format!("Plan route: {route} ({reason})");
    assert_eq!(
        settled["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d.as_str() == Some(line.as_str()))
            .count(),
        1,
        "{settled}"
    );
    if variant["plannerRoute"].is_string() {
        let line = format!("Variant: planner -> route {route} (override)");
        assert!(
            settled["decisions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d.as_str() == Some(line.as_str())),
            "{settled}"
        );
    }
    daemon.shutdown_and_wait();
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}

#[test]
fn plan_route_small_request_uses_standard_with_size_facts() {
    let request = "Fix src/button.rs";
    assert_plan_route(
        request,
        1200,
        serde_json::json!({}),
        "claude-sonnet",
        &format!(
            "small request: {} chars <= 1200, 1 paths <= 3, no list",
            request.chars().count()
        ),
    );
}

#[test]
fn plan_route_long_request_uses_planner() {
    assert_plan_route(
        &"x".repeat(1201),
        1200,
        serde_json::json!({}),
        "claude-opus",
        "not small: 1201 chars > 1200",
    );
}

#[test]
fn plan_route_numbered_and_bulleted_lists_use_planner() {
    for request in ["1. Fix button\n2. Add test", "- Fix button\n+ Add test"] {
        assert_plan_route(
            request,
            1200,
            serde_json::json!({}),
            "claude-opus",
            "not small: 2 list items >= 2",
        );
    }
}

#[test]
fn plan_route_variant_override_wins_for_small_request() {
    assert_plan_route(
        "Fix button",
        1200,
        serde_json::json!({"plannerRoute": "claude-opus"}),
        "claude-opus",
        "variant.plannerRoute",
    );
}

#[test]
fn plan_route_zero_turns_off_the_entire_small_request_check() {
    assert_plan_route(
        "Fix button",
        0,
        serde_json::json!({}),
        "claude-opus",
        "small-request check off: smallPlanMaxChars = 0",
    );
}
