//! Black-box integration tests (brief consistency check): a fake harness
//! answers the check run, the planner and the implementer.

mod common;

use common::*;
use std::time::Duration;

/// Answers by what it is asked: the check prompt gets `$CHECK_RESULT`, a
/// planner brief a draft, anything else an implementation. Every kind of
/// call is appended to `$CALLS_LOG`.
const FAKE_CHECKED_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *"You check a coding task brief"*)
    echo check >> "$CALLS_LOG"
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-check"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"'"$CHECK_RESULT"'"}'
    ;;
  *sushi-plan*)
    echo plan >> "$CALLS_LOG"
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"tier\":\"hard\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"true\"],\"questions\":[]}\n```"}'
    ;;
  *)
    echo implement >> "$CALLS_LOG"
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    ;;
esac
"#;

const CONTRADICTION: &str =
    r#"{\"contradiction\": true, \"conflict\": \"keep the API and remove the API\"}"#;

const CLEAR_RESULT: &str = r#"{\"contradiction\": false, \"conflict\": \"\"}"#;

struct Run {
    task: serde_json::Value,
    calls: Vec<String>,
    daemon: Daemon,
    _scripts: tempfile::TempDir,
    _repo: tempfile::TempDir,
}

fn run(check_result: &str, route: Option<&str>, create: serde_json::Value) -> Run {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-checked.sh", FAKE_CHECKED_SCRIPT);
    let log = scripts.path().join("calls.log");
    std::fs::write(&log, "").unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("CHECK_RESULT", check_result),
        ("CALLS_LOG", log.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["tiers"]["mechanical"] = serde_json::json!("claude-sonnet");
    settings["briefCheckRoute"] = serde_json::json!(route.unwrap_or("claude-sonnet"));
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let mut params = create;
    params["repo"] = serde_json::json!(repo.path().to_str().unwrap());
    params["start"] = serde_json::json!(true);
    let created = daemon.request("task.create", params);
    let id = created["id"].as_str().unwrap().to_string();
    let task = poll_until(&daemon, &id, Duration::from_secs(20), |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    let calls = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    Run {
        task,
        calls,
        daemon,
        _scripts: scripts,
        _repo: repo,
    }
}

fn planned() -> serde_json::Value {
    serde_json::json!({"request": "add dark mode to the settings screen"})
}

fn explicit() -> serde_json::Value {
    serde_json::json!({
        "title": "Explicit", "goal": "Ship the change",
        "criteria": ["Keep the API", "Remove the API"], "verify": ["true"],
    })
}

fn decisions(task: &serde_json::Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

fn count(calls: &[String], kind: &str) -> usize {
    calls.iter().filter(|c| *c == kind).count()
}

#[test]
fn a_contradicting_planner_draft_is_redrafted_once_and_never_twice() {
    let r = run(CONTRADICTION, None, planned());
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(count(&r.calls, "plan"), 2, "calls: {:?}", r.calls);
    assert_eq!(count(&r.calls, "check"), 2, "calls: {:?}", r.calls);
    let lines = decisions(&r.task);
    assert!(
        lines.iter().any(|l| l.contains("(drafting again)")),
        "{lines:?}"
    );
    assert!(lines
        .iter()
        .any(|l| l == "Brief check: keep the API and remove the API"));
    r.daemon.shutdown_and_wait();
}

#[test]
fn explicit_criteria_run_with_the_conflict_in_the_decisions_and_the_brief() {
    let r = run(CONTRADICTION, None, explicit());
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(r.calls, ["check", "implement"]);
    assert!(decisions(&r.task)
        .iter()
        .any(|l| l == "Brief check: keep the API and remove the API"));
    let id = r.task["id"].as_str().unwrap();
    let brief = run_file(&r.daemon, id, "brief.md");
    assert!(brief.contains("## Brief conflict"), "{brief}");
    assert!(brief.contains("keep the API and remove the API"));
    r.daemon.shutdown_and_wait();
}

#[test]
fn a_false_answer_only_adds_a_line() {
    let r = run(
        r#"{\"contradiction\": false, \"conflict\": \"\"}"#,
        None,
        explicit(),
    );
    assert_eq!(r.task["status"], "done");
    assert_eq!(r.calls, ["check", "implement"]);
    assert!(decisions(&r.task)
        .iter()
        .any(|l| l.starts_with("Brief check: no contradiction")));
    let id = r.task["id"].as_str().unwrap();
    assert!(!run_file(&r.daemon, id, "brief.md").contains("Brief conflict"));
    r.daemon.shutdown_and_wait();
}

#[test]
fn an_unparseable_reply_only_adds_a_line() {
    let r = run("not json at all", None, planned());
    assert_eq!(r.task["status"], "done");
    assert_eq!(r.calls, ["plan", "check", "implement"]);
    assert!(decisions(&r.task)
        .iter()
        .any(|l| l.starts_with("Brief check: no usable answer")));
    r.daemon.shutdown_and_wait();
}

#[test]
fn an_empty_route_turns_the_check_off() {
    let r = run(CONTRADICTION, Some(""), explicit());
    assert_eq!(r.task["status"], "done");
    assert_eq!(r.calls, ["implement"]);
    assert!(!decisions(&r.task)
        .iter()
        .any(|l| l.starts_with("Brief check")));
    r.daemon.shutdown_and_wait();
}

#[test]
fn a_route_that_names_nothing_falls_back_to_the_cheapest_available_route() {
    let r = run(CLEAR_RESULT, Some("claude-haiku"), explicit());
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(count(&r.calls, "check"), 1, "calls: {:?}", r.calls);
    let lines = decisions(&r.task);
    assert!(
        lines.iter().any(|l| l
            == "Brief check: route claude-haiku is not configured or unavailable; using the cheapest available route claude-sonnet"),
        "{lines:?}"
    );
    r.daemon.shutdown_and_wait();
}
