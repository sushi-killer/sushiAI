//! Black-box integration tests (plan gate): a fake harness plays the planner,
//! the plan reviewer and the implementer, and logs every kind of call.

mod common;

use common::*;
use std::time::Duration;

/// Answers by what it is asked. A planner brief that carries the gate's
/// findings gets `$SECOND_DRAFT`, any other `$FIRST_DRAFT`; the review prompt
/// gets `$REVIEW_RESULT`. Every call is appended to `$CALLS_LOG`; the
/// planner's briefs are kept as `$CALLS_LOG.plan<N>`.
const FAKE_GATED_SCRIPT: &str = r###"#!/bin/sh
input="$(cat)"
emit() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"'"$1"'"}'
}
case "$input" in
  *"You review a coding task plan"*)
    echo review >> "$CALLS_LOG"
    emit "$REVIEW_RESULT"
    ;;
  *sushi-plan*)
    case "$input" in
      *"## Plan gate findings"*)
        echo replan >> "$CALLS_LOG"
        printf '%s' "$input" > "$CALLS_LOG.replan"
        emit '```sushi-plan\n'"$SECOND_DRAFT"'\n```'
        ;;
      *)
        echo plan >> "$CALLS_LOG"
        emit '```sushi-plan\n'"$FIRST_DRAFT"'\n```'
        ;;
    esac
    ;;
  *)
    echo implement >> "$CALLS_LOG"
    printf '%s' "$input" > "$CALLS_LOG.implement"
    echo "changed" > CHANGED_MARKER.txt
    emit '```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```'
    ;;
esac
"###;

const NO_FINDINGS: &str = r#"{\"findings\": []}"#;

const CLEAN_DRAFT: &str = r#"{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"tier\":\"hard\",\"criteria\":[\"Toggle visible in settings -- check: sh -c true\"],\"verify\":[\"true\"],\"questions\":[]}"#;

const REPORT_ONLY_DRAFT: &str = r#"{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"tier\":\"hard\",\"criteria\":[\"The implementer's handoff lists the changed files\",\"Toggle visible in settings -- check: sh -c true\"],\"verify\":[\"true\"],\"questions\":[]}"#;

struct Run {
    task: serde_json::Value,
    calls: Vec<String>,
    log: std::path::PathBuf,
    daemon: Daemon,
    _scripts: tempfile::TempDir,
    _repo: tempfile::TempDir,
}

fn run(first: &str, second: &str, review_route: &str, review_result: &str) -> Run {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-gated.sh", FAKE_GATED_SCRIPT);
    let log = scripts.path().join("calls.log");
    std::fs::write(&log, "").unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("CALLS_LOG", log.to_str().unwrap()),
        ("FIRST_DRAFT", first),
        ("SECOND_DRAFT", second),
        ("REVIEW_RESULT", review_result),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["tiers"]["hard"] = serde_json::json!("claude-sonnet");
    settings["planner"] = serde_json::json!("claude-sonnet");
    settings["planReview"] = serde_json::json!(review_route);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let created = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": true,
        }),
    );
    let id = created["id"].as_str().unwrap().to_string();
    let task = poll_until(&daemon, &id, Duration::from_secs(30), |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    let calls = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    Run {
        task,
        calls,
        log,
        daemon,
        _scripts: scripts,
        _repo: repo,
    }
}

fn count(calls: &[String], kind: &str) -> usize {
    calls.iter().filter(|c| *c == kind).count()
}

