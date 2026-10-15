//! Black-box integration tests (brief-check contradiction in a planner
//! draft): the planner's own session is resumed with a short correction,
//! and only without a session, a failed run or an unparseable reply does a
//! fresh draft follow.

mod common;

use common::harness::fake_harness_script;
use common::polling::poll_until;
use common::process::Daemon;
use common::repository::init_git_repo;

/// Logs each call as `<kind> <argv>` to `$CALLS_LOG`. The check judge says
/// conflict on its first call and clear on the next. The first fresh
/// planner run reports session `$PLAN_SESSION` (none when empty); a resumed
/// run (`--resume` in argv) answers `$RESUME_REPLY`, a corrected plan when
/// it is empty.
const FAKE_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
plan() {
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"'"$1"'\",\"goal\":\"Add a dark theme toggle\",\"tier\":\"hard\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"true\"],\"questions\":[]}\n```"}'
}
case "$input" in
  *"You check a coding task brief"*)
    echo "check $*" >> "$CALLS_LOG"
    n="$STATE/checks"
    if [ -f "$n" ]; then
      reply='{\"contradiction\": false, \"conflict\": \"\"}'
    else
      : > "$n"
      reply='{\"contradiction\": true, \"conflict\": \"keep the API and remove the API\"}'
    fi
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"'"$reply"'"}'
    ;;
  *'```sushi-plan'*)
    case "$*" in
      *--resume*)
        echo "plan-resume $*" >> "$CALLS_LOG"
        printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan-1"}'
        if [ -n "$RESUME_REPLY" ]; then
          printf '%s\n' '{"type":"result","total_cost_usd":0.5,"usage":{"input_tokens":1,"output_tokens":1},"result":"'"$RESUME_REPLY"'"}'
        else
          plan "Corrected draft"
        fi
        ;;
      *)
        echo "plan $*" >> "$CALLS_LOG"
        if [ -f "$STATE/plans" ]; then
          plan "Fresh draft"
        else
          : > "$STATE/plans"
          [ -n "$PLAN_SESSION" ] && printf '%s\n' '{"type":"system","subtype":"init","session_id":"'"$PLAN_SESSION"'"}'
          plan "Original draft"
        fi
        ;;
    esac
    ;;
  *)
    echo "implement $*" >> "$CALLS_LOG"
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-impl"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    ;;
esac
"#;

const REQUEST: &str = "add dark mode to the settings screen";

struct Run {
    task: serde_json::Value,
    calls: Vec<String>,
    daemon: Daemon,
    _scripts: tempfile::TempDir,
    _repo: tempfile::TempDir,
}

fn run(plan_session: &str, resume_reply: &str) -> Run {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-resume.sh", FAKE_SCRIPT);
    let log = scripts.path().join("calls.log");
    std::fs::write(&log, "").unwrap();
    let state = scripts.path().join("state");
    std::fs::create_dir_all(&state).unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("CALLS_LOG", log.to_str().unwrap()),
        ("STATE", state.to_str().unwrap()),
        ("PLAN_SESSION", plan_session),
        ("RESUME_REPLY", resume_reply),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["tiers"]["mechanical"] = serde_json::json!("claude-sonnet");
    settings["briefCheckRoute"] = serde_json::json!("claude-sonnet");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let created = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": REQUEST,
            "start": true,
        }),
    );
    let id = created["id"].as_str().unwrap().to_string();
    let task = poll_until(&daemon, &id, |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    let calls = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    Run {
        task,
        calls,
        daemon,
        _scripts: scripts,
        _repo: repo,
    }
}

fn of_kind<'a>(calls: &'a [String], kind: &str) -> Vec<&'a String> {
    let prefix = format!("{kind} ");
    calls.iter().filter(|c| c.starts_with(&prefix)).collect()
}

fn decisions(task: &serde_json::Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_contradiction_resumes_the_planner_session_with_the_conflict() {
    let r = run("sess-plan-1", "");
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    let plans = of_kind(&r.calls, "plan");
    assert_eq!(plans.len(), 1, "calls: {:?}", r.calls);
    assert!(!plans[0].contains("--resume"), "{}", plans[0]);
    let resumed = of_kind(&r.calls, "plan-resume");
    assert_eq!(resumed.len(), 1, "calls: {:?}", r.calls);
    assert!(
        resumed[0].contains("--resume sess-plan-1"),
        "{}",
        resumed[0]
    );
    assert_eq!(of_kind(&r.calls, "check").len(), 2, "calls: {:?}", r.calls);
    // No note on the request; the corrected draft is the stored one.
    assert_eq!(r.task["request"], REQUEST);
    assert_eq!(r.task["title"], "Corrected draft");
    let lines = decisions(&r.task);
    assert!(
        lines.iter().any(|l| l.contains("(drafting again)")),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains("not resumed")),
        "{lines:?}"
    );
    r.daemon.shutdown_and_wait();
}

#[test]
fn without_a_session_id_a_fresh_draft_follows_with_the_request_note() {
    let r = run("", "");
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert!(of_kind(&r.calls, "plan-resume").is_empty(), "{:?}", r.calls);
    assert_eq!(of_kind(&r.calls, "plan").len(), 2, "calls: {:?}", r.calls);
    assert_eq!(r.task["title"], "Fresh draft");
    let request = r.task["request"].as_str().unwrap();
    assert!(request.starts_with(REQUEST), "{request}");
    assert!(request.contains("(Brief check:"), "{request}");
    assert!(decisions(&r.task)
        .iter()
        .any(|l| l.contains("not resumed") && l.contains("no session id")));
    r.daemon.shutdown_and_wait();
}

#[test]
fn an_unparseable_resumed_reply_falls_back_to_a_fresh_draft() {
    let r = run("sess-plan-1", "sorry, no plan");
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(
        of_kind(&r.calls, "plan-resume").len(),
        1,
        "calls: {:?}",
        r.calls
    );
    assert_eq!(of_kind(&r.calls, "plan").len(), 2, "calls: {:?}", r.calls);
    assert_eq!(r.task["title"], "Fresh draft");
    assert!(r.task["request"]
        .as_str()
        .unwrap()
        .contains("(Brief check:"));
    // One redraft per task: the second check found nothing and no third
    // planner run happened.
    assert_eq!(of_kind(&r.calls, "check").len(), 2, "calls: {:?}", r.calls);
    r.daemon.shutdown_and_wait();
}
