//! Black-box integration tests (cost budget): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::time::{Duration, Instant};

#[test]
fn a_daemon_restart_mid_attempt_keeps_the_interrupted_attempt_s_estimated_cost() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("first-run");
    // The first run streams one message (twice, as Claude does per content
    // block) and a second one, then hangs with no `result`; the daemon is
    // killed under it. Any later run finishes the task, reporting $0.20 as
    // the session total so far, as the CLI does.
    let msg = |id: &str, input: u32, write: u32, read: u32, output: u32| {
        format!(
            r#"{{"type":"assistant","message":{{"model":"claude-opus-5-5","id":"{id}","type":"message","role":"assistant","content":[],"usage":{{"input_tokens":{input},"cache_creation_input_tokens":{write},"cache_read_input_tokens":{read},"output_tokens":{output}}}}},"parent_tool_use_id":null,"session_id":"sess-fake"}}"#
        )
    };
    let first_run = [
        r#"{"type":"system","subtype":"init","session_id":"sess-fake"}"#.to_string(),
        msg("msg_1", 10, 20_000, 0, 100),
        msg("msg_1", 10, 20_000, 0, 100),
        msg("msg_2", 5, 1_000, 20_000, 2_000),
    ]
    .map(|l| format!("echo '{l}'\n"))
    .concat();
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\nif [ ! -f {m} ]; then touch {m}\n{first_run}sleep 30; fi\necho changed > CHANGED_MARKER.txt\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}}'\necho '{{\"type\":\"result\",\"total_cost_usd\":0.2,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"done\"}}'\n",
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
        "title": "Keeps its spend", "goal": "g", "criteria": [], "verify": ["true"]}),
        Some(&token),
    );
    let id = task["id"].as_str().unwrap().to_string();
    // Killed only once the daemon has written the last message to disk.
    let events = data.join("tasks").join(&id).join("runs/1/events.jsonl");
    let start = Instant::now();
    while !std::fs::read_to_string(&events).is_ok_and(|t| t.contains("msg_2")) {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "harness never streamed its messages"
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
    // Opus list prices per MTok: 4 input, 5 cache write, 0.20 cache read, 20
    // output; msg_1 counted once.
    let estimated = (10.0 * 4.0 + 20_000.0 * 5.0 + 100.0 * 20.0) / 1e6
        + (5.0 * 4.0 + 1_000.0 * 5.0 + 20_000.0 * 0.20 + 2_000.0 * 20.0) / 1e6;
    let interrupted = &settled["attempts"][0];
    assert_eq!(interrupted["status"], "interrupted", "{settled}");
    assert_eq!(interrupted["costEstimated"], true, "{settled}");
    let cost = interrupted["costUsd"].as_f64().unwrap();
    assert!((cost - estimated).abs() < 1e-9, "{cost} vs {estimated}");
    assert_eq!(interrupted["usage"]["output"], 2_100, "{settled}");
    // The retry is a fresh session: its own $0.20 is added to the interrupted
    // run's estimate.
    let retried = &settled["attempts"][1];
    assert!(retried.get("costEstimated").is_none());
    assert_eq!(retried["costUsd"], 0.2, "{settled}");
    let total = settled["costUsd"].as_f64().unwrap();
    assert!((total - (0.2 + estimated)).abs() < 1e-9, "{total}");
    let _ = call("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(second, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}

/// One Opus message as `claude -p --output-format stream-json` streams it,
/// with 10 input, 20k cache-write and 500 output tokens: $0.11004 at list
/// prices (4 / 5 / 20 per MTok).
const PRICED_MESSAGE: &str = r#"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_side","type":"message","role":"assistant","content":[],"usage":{"input_tokens":10,"cache_creation_input_tokens":20000,"cache_read_input_tokens":0,"output_tokens":500}},"parent_tool_use_id":null,"session_id":"sess-side"}"#;

const PRICED_MESSAGE_COST: f64 = (10.0 * 4.0 + 20_000.0 * 5.0 + 500.0 * 20.0) / 1e6;

/// Implements for $0.01 and reviews PASS for $0.05, except that the first
/// run whose brief matches `$HANG_ON` (a `case` pattern, unquoted) streams
/// [`PRICED_MESSAGE`] and hangs with no `result`. Only the second implement
/// run creates SECOND. Each run appends its kind to `$LOG_DIR/runs.log`.
fn side_run_hang_script() -> String {
    format!(
        r###"#!/bin/sh
brief=$(cat)
case "$brief" in
$HANG_ON)
  if [ ! -f "$LOG_DIR/hung" ]; then
    touch "$LOG_DIR/hung"
    printf '%s\n' '{PRICED_MESSAGE}'
    sleep 20
    exit 1
  fi ;;
esac
case "$brief" in
"## Review"*)
  echo review >> "$LOG_DIR/runs.log"
  printf '%s\n' '{{"type":"result","total_cost_usd":0.05,"result":"```sushi-review\n{{\"verdict\":\"PASS\",\"findings\":[]}}\n```"}}' ;;
*"An implement attempt at this task failed"*)
  echo advisor >> "$LOG_DIR/runs.log"
  printf '%s\n' '{{"type":"result","total_cost_usd":0.03,"result":"Create SECOND."}}' ;;
*)
  echo implement >> "$LOG_DIR/runs.log"
  if [ -f FIRST ]; then echo x > SECOND; else echo x > FIRST; fi
  printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-fake"}}'
  printf '%s\n' '{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":1,"output_tokens":1}},"result":"```sushi-report\n{{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}}\n```"}}' ;;