fn decisions(task: &serde_json::Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

fn criteria(task: &serde_json::Value) -> Vec<String> {
    task["criteria"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_report_only_criterion_triggers_one_replan_and_is_gone_after_it() {
    // The second draft still carries it: the replan is the only one.
    let r = run(REPORT_ONLY_DRAFT, REPORT_ONLY_DRAFT, "", NO_FINDINGS);
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(
        count(&r.calls, "plan") + count(&r.calls, "replan"),
        2,
        "calls: {:?}",
        r.calls
    );
    assert_eq!(count(&r.calls, "replan"), 1, "calls: {:?}", r.calls);
    let brief = std::fs::read_to_string(format!("{}.replan", r.log.display())).unwrap();
    assert!(brief.contains("## Plan gate findings"), "{brief}");
    assert!(brief.contains("report-only"), "{brief}");
    let stored = criteria(&r.task);
    assert!(
        !stored.iter().any(|c| c.contains("handoff")),
        "criteria: {stored:?}"
    );
    assert_eq!(stored.len(), 1, "criteria: {stored:?}");
    let lines = decisions(&r.task);
    assert!(
        lines.iter().any(|l| l.starts_with("Plan gate:")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Plan gate: removed the report-only criterion")),
        "{lines:?}"
    );
    // The request is never edited by the replan.
    assert_eq!(
        r.task["request"], "add dark mode to the settings screen",
        "{}",
        r.task
    );
    r.daemon.shutdown_and_wait();
}

#[test]
fn a_critical_plan_review_finding_triggers_one_replan_and_no_second_review() {
    let review = r#"{\"findings\": [{\"severity\": \"CRITICAL\", \"kind\": \"missing requirement\", \"text\": \"Nothing persists the choice\"}]}"#;
    let r = run(CLEAN_DRAFT, CLEAN_DRAFT, "claude-sonnet", review);
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(count(&r.calls, "review"), 1, "calls: {:?}", r.calls);
    assert_eq!(count(&r.calls, "replan"), 1, "calls: {:?}", r.calls);
    assert_eq!(count(&r.calls, "plan"), 1, "calls: {:?}", r.calls);
    let brief = std::fs::read_to_string(format!("{}.replan", r.log.display())).unwrap();
    assert!(brief.contains("Nothing persists the choice"), "{brief}");
    assert_eq!(r.task["planGate"]["replanned"], true, "{}", r.task);
    assert_eq!(r.task["planGate"]["reviewed"], true, "{}", r.task);
    r.daemon.shutdown_and_wait();
}

#[test]
fn a_low_finding_reaches_the_implementer_brief() {
    let review = r#"{\"findings\": [{\"severity\": \"low\", \"kind\": \"wrong approach\", \"text\": \"Prefer the existing theme hook\"}]}"#;
    let r = run(CLEAN_DRAFT, CLEAN_DRAFT, "claude-sonnet", review);
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(count(&r.calls, "replan"), 0, "calls: {:?}", r.calls);
    let findings = r.task["planGate"]["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f["text"] == "Prefer the existing theme hook" && f["severity"] == "LOW"),
        "{}",
        r.task
    );
    let brief = std::fs::read_to_string(format!("{}.implement", r.log.display())).unwrap();
    assert!(brief.contains("## Plan notes"), "{brief}");
    assert!(brief.contains("Prefer the existing theme hook"), "{brief}");
    r.daemon.shutdown_and_wait();
}

#[test]
fn an_empty_plan_review_setting_skips_the_review() {
    let r = run(CLEAN_DRAFT, CLEAN_DRAFT, "", NO_FINDINGS);
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(count(&r.calls, "review"), 0, "calls: {:?}", r.calls);
    let lines = decisions(&r.task);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Plan gate:") && l.contains("review is off")),
        "{lines:?}"
    );
    r.daemon.shutdown_and_wait();
}

#[test]
fn a_review_route_that_does_not_exist_is_skipped_without_a_fallback() {
    let r = run(CLEAN_DRAFT, CLEAN_DRAFT, "no-such-route", NO_FINDINGS);
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(count(&r.calls, "review"), 0, "calls: {:?}", r.calls);
    let lines = decisions(&r.task);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Plan gate:") && l.contains("no-such-route")),
        "{lines:?}"
    );
    r.daemon.shutdown_and_wait();
}
