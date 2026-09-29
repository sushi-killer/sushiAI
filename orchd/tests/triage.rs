//! Black-box integration tests (triage): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

/// A one-shot-per-connection fake HTTP server that always answers with
/// `answers_json` wrapped as an OpenAI chat-completion envelope -- a local,
/// deterministic stand-in for `classify::decide`'s `Openai` backend (the
/// only backend whose base URL is a runtime setting rather than hardcoded),
/// so a test can drive a real classifier success without a network call.
/// Runs on a detached thread for the test process's lifetime -- ponytail:
/// nothing ever joins it, since the process exit is what reclaims it.
fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn spawn_fake_openai_classifier(answers_json: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let body = serde_json::json!({
        "choices": [{"message": {"content": answers_json}}]
    })
    .to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            // Drain the full request (headers + declared Content-Length)
            // before writing anything back: closing on a socket that still
            // has unread bytes queued can RST the connection and truncate
            // our own response, which showed up as an intermittent ureq
            // "invalid header" parse failure on the client side.
            let mut received = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        received.extend_from_slice(&chunk[..n]);
                        let Some(header_end) = find_double_crlf(&received) else {
                            continue;
                        };
                        let headers = String::from_utf8_lossy(&received[..header_end]);
                        let content_length: usize = headers
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().to_string())
                            })
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        if received.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    format!("http://{addr}")
}

#[test]
fn jev_tier_decision_lands_in_task_decisions_on_classifier_success() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"changed\" > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    // "standard" routes to the Claude harness (default `tiers` map), which
    // is the one faked above via `ORCHD_CLAUDE_BIN` -- "mechanical" would
    // route to Codex and hang waiting on a real `codex` binary.
    let base_url = spawn_fake_openai_classifier(
        r#"{"answers":{"tier":{"choice":"standard","probabilities":{"mechanical":0.08,"standard":0.82,"hard":0.1}}}}"#,
    );
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["classifier"] =
        serde_json::json!({"backend": "openai", "model": "fake", "providerId": "fake"});
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    daemon.request(
        "secrets.set",
        serde_json::json!({"classifier": {"key": "test-key", "baseUrl": base_url}}),
    );

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Pass case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let decisions = settled["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(|d| d
            .as_str()
            .unwrap_or("")
            .starts_with("Jev: tier standard (p 0.82) -> route ")),
        "expected a Jev tier decision: {decisions:?}"
    );
    assert!(
        !decisions
            .iter()
            .any(|d| d.as_str().unwrap_or("").contains("tier unavailable")),
        "a confident classifier answer must not fall back: {decisions:?}"
    );
    assert!(settled.get("tierFallback").is_none(), "{settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// A low-confidence classifier answer (p < 0.5) must not be routed on the
/// classified tier -- it falls back to standard and records why, without
/// ever writing the misleading `Jev: tier hard (p 0.45)` line (the
/// pre-fallback behaviour this test guards against, docs/orchd-acceptance-audit.md G8).
#[test]
fn jev_tier_falls_back_to_standard_on_a_low_confidence_answer() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"changed\" > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    // A p 0.45 "hard" answer falls back to standard, which routes to the
    // Claude harness faked above (the default `tiers` map) -- "hard" would
    // route to Claude Opus and still work, but standard is what the
    // fallback path must land on regardless of the classifier's choice.
    let base_url = spawn_fake_openai_classifier(
        r#"{"answers":{"tier":{"choice":"hard","probabilities":{"mechanical":0.05,"standard":0.5,"hard":0.45}}}}"#,
    );
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["classifier"] =
        serde_json::json!({"backend": "openai", "model": "fake", "providerId": "fake"});
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    daemon.request(
        "secrets.set",
        serde_json::json!({"classifier": {"key": "test-key", "baseUrl": base_url}}),
    );

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Unsure case",
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
    let decisions = settled["decisions"].as_array().unwrap();
    assert!(
        decisions
            .iter()
            .any(|d| d
                == "Jev: tier unavailable (unsure: hard p 0.45) -> fallback standard, route claude-sonnet"),
        "expected a tier-unavailable fallback decision: {decisions:?}"
    );
    assert!(
        !decisions
            .iter()
            .any(|d| d.as_str().unwrap_or("").starts_with("Jev: tier hard")),
        "must not write the misleading pre-fallback line: {decisions:?}"
    );
    assert_eq!(settled["tierFallback"], "unsure: hard p 0.45", "{settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Reports `blocked` the first time (with classifier off by default, that
/// takes the "Jev says not answerable / Jev is off" branch straight into
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

    // `classifier.backend: "openrouter"` with no key configured routes the
    // first blocked report into "Jev is off" rather than a classifier call.
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
            .any(|d| d.as_str().unwrap_or("").starts_with("Orchestrator:")),
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
            .any(|d| d.as_str().unwrap_or("").starts_with("Orchestrator:")),
        "no triage should have run at all: {decisions:?}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn classify_probe_asks_the_configured_classifier_arbitrary_questions() {
    let daemon = Daemon::spawn(&[]);
    let base_url = spawn_fake_openai_classifier(
        r#"{"answers":{"consistent":{"noul":0.2},"pick":{"choice":"b"}}}"#,
    );
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["classifier"] =
        serde_json::json!({"backend": "openai", "model": "fake", "providerId": "fake"});
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    daemon.request(
        "secrets.set",
        serde_json::json!({"classifier": {"key": "test-key", "baseUrl": base_url}}),
    );
    let result = daemon.request(
        "classify.probe",
        serde_json::json!({
            "state": {"criteria": ["a", "not a"]},
            "questions": [
                {"name": "consistent", "prompt": "Consistent?"},
                {"name": "pick", "prompt": "Which?", "options": ["a", "b"]}
            ]
        }),
    );
    assert_eq!(result["answers"]["consistent"]["noul"], 0.2, "{result}");
    assert_eq!(result["answers"]["pick"]["choice"], "b", "{result}");
}
