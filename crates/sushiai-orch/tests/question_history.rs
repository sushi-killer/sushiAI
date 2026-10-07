//! Black-box integration tests (question history): who asked each task
//! question, and the per-task history of answered ones. Spawns the real
//! `orchd` binary with a fake harness and drives it over its NDJSON socket.

mod common;

use common::*;
use serde_json::{json, Value};
use std::time::Duration;

/// One fake plays every role, told apart by the brief. `$LOG_DIR` counts the
/// implement runs (`implement.<n>`, the body sees `$n`, the runs before this
/// one) and the reviews (`review.<n>`).
///
/// - a review answers PASS on run `$PASS_REVIEW_FROM` (0-based) and later,
///   and with prose without a verdict before that;
/// - the judge (its brief opens "A coding agent claims") answers `$JUDGE_REPLY`;
/// - the brief check finds nothing, and the healing judges ("You judge ...")
///   rule on nothing;
/// - an implement run edits a file and reports, or with `$IMPOSSIBLE` set
///   claims criterion 1 is impossible on its first run.
const FAKE: &str = r#"#!/bin/sh
cat > "$LOG_DIR/brief.$$"
emit() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":${COST:-0.01},\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
if head -c 9 "$LOG_DIR/brief.$$" | grep -q '## Review'; then
  r=$(ls "$LOG_DIR"/review.* 2>/dev/null | wc -l | tr -d ' ')
  touch "$LOG_DIR/review.$r"
  if [ "$r" -ge "${PASS_REVIEW_FROM:-0}" ]; then
    emit '"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"'
  else
    emit '"Looks fine to me."'
  fi
  exit 0
fi
if grep -q 'A coding agent claims' "$LOG_DIR/brief.$$"; then
  touch "$LOG_DIR/judge.ran"
  emit "$JUDGE_REPLY"
  exit 0
fi
if grep -q 'You judge' "$LOG_DIR/brief.$$"; then
  emit '"{}"'
  exit 0
fi
if grep -q 'You check a coding task brief' "$LOG_DIR/brief.$$"; then
  emit '"{\"contradiction\": false, \"conflict\": \"\"}"'
  exit 0
fi
n=$(ls "$LOG_DIR"/implement.* 2>/dev/null | wc -l | tr -d ' ')
touch "$LOG_DIR/implement.$n"
echo "$n" > "change.$n.txt"
echo ok > impl2.txt
if [ -n "$IMPOSSIBLE" ] && [ "$n" = 0 ]; then
  emit '"```sushi-impossible\n{\"criterion\":1,\"evidence\":\"the premise is wrong\"}\n```\n```sushi-report\n{\"outcome\":\"blocked\",\"summary\":\"cannot\",\"decisions\":[],\"question\":\"\"}\n```"'
else
  emit '"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
fi
"#;

/// A fake that plans with one question and implements trivially.
const PLAN_FAKE: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a toggle\",\"tier\":\"standard\",\"criteria\":[\"Toggle\"],\"verify\":[\"true\"],\"questions\":[{\"text\":\"Which default theme?\",\"options\":[\"light\",\"dark\"],\"recommended\":\"dark\",\"evidence\":\"e\",\"blocking\":true}]}\n```"}'
    ;;
  *)
    echo changed > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    ;;
esac
"#;

struct Setup {
    daemon: Daemon,
    _log: tempfile::TempDir,
    repo: tempfile::TempDir,
    _scripts: tempfile::TempDir,
}

/// A daemon with the answer policy on. `judge` also turns the brief-check
/// route (the judge's route) on.
fn setup(env: &[(&str, &str)], edit: impl Fn(&mut Value)) -> Setup {
    let scripts = tempfile::tempdir().unwrap();
    let path = fake_harness_script(scripts.path(), "fake-claude.sh", FAKE);
    let log = tempfile::tempdir().unwrap();
    let mut vars = vec![
        ("ORCHD_CLAUDE_BIN", path.to_str().unwrap()),
        ("LOG_DIR", log.path().to_str().unwrap()),
    ];
    vars.extend_from_slice(env);
    let daemon = Daemon::spawn(&vars);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["answerPolicy"] = json!(true);
    edit(&mut settings);
    daemon.request("settings.set", json!({"settings": settings}));
    Setup {
        daemon,
        _log: log,
        repo: init_git_repo(),
        _scripts: scripts,
    }
}

fn create(s: &Setup, extra: Value) -> String {
    let mut params = json!({
        "repo": s.repo.path().to_str().unwrap(),
        "title": "History",
        "goal": "Make a change",
        "criteria": ["zero", "one", "two"],
        "verify": ["true"],
    });
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    let task = s.daemon.request("task.create", params);
    let id = task["id"].as_str().unwrap().to_string();
    s.daemon.request("task.start", json!({"id": id}));
    id
}

