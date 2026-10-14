//! Black-box integration tests (first-pass metrics): the metrics a finished
//! task gets, the `done` entry in the overseer log, and late planning of a
//! backlog request. Real daemon, real git.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

/// One fake for every Claude role: a planner brief gets a plan, a review
/// brief ("## Review...") the verdict line in `$LOG_DIR/review.line`, and an
/// implement run touches a file and reports done. Other briefs (triage) get
/// a short reply.
const SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
*sushi-plan*)
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.02,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"tier\":\"hard\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"true\"],\"questions\":[]}\n```"}' ;;
"An implement attempt at this task failed"*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"Fix it."}' ;;
"## Review"*)
  cat "$LOG_DIR/review.line" ;;
*)
  echo changed >> CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

const PASS_LINE: &str = r#"{"type":"result","total_cost_usd":0.005,"result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}"#;
const FAIL_LINE: &str = r#"{"type":"result","total_cost_usd":0.005,"result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"P1: a.rs:1 - null check missing\"]}\n```"}"#;

struct Rig {
    daemon: Daemon,
    _dir: tempfile::TempDir,
    repo: tempfile::TempDir,
}

fn rig(review_line: &str, review: &str, max_attempts: u32) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(dir.path(), "fake-claude.sh", SCRIPT);
    std::fs::write(dir.path().join("review.line"), format!("{review_line}\n")).unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("LOG_DIR", dir.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["planReview"] = json!("");
    settings["review"] = json!(review);
    settings["maxAttempts"] = json!(max_attempts);
    settings["answerPolicy"] = json!(false);
    daemon.request("settings.set", json!({"settings": settings}));
    Rig {
        daemon,
        _dir: dir,
        repo: init_git_repo(),
    }
}

impl Rig {
    fn create(&self, extra: Value) -> Value {
        let mut params = json!({
            "repo": self.repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
        });
        for (k, v) in extra.as_object().unwrap() {
            params[k] = v.clone();
        }
        self.daemon.request("task.create", params)
    }

    /// The task once it has metrics (they land just after its status).
    fn with_metrics(&self, id: &str) -> Value {
        wait_until("task metrics", || {
            let task = self.daemon.request("task.get", json!({"id": id}));
            (!task["metrics"].is_null()).then_some(task)
        })
    }

