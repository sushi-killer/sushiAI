//! Black-box integration tests: the implement brief asks for targeted tests
//! only, and an implement attempt past `implementBudgetMins` is stopped,
//! asked for a short report in its own session, and recorded as
//! `time_budget`.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

/// Writes a file, announces its session and then works for a minute. A
/// resumed call (the report-only turn) replies at once; `$RESUME_EXIT`
/// makes that call fail instead.
const FAKE_SLOW: &str = r###"#!/bin/sh
cat > /dev/null
case "$*" in
  *--resume*)
    if [ -n "$RESUME_EXIT" ]; then echo "resume failed" >&2; exit 1; fi
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"partial\",\"summary\":\"half done\",\"handoff\":\"the parser is rewritten, the writer is not\",\"decisions\":[],\"question\":\"\"}\n```"}'
    exit 0
    ;;
esac
echo changed > BUDGET_MARKER.txt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-slow"}'
sleep 60
"###;

const FAKE_PASS: &str = r###"#!/bin/sh
cat > /dev/null
echo changed > CHANGED_MARKER.txt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"###;

struct Setup {
    daemon: Daemon,
    repo: tempfile::TempDir,
    _scratch: tempfile::TempDir,
}

fn setup(script_body: &str, budget_mins: Option<f64>, extra: &[(&str, &str)]) -> Setup {
    let scratch = tempfile::tempdir().unwrap();
    let repo = init_git_repo();
    let script = fake_harness_script(scratch.path(), "fake.sh", script_body);
    let mut env = vec![("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    env.extend_from_slice(extra);
    let daemon = Daemon::spawn(&env);
    let mut settings = daemon.request("settings.get", json!({}));
    assert_eq!(settings["implementBudgetMins"], 45.0, "{settings}");
    settings["review"] = json!("");
    settings["sandbox"] = json!("host");
    settings["maxAttempts"] = json!(1);
    if let Some(mins) = budget_mins {
        settings["implementBudgetMins"] = json!(mins);
    }
    daemon.request("settings.set", json!({"settings": settings}));
    Setup {
        daemon,
        repo,
        _scratch: scratch,
    }
}

fn start(s: &Setup, verify: &str) -> String {
    let task = s.daemon.request(
        "task.create",
        json!({
            "repo": s.repo.path().to_str().unwrap(),
            "title": "Budget",
            "goal": "Write the marker",
            "verify": [verify],
            "land": false,
            "start": true,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn cleanup(s: Setup, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap().to_string();
    s.daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(Path::new(&worktree));
}

fn first_attempt(task: &Value) -> &Value {
    &task["attempts"].as_array().unwrap()[0]
}

#[test]
fn the_brief_asks_for_targeted_tests_and_the_budget_defaults_to_45_minutes() {
    let s = setup(FAKE_PASS, None, &[]);
    let id = start(&s, "test -f CHANGED_MARKER.txt");
    let task = poll_until(&s.daemon, &id, |st| {
        matches!(st, "failed" | "done" | "waiting")
    });
    assert_eq!(task["status"], "done", "{task}");
    let brief = run_file(&s.daemon, &id, "brief.md");
    assert!(
        !brief.contains("Run the verification commands yourself"),
        "{brief}"
    );
    assert!(
        brief.contains("Run only the targeted tests for what you changed"),
        "{brief}"
    );
    cleanup(s, &task);
}

#[test]
fn an_attempt_past_its_budget_reports_in_its_own_session_and_fails_as_time_budget() {
    let s = setup(FAKE_SLOW, Some(0.05), &[]);
    let id = start(&s, "test -f NEVER.txt");
    let task = poll_until(&s.daemon, &id, |st| {
        matches!(st, "failed" | "done" | "waiting")
    });
    let attempt = first_attempt(&task);
    assert_eq!(attempt["failure"]["kind"], "time_budget", "{task}");
    assert_eq!(
        attempt["handoff"], "the parser is rewritten, the writer is not",
        "{task}"
    );
    assert_eq!(attempt["summary"], "half done", "{task}");
    assert!(
        attempt["changedFiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "BUDGET_MARKER.txt"),
        "{task}"
    );
    cleanup(s, &task);
}

#[test]
fn a_report_turn_that_fails_leaves_a_handoff_naming_the_budget_and_the_files() {
    let s = setup(FAKE_SLOW, Some(0.05), &[("RESUME_EXIT", "1")]);
    let id = start(&s, "test -f NEVER.txt");
    let task = poll_until(&s.daemon, &id, |st| {
        matches!(st, "failed" | "done" | "waiting")
    });
    let attempt = first_attempt(&task);
    assert_eq!(attempt["failure"]["kind"], "time_budget", "{task}");
    let handoff = attempt["handoff"].as_str().unwrap_or_default();
    assert!(handoff.contains("0.05 minute time budget"), "{task}");
    assert!(handoff.contains("BUDGET_MARKER.txt"), "{task}");
    cleanup(s, &task);
}
