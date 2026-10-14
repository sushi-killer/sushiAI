//! Black-box integration tests (batchQuestions): spawn the real `orchd` binary
//! and drive it over its NDJSON unix socket.

mod common;

use common::*;
use serde_json::json;
use std::path::Path;

/// A fake Claude that plans with `plan` (written next to the script), answers
/// the overseer with a plain note, records any triage run in `triage-ran.txt`
/// (a triage turn must never happen; it would escalate with a sharpened
/// question), and implements trivially.
fn setup(plan: serde_json::Value, dir: &Path) -> std::path::PathBuf {
    let result = json!({
        "type": "result",
        "total_cost_usd": 0.01,
        "usage": {"input_tokens": 1, "output_tokens": 1},
        "result": format!("```sushi-plan\n{plan}\n```"),
    });
    std::fs::write(dir.join("plan.jsonl"), format!("{result}\n")).unwrap();
    triage(
        dir,
        json!({"action": "escalate", "question": "Sharpened one", "options": ["x", "y"], "reason": "needs the owner"}),
    );
    let body = format!(
        r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-plan"}}'
    cat {d}/plan.jsonl
    ;;
  *"is waiting for an answer"*)
    echo overseer >> overseer-ran.txt
    printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-overseer"}}'
    printf '%s\n' '{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":1,"output_tokens":1}},"result":"Overseer note."}}'
    ;;
  *sushi-triage*)
    echo triage >> triage-ran.txt
    printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-triage"}}'
    cat {d}/triage.jsonl
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

/// What the fake triage replies from now on.
fn triage(dir: &Path, decision: serde_json::Value) {
    let result = json!({
        "type": "result",
        "total_cost_usd": 0.01,
        "usage": {"input_tokens": 1, "output_tokens": 1},
        "result": format!("```sushi-triage\n{decision}\n```"),
    });
    std::fs::write(dir.join("triage.jsonl"), format!("{result}\n")).unwrap();
}

/// Answers the routine question `text` as the overseer does.
fn settle_by_overseer(daemon: &Daemon, id: &str, text: &str) {
    wait_for_question(daemon, id, text);
    daemon.request(
        "task.answer",
        json!({"id": id, "answer": "go with the recommendation", "source": "overseer"}),
    );
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
    settings["answerPolicy"] = json!(true);
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

/// Polls until the task waits for `text`, the question the test is up to.
fn wait_for_question(daemon: &Daemon, id: &str, text: &str) -> serde_json::Value {
    for _ in 0..400 {
        let task = daemon.request("task.get", json!({"id": id}));
        if task["status"] == "waiting" && task["question"]["text"] == text {
            return task;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("the task never waited for {text:?}");
}

#[test]
fn non_blocking_questions_are_settled_by_the_overseer_as_assumptions() {
    let dir = tempfile::tempdir().unwrap();
    let script = setup(
        plan(json!([
            {"text": "Which default theme?", "options": ["light", "dark"], "recommended": "dark", "evidence": "the app is dark already", "blocking": false},
            {"text": "Where does the toggle live?", "options": ["settings", "header"], "recommended": "settings", "evidence": "settings holds prefs", "blocking": false},
        ])),
        dir.path(),
    );
    let daemon = spawn(&script, true);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), true, true);
    let id = task["id"].as_str().unwrap().to_string();
    // Each routine question waits for the overseer, which answers it.
    for question in ["Which default theme?", "Where does the toggle live?"] {
        wait_for_question(&daemon, &id, question);
        daemon.request(
            "task.answer",
            json!({"id": id, "answer": "go with the recommendation", "source": "overseer"}),
        );
    }
    // The loop records each answer after the reply; wait for the plan to end.
    let settled = poll_until(&daemon, &id, |s| matches!(s, "done" | "failed" | "stopped"));
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let assumptions = settled["assumptions"].as_array().unwrap();
    assert_eq!(assumptions.len(), 2, "{settled}");
    assert_eq!(assumptions[0]["question"], "Which default theme?");
    assert_eq!(assumptions[0]["answer"], "go with the recommendation");
    assert_eq!(assumptions[0]["evidence"], "the app is dark already");
    assert_eq!(assumptions[0]["by"], "policy");
    assert_eq!(assumptions[0]["overturned"], false);
    assert!(!Path::new(task["worktree"].as_str().unwrap())
        .join("triage-ran.txt")
        .exists());
    daemon.shutdown_and_wait();
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}

#[test]
fn a_non_blocking_question_the_overseer_does_not_settle_is_asked_not_assumed() {
    let dir = tempfile::tempdir().unwrap();
    let script = setup(
        plan(json!([
            {"text": "Also remove the legacy endpoint?", "options": ["yes", "no"], "recommended": "no", "evidence": "e", "blocking": false},
        ])),
        dir.path(),
    );
    let daemon = spawn(&script, false);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), true, true);
    let id = task["id"].as_str().unwrap().to_string();
    let waiting = poll_until(&daemon, &id, finished);
    assert_eq!(waiting["status"], "waiting", "{waiting}");
    assert!(
        waiting["question"]["text"]
            .as_str()
            .unwrap()
            .contains("Also remove the legacy endpoint?"),
        "{waiting}"
    );
    assert!(waiting.get("assumptions").is_none(), "{waiting}");
    daemon.shutdown_and_wait();
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}