fn finished(s: &Setup, id: &str) -> Value {
    poll_until(&s.daemon, id, Duration::from_secs(60), |st| {
        matches!(st, "done" | "failed" | "stopped" | "waiting")
    })
}

fn history(task: &Value) -> Vec<Value> {
    task["questionHistory"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn cleanup(s: Setup, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap().to_string();
    s.daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

fn settled(s: &Setup, id: &str) -> Value {
    poll_until(&s.daemon, id, Duration::from_secs(60), |st| {
        matches!(st, "done" | "failed" | "stopped")
    })
}

fn no_verdict_setup() -> Setup {
    setup(&[("PASS_REVIEW_FROM", "99")], |settings| {
        settings["review"] = json!("claude-sonnet");
        settings["answerPolicy"] = json!(false);
    })
}

#[test]
fn a_waiting_question_shows_who_asked_it_and_when() {
    let s = no_verdict_setup();
    let id = create(&s, json!({}));
    let task = finished(&s, &id);
    assert_eq!(task["status"], "waiting", "{task}");
    assert_eq!(task["question"]["kind"], "review_no_verdict");
    assert_eq!(task["question"]["askedBy"], "review", "{task}");
    assert!(task["question"]["askedAt"].is_i64(), "{task}");
    assert!(history(&task).is_empty(), "{task}");
    let listed = s.daemon.request("task.list", json!({}));
    let listed = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(listed["question"]["askedBy"], "review", "{listed}");
    assert!(listed["question"]["askedAt"].is_i64(), "{listed}");
    cleanup(s, &task);
}

#[test]
fn an_owner_answer_to_a_waiting_loop_is_recorded_once() {
    let s = no_verdict_setup();
    let id = create(&s, json!({}));
    let waiting = finished(&s, &id);
    let question = waiting["question"].clone();
    s.daemon
        .request("task.answer", json!({"id": id, "answer": "approve"}));
    let task = settled(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert!(task["question"].is_null(), "{task}");
    let h = history(&task);
    assert_eq!(h.len(), 1, "{task}");
    assert_eq!(h[0]["question"], question["text"]);
    assert_eq!(h[0]["options"], question["options"]);
    assert_eq!(h[0]["kind"], "review_no_verdict");
    assert_eq!(h[0]["askedBy"], "review");
    assert_eq!(h[0]["askedAt"], question["askedAt"]);
    assert_eq!(h[0]["answer"], "approve");
    assert!(h[0]["answeredAt"].is_i64(), "{task}");
    assert_eq!(h[0]["answeredBy"], "owner");
    cleanup(s, &waiting);
}

#[test]
fn an_owner_stop_is_recorded_like_any_answer() {
    let s = no_verdict_setup();
    let id = create(&s, json!({}));
    let waiting = finished(&s, &id);
    s.daemon
        .request("task.answer", json!({"id": id, "answer": "stop"}));
    let task = settled(&s, &id);
    assert_eq!(task["status"], "stopped", "{task}");
    assert!(task["question"].is_null(), "{task}");
    let h = history(&task);
    assert_eq!(h.len(), 1, "{task}");
    assert_eq!(h[0]["answer"], "stop");
    assert_eq!(h[0]["answeredBy"], "owner");
    assert_eq!(h[0]["askedBy"], "review");
    cleanup(s, &waiting);
}

#[test]
fn a_question_dropped_by_task_stop_is_not_history() {
    let s = no_verdict_setup();
    let id = create(&s, json!({}));
    let waiting = finished(&s, &id);
    s.daemon.request("task.stop", json!({"id": id}));
    let task = settled(&s, &id);
    assert_eq!(task["status"], "stopped", "{task}");
    assert!(history(&task).is_empty(), "{task}");
    cleanup(s, &waiting);
}

#[test]
fn a_policy_answer_is_recorded_as_policy() {
    let s = setup(&[("PASS_REVIEW_FROM", "2")], |settings| {
        settings["review"] = json!("claude-sonnet");
    });
    let id = create(&s, json!({}));
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    let h = history(&task);
    assert_eq!(h.len(), 1, "{task}");
    assert_eq!(h[0]["answeredBy"], "policy");
    assert_eq!(h[0]["answer"], "retry");
    assert_eq!(h[0]["askedBy"], "review");
    assert!(h[0]["askedAt"].is_i64(), "{task}");
    cleanup(s, &task);
}

#[test]
fn a_judged_impossible_claim_is_recorded_as_judge() {
    let s = setup(
        &[
            ("IMPOSSIBLE", "1"),
            (
                "JUDGE_REPLY",
                r#""{\"verified\": true, \"reason\": \"the file it names does not exist\"}""#,
            ),
        ],
        |settings| {
            settings["review"] = json!("");
            settings["briefCheckRoute"] = json!("claude-sonnet");
        },
    );
    let id = create(
        &s,
        json!({
            "variant": {"groundedChecks": true},
            "checks": [{"criterion": 2, "run": "grep -q ok impl2.txt"}],
        }),
    );
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    let h = history(&task);
    assert_eq!(h.len(), 1, "{task}");
    assert_eq!(h[0]["kind"], "impossible");
    assert_eq!(h[0]["askedBy"], "implement");
    assert_eq!(h[0]["answer"], "drop criterion");
    assert_eq!(h[0]["answeredBy"], "judge");
    cleanup(s, &task);
}

#[test]
fn a_budget_question_before_an_implement_run_is_asked_by_implement_and_answered_without_a_loop() {
    // Every run costs more than the budget and verify never passes, so the
    // gate before the second implement attempt asks. The daemon is restarted
    // before the answer, so no loop is left to receive it.
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", FAKE);
    let log = tempfile::tempdir().unwrap();
    let env = [
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("LOG_DIR", log.path().to_str().unwrap()),
        ("COST", "0.5"),
    ];
    let data_holder = tempfile::tempdir().unwrap();
    let home = data_holder.path().to_path_buf();
    let socket1 = socket_of(&home);
    let first = spawn_daemon(&home, &env);
    wait_for_socket(&socket1);
    let mut settings = request_on(&socket1, "settings.get", json!({}));
    settings["review"] = json!("");
    settings["briefCheckRoute"] = json!("");
    settings["answerPolicy"] = json!(false);
    fit_sandbox(&mut settings);
    request_on(&socket1, "settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let task = request_on(
        &socket1,
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Budget",
            "goal": "g",
            "criteria": [],
            "verify": ["false"],
            "variant": {"maxCostUsd": 0.1},
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let start = std::time::Instant::now();
    let waiting = loop {
        let t = request_on(&socket1, "task.get", json!({"id": id}));
        if t["status"] == "waiting" {
            break t;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "{t}");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(waiting["question"]["kind"], "budget", "{waiting}");
    assert_eq!(waiting["question"]["askedBy"], "implement", "{waiting}");
    assert!(waiting["question"]["askedAt"].is_i64(), "{waiting}");
    stop_daemon(&socket1);
    let _ = wait_for_exit(first, Duration::from_secs(15));

    let socket2 = socket_of(&home);
    let second = spawn_daemon(&home, &env);
    wait_for_socket(&socket2);
    let answered = request_on(
        &socket2,
        "task.answer",
        json!({"id": id, "answer": "raise"}),
    );
    let h = history(&answered);
    assert_eq!(h.len(), 1, "{answered}");
    assert_eq!(h[0]["kind"], "budget");
    assert_eq!(h[0]["question"], waiting["question"]["text"]);
    assert_eq!(h[0]["options"], waiting["question"]["options"]);
    assert_eq!(h[0]["askedBy"], "implement");
    assert_eq!(h[0]["askedAt"], waiting["question"]["askedAt"]);
    assert_eq!(h[0]["answer"], "raise");
    assert!(h[0]["answeredAt"].is_i64(), "{answered}");
    assert_eq!(h[0]["answeredBy"], "owner");
    stop_daemon(&socket2);
    let _ = wait_for_exit(second, Duration::from_secs(15));
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}

#[test]
fn a_planner_question_is_asked_by_plan_and_recorded_when_answered() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-planner.sh", PLAN_FAKE);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["autoAnswer"] = json!(false);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let waiting = poll_until(&daemon, &id, Duration::from_secs(30), |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    assert_eq!(waiting["status"], "waiting", "{waiting}");
    assert_eq!(waiting["question"]["askedBy"], "plan", "{waiting}");
    assert!(waiting["question"]["askedAt"].is_i64(), "{waiting}");
    assert!(history(&waiting).is_empty(), "{waiting}");
    daemon.request("task.answer", json!({"id": id, "answer": "dark"}));
    let done = poll_until(&daemon, &id, Duration::from_secs(30), |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    let h = history(&done);
    assert_eq!(h.len(), 1, "{done}");
    assert_eq!(h[0]["question"], "Which default theme?");
    assert_eq!(h[0]["kind"], "plan_question");
    assert_eq!(h[0]["askedBy"], "plan");
    assert_eq!(h[0]["answer"], "dark");
    assert_eq!(h[0]["answeredBy"], "owner");
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}
