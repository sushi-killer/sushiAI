//! Black-box integration tests (triage): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::time::Duration;

/// Any `Orchestrator:` line except the standing no-planner-tier note.
fn is_triage_decision(line: &str) -> bool {
    line.starts_with("Orchestrator:") && !line.starts_with("Orchestrator: no planner tier")
}

/// A task with no planner tier has no tier to route by: it runs on the
/// standard route and says why.
#[test]
fn a_task_without_a_planner_tier_runs_on_the_standard_route() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"changed\" > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "No tier case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["tier"], "standard", "{settled}");
    assert_eq!(
        settled["attempts"][0]["routeId"], "claude-sonnet",
        "{settled}"
    );
    assert_eq!(settled["tierFallback"], "no planner tier", "{settled}");
    let decisions = settled["decisions"].as_array().unwrap();
    assert!(
        decisions
            .iter()
            .any(|d| d == "Orchestrator: no planner tier -> standard, route claude-sonnet"),
        "expected the no-planner-tier decision: {decisions:?}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Reports `blocked` the first time (a blocked report goes straight into
/// triage), then `complete` on the retry -- so a triage "answer" lets the
/// task finish without ever reaching the owner. A brief containing
/// "sushi-triage" (only the triage report format mentions it) picks the
/// triage branch regardless of which route/attempt is asking.
const TRIAGE_ANSWERS_THEN_SUCCEEDS_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-triage*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-triage"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-triage\n{\"action\":\"answer\",\"answer\":\"use approach A\",\"reason\":\"README already says so\"}\n```"}'
    printf '%s\n' "$json"
    ;;
  *)
    if [ -f ALREADY_BLOCKED_ONCE.txt ]; then
      echo "changed" > CHANGED_MARKER.txt
      printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
      json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
      printf '%s\n' "$json"
    else
      touch ALREADY_BLOCKED_ONCE.txt
      echo "changed" > CHANGED_MARKER.txt
      printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
      json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"blocked\",\"summary\":\"need input\",\"decisions\":[],\"question\":\"Which approach, A or B?\"}\n```"}'
      printf '%s\n' "$json"
    fi
    ;;
esac
"#;

#[test]
fn triage_answer_lets_the_task_continue_without_the_owner() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        TRIAGE_ANSWERS_THEN_SUCCEEDS_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["orchestrator"] = serde_json::json!("claude-sonnet");
    settings["autoAnswer"] = serde_json::json!(true);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Pick an approach",
            "goal": "Add the feature",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(
        settled["status"], "done",
        "triage should have answered the blocked question itself: {settled}"
    );
    let decisions = settled["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(|d| {
            let d = d.as_str().unwrap_or("");
            d.starts_with("Orchestrator: Which approach, A or B?")
                && d.contains("use approach A")
                && d.contains("README already says so")
        }),
        "expected an Orchestrator answer decision: {decisions:?}"
    );
    // The blocked attempt keeps its own state: triage neither overwrites its
    // session nor drops its cost (two implement runs plus one triage run).
    let first = &settled["attempts"][0];
    assert_eq!(first["failure"]["kind"], "blocked", "attempt: {first}");
    assert_eq!(first["sessionId"], "sess-fake", "attempt: {first}");
    let cost = settled["costUsd"].as_f64().unwrap();
    assert!((cost - 0.03).abs() < 1e-9, "cost {cost}: {settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Fails verify until its brief carries the orchestrator's instruction;
/// as the orchestrator it answers the exhausted question with that fix.
const EXHAUSTED_TRIAGE_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-triage*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-triage"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-triage\n{\"action\":\"answer\",\"answer\":\"continue - create fixed.txt\",\"reason\":\"the verify only checks that file\"}\n```"}'
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > CHANGED_MARKER.txt
    case "$input" in *"continue - create fixed.txt"*) echo ok > fixed.txt ;; esac
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

