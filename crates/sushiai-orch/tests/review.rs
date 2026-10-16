//! Black-box integration tests (review): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::harness::fake_harness_script;
use common::polling::{poll_task_status, poll_until, remove_dir_retrying};
use common::process::Daemon;
use common::repository::init_git_repo;

use serde_json::json;

/// Review by the Claude hard route, the same fake as the implementer (`auto`
/// picks a route on the other harness).
fn review_by_claude(daemon: &Daemon) {
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
}

const FAKE_OVERLAP_REVIEW: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
'## Route the review'*) exit 0 ;;
"## Review"*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.04,"result":"Review in progress"}'
  echo "$ORCHD_TASK" > "$LOG_DIR/review-start"
  while [ ! -f "$LOG_DIR/final-start" ]; do sleep 0.02; done
  if [ "$REVIEW_MODE" = final-fails ]; then exec sleep 30; fi
  touch "$LOG_DIR/review-end"
  if [ "$REVIEW_MODE" = review-fails ]; then
    printf '%s\n' '{"type":"result","total_cost_usd":0.04,"result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"[P1] CHANGED_MARKER.txt:1 is wrong\"]}\n```"}'
  else
    printf '%s\n' '{"type":"result","total_cost_usd":0.04,"result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}'
  fi ;;
*)
  echo changed > CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

fn overlapping_review(mode: &str) -> (serde_json::Value, tempfile::TempDir) {
    let log = tempfile::tempdir().unwrap();
    let script = fake_harness_script(log.path(), "overlap.sh", FAKE_OVERLAP_REVIEW);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("LOG_DIR", log.path().to_str().unwrap()),
        ("REVIEW_MODE", mode),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    settings["maxAttempts"] = json!(1);
    // The final check reads the daemon's data folder, which the sandbox hides.
    settings["sandbox"] = json!("host");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    // Base preflight passes without markers; only the task tree runs this job.
    let command = r#"if [ -f CHANGED_MARKER.txt ]; then
echo ran >> "$LOG_DIR/final-runs"
touch "$LOG_DIR/final-start"
case "$REVIEW_MODE" in
final-fails)
  while ! grep -q total_cost_usd "$SUSHIAI_HOME/orchestrator/tasks/$(cat "$LOG_DIR/review-start" 2>/dev/null)/helpers/review-1/events.jsonl" 2>/dev/null; do sleep 0.02; done
  echo final broke; exit 9 ;;
review-fails) sleep 30 ;;
*) sleep 0.2 ;;
esac
touch "$LOG_DIR/final-done"
fi"#;
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(), "title": "Overlap",
            "goal": "Write the marker", "verify": ["true"],
            "finalVerify": [command], "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap();
    let settled = poll_task_status(&daemon, id);
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
    (settled, log)
}

/// Like FAKE_RETRY_SCRIPT, but it also answers the advisor: a brief that
/// says an attempt failed gets $ADVISOR_MODE ("ok" answers, "fail" exits 1
/// with no output). Every run's brief is kept as $LOG_DIR/brief.<pid>.
const FAKE_ADVISOR_SCRIPT: &str = r#"#!/bin/sh
cat > "$LOG_DIR/brief.$$"
if grep -q 'An implement attempt at this task failed' "$LOG_DIR/brief.$$"; then
  echo advisor >> "$LOG_DIR/advisor.log"
  if [ "$ADVISOR_MODE" = fail ]; then exit 1; fi
  printf '%s\n' '{"type":"result","total_cost_usd":0.03,"usage":{"input_tokens":1,"output_tokens":1},"result":"Create SECOND, not FIRST."}'
  exit 0
fi
if [ -f FIRST ]; then echo x > SECOND; else echo x > FIRST; fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"handoff\":\"quick fix\",\"decisions\":[],\"question\":\"\"}\n```"}'
case " $* " in *" --resume "*) json=$(printf '%s' "$json" | sed 's/total_cost_usd":0.01/total_cost_usd":0.02/') ;; esac
printf '%s\n' "$json"
"#;

/// Implements like the other fakes; as reviewer, answers PASS but marks a
/// criterion unmet when the brief asks for a ruling per criterion.
const FAKE_CONTRACT_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
'## Route the review'*) exit 0 ;;
"## Review"*)
  case "$brief" in
  *"Rule on every acceptance criterion"*)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[],\"criteria\":[{\"criterion\":0,\"met\":false,\"evidence\":\"wrong file\"}]}\n```"}' ;;
  *)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
  esac
  printf '%s\n' "$json" ;;
*)
  echo changed > CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
  printf '%s\n' "$json" ;;
esac
"###;

/// Implements; saves a screenshot only once its brief says none was saved
/// (the evidence failure's feedback); as the reviewer answers PASS.
const FAKE_SCREENSHOT_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
'## Route the review'*) exit 0 ;;
"## Review"*)
  printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*)
  echo changed > CHANGED_MARKER.txt
  case "$brief" in
  *"no image was saved"*) mkdir -p artifacts/ui; echo png > artifacts/ui/panel.png ;;
  esac
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

/// Attempt 1 writes source and an image and its review FAILs with a plain
/// finding; attempt 2 writes `ATTEMPT_TWO_FILE` and no image, its review
/// PASSes. Counters in the scripts dir tell the runs apart.
const FAKE_RETRY_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
d=$(dirname "$0")
case "$brief" in
'## Route the review'*) exit 0 ;;
"## Review"*)
  echo x >> "$d/review.count"
  if [ "$(wc -l < "$d/review.count" | tr -d ' ')" = 1 ]; then
    printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"P1: the label is wrong\"]}\n```"}'
  else
    printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}'
  fi ;;
