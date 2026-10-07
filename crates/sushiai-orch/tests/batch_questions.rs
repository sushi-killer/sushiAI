//! Black-box integration tests (batchQuestions): spawn the real `orchd` binary
//! and drive it over its NDJSON unix socket.

mod common;

use common::*;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

/// A fake Claude that plans with `plan` (written next to the script), answers
/// triage by escalating with a sharpened question, and implements trivially.
fn setup(plan: serde_json::Value, dir: &Path) -> std::path::PathBuf {
    let result = json!({
        "type": "result",
        "total_cost_usd": 0.01,
        "usage": {"input_tokens": 1, "output_tokens": 1},
        "result": format!("```sushi-plan\n{plan}\n```"),
    });
    std::fs::write(dir.join("plan.jsonl"), format!("{result}\n")).unwrap();
    let body = format!(
        r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-plan"}}'
    cat {d}/plan.jsonl
    ;;
  *sushi-triage*)
    printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-triage"}}'
    printf '%s\n' '{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":1,"output_tokens":1}},"result":"```sushi-triage\n{{\"action\":\"escalate\",\"question\":\"Sharpened one\",\"options\":[\"x\",\"y\"],\"reason\":\"needs the owner\"}}\n```"}}'
    ;;
  *)
    echo changed > CHANGED_MARKER.txt
    printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-fake"}}'
    printf '%s\n' '{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":1,"output_tokens":1}},"result":"```sushi-report\n{{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}}\n```"}}'
    ;;
esac
"#,
        d = dir.display()
    );
    fake_harness_script(dir, "fake-planner.sh", &body)
}

fn plan(questions: serde_json::Value) -> serde_json::Value {
    json!({
        "title": "Add dark mode",
        "goal": "Add a dark theme toggle",
        "tier": "standard",
        "criteria": ["Toggle visible in settings"],
        "verify": ["true"],
        "questions": questions,
    })
}

fn spawn(script: &Path, auto_answer: bool) -> Daemon {
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["autoAnswer"] = json!(auto_answer);
    daemon.request("settings.set", json!({"settings": settings}));
    daemon
}

fn create(daemon: &Daemon, repo: &Path, flag: bool, start: bool) -> serde_json::Value {
    daemon.request(
        "task.create",
        json!({
            "repo": repo.to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "variant": {"batchQuestions": flag},
            "start": start,
        }),
    )
}

fn finished(s: &str) -> bool {
    matches!(s, "done" | "failed" | "stopped" | "waiting")
}

#[test]
fn non_blocking_questions_become_assumptions_and_never_wait() {
    let dir = tempfile::tempdir().unwrap();
    let script = setup(
        plan(json!([
            {"text": "Which default theme?", "options": ["light", "dark"], "recommended": "dark", "evidence": "the app is dark already", "blocking": false},
            {"text": "Where does the toggle live?", "options": ["settings", "header"], "recommended": "settings", "evidence": "settings holds prefs", "blocking": false},
        ])),
        dir.path(),
    );
    let daemon = spawn(&script, false);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), true, true);
    let id = task["id"].as_str().unwrap().to_string();
    let settled = poll_until(&daemon, &id, Duration::from_secs(20), finished);
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let assumptions = settled["assumptions"].as_array().unwrap();
    assert_eq!(assumptions.len(), 2, "{settled}");
    assert_eq!(assumptions[0]["question"], "Which default theme?");
    assert_eq!(assumptions[0]["answer"], "dark");
    assert_eq!(assumptions[0]["evidence"], "the app is dark already");
    assert_eq!(assumptions[0]["by"], "planner");
    assert_eq!(assumptions[0]["overturned"], false);
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}

#[test]
fn without_the_flag_every_question_is_still_asked() {
    let dir = tempfile::tempdir().unwrap();
    let script = setup(
        plan(json!([
            {"text": "Which default theme?", "options": ["light", "dark"], "recommended": "dark", "evidence": "e", "blocking": false},
        ])),
        dir.path(),
    );
    let daemon = spawn(&script, false);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), false, true);
    let id = task["id"].as_str().unwrap().to_string();
    let waiting = poll_until(&daemon, &id, Duration::from_secs(20), finished);
    assert_eq!(waiting["status"], "waiting", "{waiting}");
    assert_eq!(waiting["question"]["text"], "Which default theme?");
    assert!(waiting.get("assumptions").is_none(), "{waiting}");
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}