    fn done_entries(&self, id: &str) -> Vec<Value> {
        std::fs::read_to_string(self.daemon.data_dir().join("overseer.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|e| e["taskId"] == id && e["kind"] == "done")
            .collect()
    }
}

#[test]
fn a_landed_first_pass_task_records_its_metrics_and_one_done_entry() {
    let rig = rig(PASS_LINE, "claude-opus", 3);
    let task = rig.create(json!({"start": true}));
    let id = task["id"].as_str().unwrap().to_string();
    let done = poll_until(&rig.daemon, &id, |s| s == "done");
    assert_eq!(done["status"], "done", "{done}");
    let done = rig.with_metrics(&id);
    let m = &done["metrics"];
    assert_eq!(m["firstPass"], true, "{done}");
    assert_eq!(m["ownerTouches"], 0);
    assert!(m.get("failureCause").is_none(), "{m}");
    let cost = m["costByPhase"].as_object().unwrap();
    let phases = [
        "plan",
        "implement",
        "review",
        "triage",
        "advisor",
        "finding_judge",
        "answer_judge",
        "pick",
    ];
    assert_eq!(cost.len(), phases.len(), "{cost:?}");
    for phase in phases {
        assert!(cost[phase].is_number(), "{phase}: {cost:?}");
    }
    assert!(cost["plan"].as_f64().unwrap() > 0.0, "{cost:?}");
    assert!(cost["implement"].as_f64().unwrap() > 0.0, "{cost:?}");

    let entries = rig.done_entries(&id);
    assert_eq!(entries.len(), 1, "{entries:?}");
    let text = entries[0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("first pass: yes; owner touches: 0; cause: none; cost:"),
        "{text}"
    );
    let detail = entries[0]["detail"].as_str().unwrap();
    assert!(detail.starts_with(text), "{detail}");
    assert!(detail.contains("first-pass rate 100% (1/1)"), "{detail}");
    assert!(
        detail.contains("average cost per landed task $"),
        "{detail}"
    );
    assert!(detail.contains("(1 landed)"), "{detail}");

    let worktree = done["worktree"].as_str().unwrap().to_string();
    rig.daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_task_that_fails_after_its_review_rounds_is_a_review_loop_failure() {
    let rig = rig(FAIL_LINE, "claude-opus", 1);
    let task = rig.create(json!({
        "title": "Rounds",
        "goal": "Write the marker",
        "verify": ["true"],
        "request": Value::Null,
        "start": true,
    }));
    let id = task["id"].as_str().unwrap().to_string();
    // The review keeps failing: the ladder changes the approach once, then
    // the task ends `failed`, and its last failure is still the review.
    let failed = poll_until(&rig.daemon, &id, |s| matches!(s, "failed" | "stopped"));
    assert_eq!(failed["status"], "failed", "{failed}");
    assert_eq!(
        failed["attempts"][0]["failure"]["kind"], "review",
        "{failed}"
    );
    let failed = rig.with_metrics(&id);
    assert_eq!(failed["metrics"]["firstPass"], false, "{failed}");
    assert_eq!(failed["metrics"]["failureCause"], "review_loop", "{failed}");
    assert_eq!(failed["metrics"]["ownerTouches"], 0, "{failed}");

    let entries = rig.done_entries(&id);
    assert_eq!(entries.len(), 1, "{entries:?}");
    let text = entries[0]["text"].as_str().unwrap();
    assert!(text.contains("cause: review_loop"), "{text}");
    assert!(text.starts_with("first pass: no;"), "{text}");
    let detail = entries[0]["detail"].as_str().unwrap();
    assert!(detail.contains("first-pass rate 0% (0/1)"), "{detail}");
    assert!(
        detail.contains("average cost per landed task n/a (0 landed)"),
        "{detail}"
    );
    rig.daemon.shutdown_and_wait();
}

fn commit_on_base(repo: &Path, name: &str) -> String {
    std::fs::write(repo.join(name), "later\n").unwrap();
    git_out(repo, &["add", name]);
    git_out(repo, &["commit", "-q", "-m", &format!("add {name}")]);
    git_out(repo, &["rev-parse", "HEAD"]).trim().to_string()
}

#[test]
fn a_backlog_request_is_planned_when_it_starts_on_the_base_as_it_is_then() {
    let rig = rig(PASS_LINE, "", 3);
    let task = rig.create(json!({"backlog": {"bucket": "later"}}));
    let id = task["id"].as_str().unwrap().to_string();
    assert_eq!(task["status"], "queued", "{task}");
    let created_base = task["baseSha"].as_str().unwrap().to_string();

    // Nothing plans while it waits in the backlog.
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let parked = rig.daemon.request("task.get", json!({"id": id}));
    assert_eq!(parked["status"], "queued", "{parked}");
    assert_eq!(parked["attempts"], json!([]), "{parked}");

    let moved = commit_on_base(rig.repo.path(), "later.txt");
    assert_ne!(moved, created_base);
    rig.daemon.request("task.start", json!({"id": id}));
    let done = poll_until(&rig.daemon, &id, |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["attempts"][0]["stage"], "plan", "{done}");
    assert_eq!(done["baseSha"], json!(moved), "{done}");
    let decisions: Vec<&str> = done["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect();
    assert!(
        decisions
            .iter()
            .any(|d| d.starts_with("Rebase: planning from ") && d.contains(&moved[..8])),
        "{decisions:?}"
    );

    let worktree = done["worktree"].as_str().unwrap().to_string();
    rig.daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
