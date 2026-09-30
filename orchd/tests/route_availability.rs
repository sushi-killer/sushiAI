//! Black-box integration tests (harness availability): each daemon runs its
//! own orchd, so a missing CLI on one machine changes only that machine's
//! routing. Fake harness scripts stand in for the CLIs; one is pointed at a
//! path that does not exist.

mod common;

use common::*;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

const MISSING: &str = "/nonexistent/orchd-test/harness";

/// Answers the brief check, a planner brief and an implementation, and logs
/// which one it was to `$CALLS_LOG`.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *"You check a coding task brief"*)
    echo claude-check >> "$CALLS_LOG"
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-check"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"{\"contradiction\": false, \"conflict\": \"\"}"}'
    ;;
  *)
    echo claude-implement >> "$CALLS_LOG"
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    ;;
esac
"#;

const FAKE_CODEX: &str = r#"#!/bin/sh
case "$1" in --version) echo "codex-fake 1.0"; exit 0;; esac
input="$(cat)"
case "$input" in
  *sushi-plan*)
    echo codex-plan >> "$CALLS_LOG"
    printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"```sushi-plan\n{\"title\":\"Add it\",\"goal\":\"Add the thing\",\"tier\":\"standard\",\"criteria\":[\"It exists\"],\"verify\":[\"true\"],\"questions\":[]}\n```"}}'
    ;;
  *"You check a coding task brief"*)
    echo codex-check >> "$CALLS_LOG"
    printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"contradiction\": false, \"conflict\": \"\"}"}}'
    ;;
  *)
    echo codex-implement >> "$CALLS_LOG"
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}}'
    ;;
esac
"#;

struct Setup {
    daemon: Daemon,
    log: std::path::PathBuf,
    _scripts: tempfile::TempDir,
    _repo: tempfile::TempDir,
}

