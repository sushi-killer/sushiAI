//! Black-box integration tests (plan): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::harness::{fake_harness_script, FAKE_PLANNER_SCRIPT};
use common::native_sandbox::fit_sandbox;
use common::polling::{poll_until, remove_dir_retrying, settle, wait_until};
use common::process::{socket_of, spawn_daemon, wait_for_exit, Daemon};
use common::protocol::{request_on, stop_daemon, wait_for_socket};
use common::repository::init_git_repo;

/// The plan branch never produces a valid ```sushi-plan fence, no matter how
/// many times it's asked -- for exercising the "could not draft this task"
/// clarification path itself (as opposed to [`CLARIFY_THEN_SUCCEED_SCRIPT`],
/// which uses the clarification to recover).
const GARBAGE_PLANNER_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
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
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
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

/// The planner flags a criterion visual although its check is a command.
const VISUAL_COMMAND_CRITERION_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
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

/// The planner names a screenshot command it found in the repo.
const SCREENSHOT_PLAN_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
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
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
  *sushi-plan*)
    case "$input" in
      *CHILD_REQ_B*) out '"```sushi-plan\n{\"title\":\"Part B\",\"goal\":\"MAKE_B: create b.txt\",\"criteria\":[\"b.txt exists\"],\"verify\":[\"test -f b.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"Both parts\",\"goal\":\"Create a.txt, then b.txt\",\"verify\":[\"true\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"goal\":\"MAKE_A: create a.txt\",\"criteria\":[\"a.txt exists\",{\"text\":\"a screenshot of a.txt\",\"visual\":true}],\"verify\":[\"test -f a.txt\"],\"finalVerify\":[\"true\"],\"paths\":[\"a.txt\"],\"check\":\"test -f a.txt\",\"tier\":\"standard\",\"request\":\"CHILD_REQ_A create a.txt\"},__PART_B__]}\n```"' ;;
    esac
    ;;
  *MAKE_B*) echo b > b.txt; out "$report" ;;
  *MAKE_A*) echo a > a.txt; out "$report" ;;
  *) out "$report" ;;
esac
"#;

const COMPLETE_PART_B: &str = r#"{\"key\":\"b\",\"title\":\"Part B\",\"goal\":\"MAKE_B: create b.txt\",\"criteria\":[\"b.txt exists\"],\"verify\":[\"test -f b.txt\"],\"paths\":[\"b.txt\"],\"check\":\"test -f b.txt\",\"dependsOn\":[\"a\"],\"request\":\"CHILD_REQ_B create b.txt\"}"#;
const INCOMPLETE_PART_B: &str = r#"{\"key\":\"b\",\"paths\":[\"b.txt\"],\"check\":\"test -f b.txt\",\"dependsOn\":[\"a\"],\"request\":\"CHILD_REQ_B create b.txt\"}"#;

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
  *"You review a coding task plan"*) printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}' ;;
  *sushi-plan*) out '"```sushi-plan\n{\"title\":\"One part\",\"goal\":\"Create a.txt\",\"verify\":[\"true\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"goal\":\"MAKE_A: create a.txt\",\"criteria\":[\"a.txt exists\"],\"verify\":[\"true\"],\"checks\":[{\"criterion\":0,\"run\":\"test -f a.txt\"},{\"criterion\":9,\"run\":\"test -f nowhere.txt\"}],\"heldOut\":{\"criterion\":0,\"run\":\"test -s a.txt\"},\"screenshot\":\"  mkdir -p artifacts && echo png > artifacts/a.png  \",\"paths\":[\"a.txt\"],\"check\":\"test -f a.txt\",\"request\":\"CHILD_REQ_A\"},{\"key\":\"b\",\"title\":\"Part B\",\"goal\":\"MAKE_B: create b.txt\",\"criteria\":[\"b.txt exists\"],\"verify\":[\"test -f b.txt\"],\"paths\":[\"b.txt\"],\"check\":\"test -f b.txt\",\"dependsOn\":[\"a\"],\"request\":\"CHILD_REQ_B\"}]}\n```"' ;;
  *MAKE_A*) echo a > a.txt; out "$report" ;;
  *MAKE_B*) echo b > b.txt; out "$report" ;;
  *) out "$report" ;;
esac
"#;

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

#[path = "plan/contract.rs"]
mod contract_tests;
#[path = "plan/drafting.rs"]
mod drafting_tests;
#[path = "plan/routing.rs"]
mod routing_tests;
#[path = "plan/subtasks.rs"]
mod subtasks_tests;