#[test]
fn the_overseer_does_not_settle_a_planner_question_while_the_answer_policy_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let script = setup(
        plan(json!([
            {"text": "Which default theme?", "options": ["light", "dark"], "recommended": "dark", "evidence": "e", "blocking": false},
        ])),
        dir.path(),
    );
    let daemon = spawn(&script, true);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["answerPolicy"] = json!(false);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), true, true);
    let id = task["id"].as_str().unwrap().to_string();
    let waiting = poll_until(&daemon, &id, finished);
    assert_eq!(waiting["status"], "waiting", "{waiting}");
    assert!(
        waiting["question"]["text"]
            .as_str()
            .unwrap()
            .contains("Which default theme?"),
        "{waiting}"
    );
    assert!(waiting.get("assumptions").is_none(), "{waiting}");
    daemon.shutdown_and_wait();
    remove_dir_retrying(task["worktree"].as_str().unwrap());
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
    let waiting = poll_until(&daemon, &id, finished);
    assert_eq!(waiting["status"], "waiting", "{waiting}");
    assert_eq!(waiting["question"]["text"], "Which default theme?");
    assert!(waiting.get("assumptions").is_none(), "{waiting}");
    daemon.shutdown_and_wait();
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}

#[test]
fn a_task_created_without_a_variant_assumes_a_non_blocking_question() {
    let dir = tempfile::tempdir().unwrap();
    let script = setup(
        plan(json!([
            {"text": "Which default theme?", "options": ["light", "dark"], "recommended": "dark", "evidence": "e", "blocking": false},
        ])),
        dir.path(),
    );
    let daemon = spawn(&script, true);
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": false,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    settle_by_overseer(&daemon, &id, "Which default theme?");
    let stopped = poll_until(&daemon, &id, |s| matches!(s, "done" | "failed" | "stopped"));
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    assert_eq!(
        stopped["assumptions"][0]["answer"], "go with the recommendation",
        "{stopped}"
    );
    daemon.shutdown_and_wait();
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}