#[test]
fn two_blocking_questions_escalated_by_triage_become_one_owner_question() {
    let dir = tempfile::tempdir().unwrap();
    let script = setup(
        plan(json!([
            {"text": "Drop the old table?", "options": ["yes", "no"], "recommended": "no", "evidence": "data loss", "blocking": true},
            {"text": "Rename the public endpoint?", "options": ["yes", "no"], "recommended": "no", "evidence": "API contract", "blocking": true},
            {"text": "Which colour?", "options": ["red", "blue"], "recommended": "blue", "evidence": "brand", "blocking": false},
        ])),
        dir.path(),
    );
    let daemon = spawn(&script, true);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), true, true);
    let id = task["id"].as_str().unwrap().to_string();
    let waiting = poll_until(&daemon, &id, Duration::from_secs(20), finished);
    assert_eq!(waiting["status"], "waiting", "{waiting}");
    let text = waiting["question"]["text"].as_str().unwrap();
    assert!(text.contains("Drop the old table?"), "{text}");
    assert!(text.contains("Rename the public endpoint?"), "{text}");
    assert!(text.contains("recommended: no"), "{text}");
    assert!(!text.contains("Sharpened one"), "{text}");
    assert!(!text.contains("Which colour?"), "{text}");
    assert_eq!(waiting["assumptions"].as_array().unwrap().len(), 1);

    daemon.request("task.answer", json!({"id": id, "answer": "no to both"}));
    let settled = poll_until(&daemon, &id, Duration::from_secs(20), |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    assert_eq!(settled["status"], "done", "{settled}");
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}

#[test]
fn overturn_marks_the_assumption_and_reaches_the_next_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let script = setup(
        plan(json!([
            {"text": "Which default theme?", "options": ["light", "dark"], "recommended": "dark", "evidence": "e", "blocking": false},
        ])),
        dir.path(),
    );
    let daemon = spawn(&script, false);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), true, false);
    let id = task["id"].as_str().unwrap().to_string();
    let stopped = poll_until(&daemon, &id, Duration::from_secs(20), finished);
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    assert_eq!(stopped["assumptions"].as_array().unwrap().len(), 1);

    let overturned = daemon.request(
        "task.overturn",
        json!({"id": id, "index": 0, "answer": "light with a sepia tint"}),
    );
    assert_eq!(overturned["assumptions"][0]["overturned"], true);
    assert_eq!(
        overturned["assumptions"][0]["ownerAnswer"],
        "light with a sepia tint"
    );

    daemon.request("task.start", json!({"id": id}));
    let settled = poll_until(&daemon, &id, Duration::from_secs(20), |s| {
        matches!(s, "done" | "failed")
    });
    assert_eq!(settled["status"], "done", "{settled}");
    let n = settled["attempts"].as_array().unwrap().last().unwrap()["n"].clone();
    let brief = std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(&id)
            .join(format!("runs/{n}/brief.md")),
    )
    .unwrap();
    assert!(
        brief.contains("light with a sepia tint"),
        "the brief carries the owner's answer: {brief}"
    );

    assert!(
        brief.contains("The owner overturned an assumption"),
        "{brief}"
    );

    // Once done, an overturn only records: nothing is queued for anyone.
    let before = daemon.request("message.inbox", json!({"id": id}));
    daemon.request(
        "task.overturn",
        json!({"id": id, "index": 0, "answer": "dark after all"}),
    );
    let after = daemon.request("message.inbox", json!({"id": id}));
    assert_eq!(before, after);
    let latest = daemon.request("task.get", json!({"id": id}));
    assert_eq!(latest["assumptions"][0]["ownerAnswer"], "dark after all");
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}