esac
"###
    )
}

/// Runs `task` on [`side_run_hang_script`] with `settings.review = review`,
/// kills the daemon once the hung run's `runs/<events>` has its message on
/// disk, restarts it and returns the settled task and the runs log.
fn kill_during_side_run_and_restart(
    hang_on: &str,
    review: &str,
    task: serde_json::Value,
    events: &str,
) -> (serde_json::Value, Vec<String>) {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &side_run_hang_script(),
    );
    let log_dir = scripts_dir.path().to_str().unwrap();
    let env = [
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("LOG_DIR", log_dir),
        ("HANG_ON", hang_on),
    ];
    let data_holder = tempfile::tempdir().unwrap();
    let data = data_holder.path().to_path_buf();

    let socket1 = data.join("orchd1.sock");
    let mut first = spawn_orchd_raw(&data, &socket1, &env);
    wait_for_socket(&socket1);
    let token = read_control_token(&data);
    let call1 = |m: &str, p: serde_json::Value| request_on(&socket1, m, p, Some(&token));
    let mut settings = call1("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!(review);
    fit_sandbox(&mut settings);
    settings["briefCheckRoute"] = serde_json::json!("");
    settings["answerPolicy"] = serde_json::json!(false);
    call1("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let mut params = task;
    params["repo"] = serde_json::json!(repo.path().to_str().unwrap());
    params["start"] = serde_json::json!(true);
    let created = call1("task.create", params);
    let id = created["id"].as_str().unwrap().to_string();
    let events = data.join("tasks").join(&id).join("runs").join(events);
    let start = Instant::now();
    while !std::fs::read_to_string(&events).is_ok_and(|t| t.contains("msg_side")) {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the side run never streamed its message: {}",
            call1("task.get", serde_json::json!({"id": id}))
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    first.kill().unwrap();
    let _ = first.wait();

    let socket2 = data.join("orchd2.sock");
    let second = spawn_orchd_raw(&data, &socket2, &env);
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
            start.elapsed() < Duration::from_secs(30),
            "task did not finish after restart: {t}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    let runs = std::fs::read_to_string(scripts_dir.path().join("runs.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let _ = call("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(second, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(created["worktree"].as_str().unwrap());
    (settled, runs)
}

#[test]
fn a_daemon_restart_mid_review_keeps_the_review_s_estimated_cost() {
    let (settled, runs) = kill_during_side_run_and_restart(
        "## Review*",
        "claude-opus",
        serde_json::json!({"title": "Reviewed", "goal": "g", "verify": ["true"]}),
        "1/review/events.jsonl",
    );
    let interrupted = &settled["attempts"][0];
    assert_eq!(interrupted["status"], "interrupted", "{settled}");
    let review = interrupted["reviewCostUsd"].as_f64().unwrap();
    assert!((review - PRICED_MESSAGE_COST).abs() < 1e-9, "{settled}");
    assert_eq!(interrupted["costUsd"], 0.01, "{settled}");
    // The retry is a fresh session with its own $0.01, and its own review
    // adds $0.05.
    assert_eq!(runs, ["implement", "implement", "review"], "{settled}");
    let total = settled["costUsd"].as_f64().unwrap();
    let expected = 0.01 + PRICED_MESSAGE_COST + 0.01 + 0.05;
    assert!((total - expected).abs() < 1e-9, "{total} vs {expected}");
}

#[test]
fn a_daemon_restart_mid_advisor_keeps_the_advisor_s_estimated_cost() {
    let (settled, runs) = kill_during_side_run_and_restart(
        "*An implement attempt at this task failed*",
        "",
        serde_json::json!({"title": "Advised", "goal": "g", "verify": ["test -f SECOND"],
            "variant": {"advisor": true}}),
        "1/advisor/events.jsonl",
    );
    let failed = &settled["attempts"][0];
    let advisor = failed["advisorCostUsd"].as_f64().unwrap();
    assert!((advisor - PRICED_MESSAGE_COST).abs() < 1e-9, "{settled}");
    // A failure whose advisor run was paid for is not advised again.
    assert_eq!(runs, ["implement", "implement"], "{settled}");
    assert!(failed.get("advice").is_none(), "{settled}");
    let total = settled["costUsd"].as_f64().unwrap();
    let expected = 0.02 + PRICED_MESSAGE_COST;
    assert!((total - expected).abs() < 1e-9, "{total} vs {expected}");
}

#[test]
fn stopping_a_task_mid_review_keeps_the_review_s_estimated_cost() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &side_run_hang_script(),
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("LOG_DIR", scripts_dir.path().to_str().unwrap()),
        ("HANG_ON", "## Review*"),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "title": "Stopped",
            "goal": "g", "verify": ["true"], "start": true}),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let events = daemon
        .data_dir()
        .join("tasks")
        .join(&task_id)
        .join("runs/1/review/events.jsonl");
    let start = Instant::now();
    while !std::fs::read_to_string(&events).is_ok_and(|t| t.contains("msg_side")) {
        assert!(start.elapsed() < Duration::from_secs(20), "no review ran");
        std::thread::sleep(Duration::from_millis(50));
    }
    daemon.request("task.stop", serde_json::json!({"id": task_id}));
    let stopped = poll_until(&daemon, &task_id, Duration::from_secs(10), |s| {
        s == "stopped"
    });
    let review = stopped["attempts"][0]["reviewCostUsd"].as_f64().unwrap();
    assert!((review - PRICED_MESSAGE_COST).abs() < 1e-9, "{stopped}");
    let total = stopped["costUsd"].as_f64().unwrap();
    assert!(
        (total - (0.01 + PRICED_MESSAGE_COST)).abs() < 1e-9,
        "{stopped}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn a_task_over_its_dollar_budget_waits_before_the_next_attempt_and_raise_continues_it() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("first-run");
    // Every run costs $0.01; only the second one makes verify pass.
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\nif [ -f {m} ]; then echo pass > PASS.txt; fi\ntouch {m}\necho changed > CHANGED_MARKER.txt\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}}'\necho '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"done\"}}'\n",
        m = marker.display()
    );
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Budget case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["test -f PASS.txt"],
            "variant": {"maxCostUsd": 0.008},
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting" || s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(waiting["status"], "waiting", "task JSON: {waiting}");
    assert_eq!(
        waiting["attempts"].as_array().unwrap().len(),
        1,
        "{waiting}"
    );
    assert_eq!(waiting["attempts"][0]["failure"]["kind"], "verify");
    let question = waiting["question"]["text"].as_str().unwrap();
    assert!(question.contains("$0.01 budget"), "{question}");
    assert_eq!(
        waiting["question"]["options"],
        serde_json::json!(["raise", "stop"])
    );
    assert!(
        waiting["decisions"]
            .to_string()
            .contains("Orchestrator: budget $0.01 reached ($0.01 spent)"),
        "{waiting}"
    );

    daemon.request(
        "task.answer",
        serde_json::json!({"id": task_id, "answer": "raise"}),
    );
    // Right after the answer the task can still read `waiting`.
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(settled["budgetRaises"], 1);
    assert!(settled["decisions"].to_string().contains("Owner: raise"));

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Logs every planner run to `$MSG_DIR/plan-runs`; the first one costs $0.01
/// and returns no plan block, every later one drafts the task.
const UNPARSEABLE_FIRST_PLAN_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    if [ -f "$MSG_DIR/plan-runs" ]; then
      json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"true\"],\"questions\":[]}\n```"}'
    else
      json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"no fenced block here, sorry"}'
    fi
    echo run >> "$MSG_DIR/plan-runs"
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
fn a_plan_run_that_reaches_the_budget_waits_before_its_retry() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-planner.sh",
        UNPARSEABLE_FIRST_PLAN_SCRIPT,
    );
    let msg_dir = scripts_dir.path().to_str().unwrap().to_string();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("MSG_DIR", msg_dir.as_str()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": true,
            "variant": {"maxCostUsd": 0.008},
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let plan_runs = scripts_dir.path().join("plan-runs");
    let count_plan_runs = || {
        std::fs::read_to_string(&plan_runs)
            .unwrap_or_default()
            .lines()
            .count()
    };

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting" || s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(waiting["status"], "waiting", "task JSON: {waiting}");
    let question = waiting["question"]["text"].as_str().unwrap();
    assert!(question.contains("before the plan run"), "{question}");
    assert_eq!(count_plan_runs(), 1, "the retry must not run: {waiting}");
    let attempts = waiting["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1, "{waiting}");
    assert_eq!(attempts[0]["status"], "failed");
    assert_eq!(attempts[0]["costUsd"], 0.01);

    daemon.request(
        "task.answer",
        serde_json::json!({"id": task_id, "answer": "raise"}),
    );
    // The new plan run brings the spend to the raised budget, so the
    // implement attempt asks again.
    let again = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting" && count_plan_runs() == 2
    });
    assert!(
        again["question"]["text"]
            .as_str()
            .unwrap()
            .contains("before the implement run"),
        "{again}"
    );
    assert_eq!(again["title"], "Add dark mode");
    daemon.request(
        "task.answer",
        serde_json::json!({"id": task_id, "answer": "raise"}),
    );

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(count_plan_runs(), 2);
    assert_eq!(settled["budgetRaises"], 2);

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Implements like the other fakes; as reviewer, answers PASS and reports a
/// cost of its own via `total_cost_usd`.
const FAKE_PRICED_REVIEWER_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Review"*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.05,"result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*)
  echo changed > CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