#[test]
fn a_plan_without_a_verify_command_takes_the_first_one_the_repo_offers() {
    let dir = tempfile::tempdir().unwrap();
    let mut empty = plan(json!([]));
    empty["verify"] = json!([]);
    let script = setup(empty, dir.path());
    let daemon = spawn(&script, false);
    let repo = init_git_repo();
    std::fs::write(repo.path().join("Makefile"), "test:\n\ttrue\n").unwrap();
    for args in [vec!["add", "."], vec!["commit", "-qm", "makefile"]] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
    let task = create(&daemon, repo.path(), true, false);
    let id = task["id"].as_str().unwrap().to_string();
    let stopped = poll_until(&daemon, &id, finished);
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    assert_eq!(stopped["verify"], json!(["make test"]), "{stopped}");
    assert_eq!(
        stopped["assumptions"][0]["answer"], "make test",
        "{stopped}"
    );
    daemon.shutdown_and_wait();
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}

#[test]
fn a_plan_without_a_verify_command_asks_when_the_repo_offers_no_whole_suite() {
    let dir = tempfile::tempdir().unwrap();
    let mut empty = plan(json!([]));
    empty["verify"] = json!([]);
    let script = setup(empty, dir.path());
    let daemon = spawn(&script, false);
    let repo = init_git_repo();
    std::fs::write(
        repo.path().join("package.json"),
        r#"{"scripts":{"pretest":"x","test:update-snapshots":"y"}}"#,
    )
    .unwrap();
    for args in [vec!["add", "."], vec!["commit", "-qm", "scripts"]] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
    let task = create(&daemon, repo.path(), true, false);
    let id = task["id"].as_str().unwrap().to_string();
    let waiting = poll_until(&daemon, &id, finished);
    assert_eq!(waiting["status"], "waiting", "{waiting}");
    assert!(
        waiting["question"]["text"]
            .as_str()
            .unwrap()
            .contains("No verification command was found"),
        "{waiting}"
    );
    daemon.shutdown_and_wait();
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}

#[test]
fn two_blocking_questions_become_one_owner_question_with_no_triage_turn() {
    let dir = tempfile::tempdir().unwrap();
    let script = setup(
        plan(json!([
            {"text": "Drop the old table?", "options": ["yes", "no"], "recommended": "no", "evidence": "data loss", "blocking": true},
            {"text": "Rename the public endpoint?", "options": ["yes", "no"], "recommended": "no", "evidence": "API contract", "blocking": true},
            {"text": "Which colour?", "options": ["red", "blue"], "recommended": "blue", "evidence": "brand", "blocking": false},
        ])),
        dir.path(),
    );
    // Without auto-answer nobody settles the routine question: it joins the batch.
    let daemon = spawn(&script, false);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), true, true);
    let id = task["id"].as_str().unwrap().to_string();
    let waiting = poll_until(&daemon, &id, finished);
    assert_eq!(waiting["status"], "waiting", "{waiting}");
    let text = waiting["question"]["text"].as_str().unwrap();
    assert!(text.contains("Drop the old table?"), "{text}");
    assert!(text.contains("Rename the public endpoint?"), "{text}");
    assert!(text.contains("recommended: no"), "{text}");
    assert!(!text.contains("Sharpened one"), "{text}");
    assert!(text.contains("Which colour?"), "{text}");
    assert!(waiting.get("assumptions").is_none(), "{waiting}");
    assert!(!Path::new(task["worktree"].as_str().unwrap())
        .join("triage-ran.txt")
        .exists());

    daemon.request("task.answer", json!({"id": id, "answer": "no to both"}));
    let settled = poll_until(&daemon, &id, |s| matches!(s, "done" | "failed" | "stopped"));
    assert_eq!(settled["status"], "done", "{settled}");
    daemon.shutdown_and_wait();
    remove_dir_retrying(task["worktree"].as_str().unwrap());
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
    let daemon = spawn(&script, true);
    let repo = init_git_repo();
    let task = create(&daemon, repo.path(), true, false);
    let id = task["id"].as_str().unwrap().to_string();
    settle_by_overseer(&daemon, &id, "Which default theme?");
    let stopped = poll_until(&daemon, &id, |s| matches!(s, "done" | "failed" | "stopped"));
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
    let settled = poll_until(&daemon, &id, |s| matches!(s, "done" | "failed"));
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
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}