#[test]
fn the_orchestrator_answers_attempts_keep_failing_and_the_task_passes() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        EXHAUSTED_TRIAGE_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["orchestrator"] = serde_json::json!("claude-sonnet");
    settings["autoAnswer"] = serde_json::json!(true);
    settings["maxAttempts"] = serde_json::json!(1);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Exhausted case",
            "goal": "Make verify pass",
            "criteria": [],
            "verify": ["test -f fixed.txt"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(20), |s| {
        s == "done" || s == "waiting" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let decisions = settled["decisions"].to_string();
    assert!(
        decisions.contains("Orchestrator: Attempts keep failing")
            && decisions.contains("continue - create fixed.txt"),
        "{decisions}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Always reports `blocked`; triage always escalates with a sharpened
/// question and its own options.
const TRIAGE_ESCALATES_WITH_SHARPENED_QUESTION_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-triage*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-triage"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-triage\n{\"action\":\"escalate\",\"question\":\"Pick the auth approach: OAuth or API key?\",\"options\":[\"OAuth\",\"API key\"],\"reason\":\"repo supports both, no default\"}\n```"}'
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"blocked\",\"summary\":\"need input\",\"decisions\":[],\"question\":\"Which auth approach?\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

#[test]
fn triage_escalate_waits_for_the_owner_with_the_sharpened_question() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        TRIAGE_ESCALATES_WITH_SHARPENED_QUESTION_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["orchestrator"] = serde_json::json!("claude-sonnet");
    settings["autoAnswer"] = serde_json::json!(true);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Add auth",
            "goal": "Add authentication",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert_eq!(
        waiting["question"]["text"], "Pick the auth approach: OAuth or API key?",
        "the owner should see triage's sharpened question, not the original: {waiting}"
    );
    assert_eq!(
        waiting["question"]["options"],
        serde_json::json!(["OAuth", "API key"])
    );
    let decisions = waiting["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(|d| d
            .as_str()
            .unwrap_or("")
            .starts_with("Orchestrator: escalated (")),
        "expected an Orchestrator escalation decision: {decisions:?}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Reports `complete` on a change under a protected path -- the resulting
/// approval question must reach the owner completely untouched, even with
/// the orchestrator on and (per its triage branch) willing to answer.
const TRIAGE_POISON_IF_EVER_ASKED_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-triage*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-triage"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-triage\n{\"action\":\"answer\",\"answer\":\"approve\",\"reason\":\"should never be asked\"}\n```"}'
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > PROTECTED.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

#[test]
fn protected_path_approval_is_never_triaged() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        TRIAGE_POISON_IF_EVER_ASKED_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["orchestrator"] = serde_json::json!("claude-sonnet");
    settings["autoAnswer"] = serde_json::json!(true);
    settings["protectedPaths"] = serde_json::json!(["PROTECTED.txt"]);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Touch a protected file",
            "goal": "Change something protected",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert!(
        waiting["question"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Change touches protected path"),
        "the protected-path question must reach the owner unmodified: {waiting}"
    );
    let decisions = waiting["decisions"].as_array().unwrap();
    assert!(
        !decisions
            .iter()
            .any(|d| is_triage_decision(d.as_str().unwrap_or(""))),
        "the orchestrator must never be asked to approve a protected path: {decisions:?}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn orchestrator_off_goes_straight_to_the_owner() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        TRIAGE_ESCALATES_WITH_SHARPENED_QUESTION_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["autoAnswer"] = serde_json::json!(false);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Add auth",
            "goal": "Add authentication",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    // The script would escalate with a *different*, sharpened question if
    // triage ever ran -- the owner seeing the original proves it didn't.
    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert_eq!(
        waiting["question"]["text"], "Which auth approach?",
        "orchestrator = \"\" must skip triage entirely: {waiting}"
    );
    let decisions = waiting["decisions"].as_array().unwrap();
    assert!(
        !decisions
            .iter()
            .any(|d| is_triage_decision(d.as_str().unwrap_or(""))),
        "no triage should have run at all: {decisions:?}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}