#[test]
fn review_cost_lands_on_the_attempt_s_review_cost_usd_and_the_task_total() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-priced-reviewer.sh",
        FAKE_PRICED_REVIEWER_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Priced review",
            "goal": "Write the marker",
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");
    let attempt = &settled["attempts"][0];
    assert_eq!(attempt["review"]["verdict"], "PASS", "{settled}");
    // The review's cost sits on the implement attempt's own reviewCostUsd,
    // never folded into that attempt's costUsd.
    assert_eq!(attempt["reviewCostUsd"], 0.05, "{settled}");
    assert_eq!(attempt["costUsd"], 0.01, "{settled}");
    assert!(
        (settled["costUsd"].as_f64().unwrap() - 0.06).abs() < 1e-9,
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn an_attempt_streaming_past_its_cap_is_stopped_as_budget_and_retried() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("first-run");
    // The first run streams a message worth $0.20 (10k Opus output tokens)
    // and hangs with no `result`; only the cap can end it. The retry passes.
    let line = r#"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_1","type":"message","role":"assistant","content":[],"usage":{"input_tokens":1,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":10000}},"parent_tool_use_id":null,"session_id":"sess-fake"}"#;
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\nif [ ! -f {m} ]; then touch {m}\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}}'\necho '{line}'\nsleep 30; fi\necho pass > PASS.txt\necho changed > CHANGED_MARKER.txt\necho '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"done\"}}'\n",
        m = marker.display()
    );
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Attempt cap",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["test -f PASS.txt"],
            "variant": {"maxAttemptCostUsd": 0.05},
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(20), |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{settled}");
    assert_eq!(attempts[0]["failure"]["kind"], "budget", "{settled}");
    assert_eq!(attempts[0]["costEstimated"], true, "{settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}
