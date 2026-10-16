//! Black-box integration tests (waiting questions): spawn the real `orchd`
//! binary and drive it over its NDJSON unix socket. The overseer is the one
//! decider of a waiting question: no triage turn runs before it.

mod common;

use common::process::Daemon;

use common::harness::fake_harness_script;
use common::polling::{poll_task_status, poll_until, remove_dir_retrying};
use common::repository::init_git_repo;

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

    let settled = poll_task_status(&daemon, &task_id);
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
    remove_dir_retrying(worktree);
}

/// A fake Claude whose implement runs end with `report`. It records every run
/// that carries the triage report format (`triage-ran.txt` in its cwd, the
/// task's worktree) and every overseer wake (`overseer-ran.txt` in the
/// repository), and the overseer answers with a plain note.
fn overseer_script(report: &str) -> String {
    OVERSEER_SCRIPT.replace("__REPORT__", report)
}

const OVERSEER_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-triage*)
    echo triage >> triage-ran.txt
    ;;
  *"is waiting for an answer"*)
    echo overseer >> overseer-ran.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-overseer"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"Overseer note."}'
    ;;
  *)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"__REPORT__"}'
    ;;
esac
"#;

const BLOCKED_REPORT: &str = r#"```sushi-report\n{\"outcome\":\"blocked\",\"summary\":\"need input\",\"decisions\":[],\"question\":\"Which auth approach?\"}\n```"#;

const COMPLETE_REPORT: &str = r#"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"#;

const TAGGED_REPORT: &str = r#"```sushi-report\n{\"outcome\":\"blocked\",\"summary\":\"need input\",\"decisions\":[],\"question\":\"Which auth approach?\",\"risk\":[\"security\"]}\n```"#;

/// Waits until `path` exists, for a run that starts in the background.
fn wait_for_file(path: &std::path::Path) -> bool {
    let start = std::time::Instant::now();
    while !path.exists() {
        if start.elapsed() > std::time::Duration::from_secs(10) {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    true
}

#[test]
fn a_blocked_question_goes_to_the_overseer_with_no_triage_turn() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &overseer_script(BLOCKED_REPORT),
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

    let waiting = poll_until(&daemon, &task_id, |s| s == "waiting");
    assert_eq!(
        waiting["question"]["text"], "Which auth approach?",
        "the overseer sees the question as the agent asked it: {waiting}"
    );
    let worktree = std::path::PathBuf::from(task["worktree"].as_str().unwrap());
    assert!(
        wait_for_file(&repo.path().join("overseer-ran.txt")),
        "the overseer should be woken for the waiting task"
    );
    assert!(
        !worktree.join("triage-ran.txt").exists(),
        "no triage turn may run before the overseer decides"
    );

    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

/// The agent tags its blocked question with what it commits the owner to: the
/// question carries the tag, and the overseer is offered no answer for it.
#[test]
fn a_blocked_report_tags_its_question_with_risk() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &overseer_script(TAGGED_REPORT),
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

    let waiting = poll_until(&daemon, &task_id, |s| s == "waiting");
    assert_eq!(waiting["question"]["risk"], serde_json::json!(["security"]));

    let worktree = std::path::PathBuf::from(task["worktree"].as_str().unwrap());
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn with_auto_answer_off_the_owner_gets_the_original_question() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &overseer_script(BLOCKED_REPORT),
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["orchestrator"] = serde_json::json!("claude-sonnet");
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

    let waiting = poll_until(&daemon, &task_id, |s| s == "waiting");
    assert_eq!(
        waiting["question"]["text"], "Which auth approach?",
        "{waiting}"
    );
    let worktree = std::path::PathBuf::from(task["worktree"].as_str().unwrap());
    assert!(
        !worktree.join("triage-ran.txt").exists(),
        "no triage turn may run with auto-answer off"
    );
    assert!(
        !repo.path().join("overseer-ran.txt").exists(),
        "no overseer turn may run with auto-answer off"
    );

    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

/// Fails verify on every attempt, so the attempts run out and "attempts keep
/// failing" goes to the overseer, never a triage turn.
#[test]
fn attempts_keep_failing_waits_for_the_overseer_with_no_triage_turn() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        &overseer_script(COMPLETE_REPORT),
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

    // The overseer is woken once; its turn leaves the question open (the fake
    // answers nothing), so the failure ladder ends the task and the owner is
    // not asked.
    let ended = poll_until(&daemon, &task_id, |s| s == "failed");
    assert_eq!(ended["status"], "failed", "task JSON: {ended}");
    assert!(
        ended.get("question").is_none() || ended["question"].is_null(),
        "{ended}"
    );
    assert!(
        ended["decisions"]
            .to_string()
            .contains("Ladder: stopped after"),
        "{ended}"
    );
    let worktree = std::path::PathBuf::from(task["worktree"].as_str().unwrap());
    assert!(
        wait_for_file(&repo.path().join("overseer-ran.txt")),
        "the overseer should be woken for the exhausted task"
    );
    assert!(!worktree.join("triage-ran.txt").exists());

    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

/// Reports `complete` on a change under a protected path -- the resulting
/// approval question must reach the owner completely untouched: the
/// protected path is owner-only, so no agent turn may answer it.
#[test]
fn protected_path_approval_is_never_given_to_an_agent() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *"is waiting for an answer"*)
    echo overseer >> overseer-ran.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-overseer"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"Overseer note."}'
    ;;
  *)
    echo "changed" > PROTECTED.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    ;;
esac
"#,
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

    let waiting = poll_until(&daemon, &task_id, |s| s == "waiting");
    assert!(
        waiting["question"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Change touches protected path"),
        "the protected-path question must reach the owner unmodified: {waiting}"
    );
    // The standing "no planner tier" note is not an answer.
    let decisions = waiting["decisions"].as_array().unwrap();
    assert!(
        !decisions.iter().any(|d| {
            let d = d.as_str().unwrap_or("");
            d.starts_with("Orchestrator:") && !d.contains("no planner tier")
        }),
        "no agent may answer a protected path: {decisions:?}"
    );
    assert!(
        !repo.path().join("overseer-ran.txt").exists(),
        "an owner-only question never wakes the overseer"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