/// `claude`/`codex` are the fake script, or `None` for a path that is not
/// there. `tweak` edits the settings before any task exists.
fn setup(
    claude: Option<&str>,
    codex: Option<&str>,
    tweak: impl Fn(&mut serde_json::Value),
) -> (Setup, String) {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("calls.log");
    std::fs::write(&log, "").unwrap();
    let bin = |name: &str, body: Option<&str>| match body {
        Some(body) => fake_harness_script(scripts.path(), name, body)
            .to_str()
            .unwrap()
            .to_string(),
        None => MISSING.to_string(),
    };
    let (claude_bin, codex_bin) = (bin("claude.sh", claude), bin("codex.sh", codex));
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", &claude_bin),
        ("ORCHD_CODEX_BIN", &codex_bin),
        ("CALLS_LOG", log.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    tweak(&mut settings);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap().to_string();
    (
        Setup {
            daemon,
            log,
            _scripts: scripts,
            _repo: repo,
        },
        repo_path,
    )
}

fn create(s: &Setup, repo: &str, extra: serde_json::Value) -> String {
    let mut params = json!({
        "repo": repo, "title": "Add it", "goal": "Add the thing",
        "criteria": [], "verify": ["true"], "start": true,
    });
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    s.daemon.request("task.create", params)["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn calls(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn decisions(task: &serde_json::Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

fn settle(s: &Setup, id: &str) -> serde_json::Value {
    poll_until(&s.daemon, id, Duration::from_secs(30), |st| {
        st == "done" || st == "failed" || st == "stopped" || st == "waiting"
    })
}

#[test]
fn a_task_routed_to_a_missing_codex_runs_on_claude_and_says_so() {
    let (s, repo) = setup(Some(FAKE_CLAUDE), None, |settings| {
        // Standard work goes to codex; the brief check is on.
        settings["tiers"]["standard"] = json!("codex");
        settings["briefCheckRoute"] = json!("auto");
    });
    let id = create(&s, &repo, json!({}));
    let task = settle(&s, &id);
    assert_eq!(task["status"], "done", "task JSON: {task}");
    // The check and the attempt both ran on claude; the ENOENT path was
    // never reached because the route was swapped up front.
    assert_eq!(calls(&s.log), ["claude-check", "claude-implement"]);
    assert_eq!(task["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(task["attempts"][0]["routeId"], "claude-sonnet");
    let line =
        format!("Orchestrator: codex unavailable ({MISSING} not found), using claude-sonnet");
    assert!(decisions(&task).contains(&line), "{:?}", decisions(&task));
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_deleted_codex_route_is_not_resurrected_by_the_brief_check() {
    // codex is installed, but the owner deleted its route and the check
    // setting still names it.
    let (s, repo) = setup(Some(FAKE_CLAUDE), Some(FAKE_CODEX), |settings| {
        let routes = settings["routes"].as_array_mut().unwrap();
        routes.retain(|r| r["id"] != "codex");
        settings["tiers"]["mechanical"] = json!("claude-sonnet");
        settings["briefCheckRoute"] = json!("codex");
    });
    let id = create(&s, &repo, json!({}));
    let task = settle(&s, &id);
    assert_eq!(task["status"], "done", "task JSON: {task}");
    assert_eq!(calls(&s.log), ["claude-check", "claude-implement"]);
    let line = "Brief check: route codex is not configured or unavailable; using the cheapest available route claude-sonnet";
    assert!(
        decisions(&task).iter().any(|l| l == line),
        "{:?}",
        decisions(&task)
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_missing_claude_sends_the_planner_and_the_attempt_to_codex() {
    let (s, repo) = setup(None, Some(FAKE_CODEX), |_| {});
    let id = s.daemon.request(
        "task.create",
        json!({"repo": repo, "request": "add the thing", "start": true}),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let task = settle(&s, &id);
    assert_eq!(task["status"], "done", "task JSON: {task}");
    assert_eq!(calls(&s.log), ["codex-plan", "codex-implement"]);
    assert_eq!(task["attempts"].as_array().unwrap().len(), 2);
    let lines = decisions(&task);
    let swap = format!("Orchestrator: claude-opus unavailable ({MISSING} not found), using codex");
    assert!(lines.contains(&swap), "{lines:?}");
    s.daemon.shutdown_and_wait();
}

#[test]
fn with_no_harness_the_task_waits_with_an_install_question_and_spends_no_attempt() {
    let (s, repo) = setup(None, None, |_| {});
    let id = create(&s, &repo, json!({}));
    let task = settle(&s, &id);
    assert_eq!(task["status"], "waiting", "task JSON: {task}");
    assert_eq!(task["question"]["kind"], "harness_missing");
    let text = task["question"]["text"].as_str().unwrap();
    assert!(text.contains("install") && text.contains(MISSING), "{text}");
    assert!(task["attempts"].as_array().unwrap().is_empty());
    // Answering while nothing is installed asks again; still no attempt.
    s.daemon
        .request("task.answer", json!({"id": id, "answer": "retry"}));
    std::thread::sleep(Duration::from_millis(500));
    let again = s.daemon.request("task.get", json!({"id": id}));
    assert_eq!(again["status"], "waiting", "{again}");
    assert_eq!(again["question"]["kind"], "harness_missing");
    assert!(again["attempts"].as_array().unwrap().is_empty());
    assert!(calls(&s.log).is_empty());
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_spawn_that_finds_no_binary_costs_no_attempt() {
    // Executable, so the probe finds it, but its interpreter is absent: the
    // spawn itself fails with ENOENT.
    let (s, repo) = setup(
        Some("#!/nonexistent/orchd-test/interpreter\n"),
        Some(FAKE_CODEX),
        |settings| settings["briefCheckRoute"] = json!(""),
    );
    let id = create(&s, &repo, json!({}));
    let task = settle(&s, &id);
    assert_eq!(task["status"], "done", "task JSON: {task}");
    let attempts = task["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1, "the failed spawn was not an attempt");
    assert_eq!(attempts[0]["routeId"], "codex");
    assert_eq!(attempts[0]["n"], 1);
    assert_eq!(calls(&s.log), ["codex-implement"]);
    let lines = decisions(&task);
    assert!(
        lines.iter().any(|l| l.contains("no attempt was used")),
        "{lines:?}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn verify_timeout_secs_bounds_a_verify_command_and_names_the_limit() {
    let (s, repo) = setup(Some(FAKE_CLAUDE), None, |settings| {
        settings["verifyTimeoutSecs"] = json!(1);
        settings["briefCheckRoute"] = json!("");
        settings["maxAttempts"] = json!(1);
    });
    let id = create(&s, &repo, json!({"verify": ["sleep 30"]}));
    let task = settle(&s, &id);
    let verify = &task["attempts"][0]["verify"][0];
    assert_eq!(verify["tail"], "timed out after 1 seconds", "{task}");
    assert!(verify["code"].is_null());
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_zero_verify_timeout_is_refused() {
    let (s, _repo) = setup(Some(FAKE_CLAUDE), None, |_| {});
    let mut settings = s.daemon.request("settings.get", json!({}));
    settings["verifyTimeoutSecs"] = json!(0);
    let err = s
        .daemon
        .request_error("settings.set", json!({"settings": settings}));
    assert!(err.contains("verifyTimeoutSecs"), "{err}");
    s.daemon.shutdown_and_wait();
}