*)
  echo x >> "$d/impl.count"
  if [ "$(wc -l < "$d/impl.count" | tr -d ' ')" = 1 ]; then
    mkdir -p src/components artifacts/ui
    echo one > src/components/panel.tsx
    echo png > artifacts/ui/panel.png
  else
    mkdir -p ATTEMPT_TWO_DIR
    echo two > ATTEMPT_TWO_FILE
  fi
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

fn run_retry_task(second_file: &str) -> (Daemon, serde_json::Value, String) {
    let scripts_dir = tempfile::tempdir().unwrap();
    let dir = std::path::Path::new(second_file)
        .parent()
        .unwrap()
        .to_str()
        .unwrap();
    let body = FAKE_RETRY_SCRIPT
        .replace("ATTEMPT_TWO_DIR", dir)
        .replace("ATTEMPT_TWO_FILE", second_file);
    let script = fake_harness_script(scripts_dir.path(), "fake-retry.sh", &body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_by_claude(&daemon);
    let repo = init_git_repo();
    std::fs::write(repo.path().join(".gitignore"), "artifacts/\n").unwrap();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Visual retry",
            "goal": "Change the panel",
            "screenshot": "true",
            "criteria": ["The panel shows thumbnails -- check: screenshot under artifacts/"],
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    // The scripts dir must outlive the run.
    drop(scripts_dir);
    (daemon, settled, task_id)
}

/// Implements (appending, so every attempt's diff differs), answers the
/// advisor, and reviews: FAIL with a plain finding at first, and once the
/// review brief carries the previous review, FAIL with the same finding
/// marked `repeat`. Every review brief is kept as $LOG_DIR/review.<pid>.
const FAKE_REPEAT_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
*"An implement attempt at this task failed"*)
  echo advisor >> "$LOG_DIR/advisor.log"
  printf '%s\n' '{"type":"result","total_cost_usd":0.03,"usage":{"input_tokens":1,"output_tokens":1},"result":"Fix the null check."}' ;;
'## Route the review'*) exit 0 ;;
"## Review"*)
  printf '%s' "$brief" > "$LOG_DIR/review.$$"
  case "$brief" in
  *"## Previous review"*)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[{\"finding\":\"P1: a.rs:1 - null check missing\",\"id\":\"F-1\"}]}\n```"}' ;;
  *)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"P1: a.rs:1 - null check missing\"]}\n```"}' ;;
  esac
  printf '%s\n' "$json" ;;
*)
  echo changed >> CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

/// Implements (appending), and reviews: a first FAIL with a plain finding; a
/// review that has declined findings to rule on answers `UPHELD` for the
/// declined F-1. An implement brief that shows a review failure gets a report
/// declining F-1 with evidence.
const FAKE_DECLINE_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
*"An implement attempt at this task failed"*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.03,"usage":{"input_tokens":1,"output_tokens":1},"result":"Fix the null check."}' ;;
'## Route the review'*) exit 0 ;;
"## Review"*)
  case "$brief" in
  *"## Declined findings"*)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[],\"declineRulings\":[{\"id\":\"F-1\",\"upheld\":UPHELD}]}\n```"}' ;;
  *)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"P1: a.rs:1 - null check missing\"]}\n```"}' ;;
  esac
  printf '%s\n' "$json" ;;
*)
  echo changed >> CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  case "$brief" in
  *"The last attempt failed review"*)
    report='```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\",\"declines\":[{\"id\":\"F-1\",\"reason\":\"wrong\",\"evidence\":[\"a.rs:1\"]}]}\n```' ;;
  *)
    report='```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```' ;;
  esac
  printf '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"%s"}\n' "$report" ;;
esac
"###;

/// The decline script with the review's ruling on F-1 filled in.
fn decline_script(upheld: bool) -> String {
    FAKE_DECLINE_SCRIPT.replace("UPHELD", if upheld { "true" } else { "false" })
}

/// Runs the decline flow with the review ruling `upheld` on the declined
/// finding; returns the settled task.
fn run_decline_task(upheld: bool) -> serde_json::Value {
    let scripts_dir = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(
        scripts_dir.path(),
        "fake-decline.sh",
        &decline_script(upheld),
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", claude.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(6);
    settings["answerPolicy"] = serde_json::json!(false);
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Decline",
            "goal": "Write the marker",
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
    settled
}

fn decision_lines(task: &serde_json::Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str().map(String::from))
        .collect()
}

/// The route turn answers with the areas a change can break; the full review
/// is briefed with those areas and no others.
const FAKE_ROUTE_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Route the review"*)
  printf '%s\n' '{"type":"result","result":"```sushi-route\n{\"lenses\":{\"seams\":false,\"code\":true,\"tests\":false,\"errors\":false,\"docs\":false},\"reason\":\"one function\"}\n```"}' ;;
"## Review"*)
  printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*)
  echo changed >> CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

#[path = "review/evidence.rs"]
mod evidence_tests;
#[path = "review/judgment.rs"]
mod judgment_tests;
#[path = "review/overlap.rs"]
mod overlap_tests;
#[path = "review/routing.rs"]
mod routing_tests;
