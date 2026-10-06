//! Black-box integration tests (task results): per-criterion results, diff
//! stats, `landedAt` and the `task.log` RPC, over the real `orchd` binary and
//! its NDJSON unix socket.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// One fake plays every role. `$LOG_DIR` counts implement runs
/// (`implement.<n>`) and review runs (`review.<n>`). The implement body sees
/// `$n`, the number of implement runs before this one; a review answers
/// `$FIRST_REVIEW` the first time and `$NEXT_REVIEW` after that.
fn fake(implement: &str, first_review: &str, next_review: &str) -> String {
    format!(
        r###"#!/bin/sh
input="$(cat)"
case "$input" in
  "## Review"*)
    r=$(ls "$LOG_DIR"/review.* 2>/dev/null | wc -l | tr -d ' ')
    touch "$LOG_DIR/review.$r"
    if [ "$r" = 0 ]; then
      printf '%s\n' '{{"type":"result","result":"```sushi-review\n{first_review}\n```"}}'
    else
      printf '%s\n' '{{"type":"result","result":"```sushi-review\n{next_review}\n```"}}'
    fi
    exit 0 ;;
  *sushi-plan*)
    printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-plan"}}'
    printf '%s\n' '{{"type":"result","result":"```sushi-plan\n{{\"title\":\"Planned\",\"goal\":\"Add lines\",\"criteria\":[\"Lines added\"],\"verify\":[\"true\"]}}\n```"}}'
    exit 0 ;;
esac
n=$(ls "$LOG_DIR"/implement.* 2>/dev/null | wc -l | tr -d ' ')
touch "$LOG_DIR/implement.$n"
echo "noise on stderr" >&2
{implement}
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-fake"}}'
printf '%s\n' '{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":1,"output_tokens":1}},"result":"```sushi-report\n{{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}}\n```"}}'
"###
    )
}

/// The first attempt adds three lines to a tracked file, creates an untracked
/// two-line file and `beta.txt`; the second adds `alpha.txt`.
const IMPLEMENT: &str = r#"if [ "$n" = 0 ]; then
  printf 'l1\nl2\nl3\n' >> README.md
  printf 'x\ny\n' > new.txt
  echo b > beta.txt
elif [ "$n" = 1 ]; then
  echo a > alpha.txt
fi"#;

const PASS_REVIEW: &str = r#"{\"verdict\":\"PASS\",\"findings\":[]}"#;

struct Setup {
    daemon: Daemon,
    _log: tempfile::TempDir,
    _scripts: tempfile::TempDir,
}

fn setup(script: &str, review: bool) -> Setup {
    let scripts = tempfile::tempdir().unwrap();
    let path = fake_harness_script(scripts.path(), "fake-claude.sh", script);
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", path.to_str().unwrap()),
        ("LOG_DIR", log.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!(if review { "claude-opus" } else { "" });
    settings["answerPolicy"] = json!(false);
    daemon.request("settings.set", json!({"settings": settings}));
    Setup {
        daemon,
        _log: log,
        _scripts: scripts,
    }
}

fn implement_attempts(task: &Value) -> Vec<Value> {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .cloned()
        .collect()
}

fn statuses(attempt: &Value) -> Vec<String> {
    attempt["criteriaResults"]
        .as_array()
        .unwrap_or_else(|| panic!("no criteriaResults: {attempt}"))
        .iter()
        .map(|r| r["status"].as_str().unwrap().to_string())
        .collect()
}

fn cleanup(s: Setup, tasks: &[&Value]) {
    let worktrees: Vec<String> = tasks
        .iter()
        .map(|t| t["worktree"].as_str().unwrap().to_string())
        .collect();
    s.daemon.shutdown_and_wait();
    for wt in worktrees {
        let _ = std::fs::remove_dir_all(wt);
    }
}

fn write_task(data: &Path, id: &str, extra: Value) {
    let mut task = json!({
        "id": id, "title": "Fixture", "goal": "g", "criteria": ["c one"], "verify": ["true"],
        "repo": "/repo", "worktree": "/nowhere", "branch": "b", "baseSha": "0",
        "status": "done", "tier": "standard", "attempts": [],
        "createdAt": 1_000, "updatedAt": 90_000,
    });
    for (k, v) in extra.as_object().unwrap() {
        task[k] = v.clone();
    }
    let dir = data.join("tasks").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("task.json"), task.to_string()).unwrap();
}

fn attempt(n: u32, stage: &str, status: &str) -> Value {
    json!({
        "n": n, "stage": stage, "routeId": "claude-sonnet", "harness": "claude",
        "model": "sonnet", "reason": "test", "startedAt": 1_000, "endedAt": 2_000,
        "status": status,
    })
}

#[test]
fn criteria_results_follow_checks_and_review_rulings_and_diff_stats_are_exact() {
    let rulings_first = r#"{\"verdict\":\"FAIL\",\"findings\":[],\"criteria\":[{\"criterion\":\"Alpha file exists\",\"met\":false,\"evidence\":\"not in the diff\"},{\"criterion\":\"Gamma is documented\",\"met\":true,\"evidence\":\"README\"},{\"criterion\":\"Delta is fast\",\"met\":null},{\"criterion\":\"Epsilon is safe\",\"met\":false}]}"#;
    let rulings_next = r#"{\"verdict\":\"PASS\",\"findings\":[],\"criteria\":[{\"criterion\":\"Alpha file exists\",\"met\":true},{\"criterion\":\"Gamma is documented\",\"met\":true},{\"criterion\":\"Delta is fast\",\"met\":null},{\"criterion\":\"Epsilon is safe\",\"met\":true}]}"#;
    let s = setup(&fake(IMPLEMENT, rulings_first, rulings_next), true);
    let repo = init_git_repo();
    let task = s.daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Grounded",
            "goal": "Make the checks pass",
            "variant": {"groundedChecks": true},
            "criteria": [
                "Alpha file exists -- check: test -f alpha.txt",
                "Beta file exists -- check: test -f beta.txt",
                "Gamma is documented",
                "Delta is fast",
                "Epsilon is safe",
            ],
            "verify": ["true"],
            "checks": [
                {"criterion": 0, "run": "test -f alpha.txt"},
                {"criterion": 1, "run": "test -f beta.txt"},
            ],
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let done = settle(&s.daemon, &id);
    assert_eq!(done["status"], "done", "task JSON: {done}");

    let attempts = implement_attempts(&done);
    assert_eq!(attempts.len(), 3, "{done}");
    // Attempt 1: the alpha check failed, beta's passed, nothing was reviewed.
    assert_eq!(attempts[0]["status"], "failed");
    assert_eq!(
        statuses(&attempts[0]),
        ["failing", "met", "pending", "pending", "pending"]
    );
    // Attempt 2: both checks passed, but the review ruled alpha unmet (which
    // beats the passing check) and epsilon unmet, gamma met and delta null.
    assert_eq!(
        statuses(&attempts[1]),
        ["failing", "met", "met", "pending", "failing"]
    );
    let alpha = &attempts[1]["criteriaResults"][0];
    assert_eq!(alpha["criterion"], 0);
    assert_eq!(alpha["text"], done["criteria"][0]);
    assert_eq!(alpha["evidence"], "not in the diff");
    assert_eq!(attempts[1]["review"]["criteria"][1]["met"], true);
    assert!(attempts[1]["review"]["criteria"][2]["met"].is_null());
    // The latest implement attempt is the current state.
    assert_eq!(attempts[2]["status"], "passed");
    assert_eq!(
        statuses(&attempts[2]),
        ["met", "met", "met", "pending", "met"]
    );

    // Diff stats: README +3, new.txt +2 and beta.txt +1, then alpha.txt +1.
    assert_eq!(
        attempts[0]["diffStat"],
        json!({"files": 3, "added": 6, "removed": 0})
    );
    assert_eq!(
        attempts[1]["diffStat"],
        json!({"files": 4, "added": 7, "removed": 0})
    );
    assert_eq!(
        done["diffStat"],
        json!({"files": 4, "added": 7, "removed": 0}),
        "the branch against its base: {done}"
    );
    assert!(done.get("landedAt").is_none(), "not a landing task: {done}");
    cleanup(s, &[&done]);
}

#[test]
fn task_log_replays_the_implement_plan_and_review_stages() {
    let s = setup(&fake(IMPLEMENT, PASS_REVIEW, PASS_REVIEW), true);
    let repo = init_git_repo();
    let task = s.daemon.request(
        "task.create",
        json!({"repo": repo.path().to_str().unwrap(), "request": "add some lines"}),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let done = settle(&s.daemon, &id);
    assert_eq!(done["status"], "done", "task JSON: {done}");
    let plan = done["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["stage"] == "plan")
        .unwrap_or_else(|| panic!("no plan attempt: {done}"));
    let implement = &implement_attempts(&done)[0];

    let log = |attempt: &Value, stage: &str| {
        s.daemon.request(
            "task.log",
            json!({"id": id, "attempt": attempt["n"], "stage": stage}),
        )
    };
    // The stderr line in events.jsonl is not a stream line and is skipped,
    // exactly as it never reached the live `log` event.
    let run = log(implement, "implement");
    assert_eq!(
        run,
        json!({"lines": ["session started", "session finished"], "truncated": false})
    );
    let planned = log(plan, "plan");
    assert_eq!(
        planned,
        json!({"lines": ["session started", "session finished"], "truncated": false})
    );
    let review = log(implement, "review");
    assert_eq!(
        review,
        json!({"lines": ["session finished"], "truncated": false}),
        "{review}"
    );
    cleanup(s, &[&done]);
}

#[test]
fn task_log_errors_on_a_running_attempt_a_stage_that_did_not_run_and_an_unknown_task() {
    let daemon = Daemon::spawn(&[]);
    let id = "33333333-3333-4333-8333-333333333333";
    write_task(
        daemon.data_dir(),
        id,
        json!({"attempts": [
            attempt(1, "plan", "passed"),
            attempt(2, "implement", "passed"),
            attempt(3, "implement", "running"),
        ]}),
    );
    let run2 = daemon.data_dir().join("tasks").join(id).join("runs/2");
    std::fs::create_dir_all(&run2).unwrap();
    std::fs::write(
        run2.join("events.jsonl"),
        "{\"type\":\"system\",\"subtype\":\"init\"}\n",
    )
    .unwrap();

    let err = |attempt: u32, stage: &str| {
        daemon.request_error(
            "task.log",
            json!({"id": id, "attempt": attempt, "stage": stage}),
        )
    };
    assert!(err(3, "implement").contains("still running"));
    // Plan on an implement attempt, implement on a plan attempt.
    assert!(err(2, "plan").contains("no plan stage"));
    assert!(err(1, "implement").contains("no implement stage"));
    // Stages that never ran have no stored log.
    assert!(err(2, "review").contains("no stored log"));
    assert!(err(2, "advisor").contains("no stored log"));
    assert!(err(1, "plan").contains("no stored log"));
    assert!(err(9, "implement").contains("not found"));
    assert!(err(2, "verify").contains("unknown stage"));
    let unknown = daemon.request_error(
        "task.log",
        json!({"id": "44444444-4444-4444-8444-444444444444", "attempt": 1, "stage": "implement"}),
    );
    assert!(unknown.contains("task not found"), "{unknown}");
    let invalid = daemon.request_error(
        "task.log",
        json!({"id": "../x", "attempt": 1, "stage": "implement"}),
    );
    assert!(invalid.contains("invalid task id"), "{invalid}");

    let ok = daemon.request(
        "task.log",
        json!({"id": id, "attempt": 2, "stage": "implement"}),
    );
    assert_eq!(
        ok,
        json!({"lines": ["session started"], "truncated": false})
    );
    daemon.shutdown_and_wait();
}

#[test]
fn task_log_keeps_only_the_last_ten_thousand_lines() {
    let daemon = Daemon::spawn(&[]);
    let id = "55555555-5555-4555-8555-555555555555";
    write_task(
        daemon.data_dir(),
        id,
        json!({"attempts": [attempt(1, "implement", "passed")]}),
    );
    let run = daemon.data_dir().join("tasks").join(id).join("runs/1");
    std::fs::create_dir_all(&run).unwrap();
    let mut body = "{\"type\":\"system\",\"subtype\":\"init\"}\n".repeat(5);
    body.push_str(&"{\"type\":\"result\"}\n".repeat(10_000));
    std::fs::write(run.join("events.jsonl"), body).unwrap();
    let out = daemon.request(
        "task.log",
        json!({"id": id, "attempt": 1, "stage": "implement"}),
    );
    let lines = out["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 10_000);
    assert!(
        lines.iter().all(|l| l == "session finished"),
        "the first lines were dropped"
    );
    assert_eq!(out["truncated"], true);
    daemon.shutdown_and_wait();
}

#[test]
fn a_task_json_written_before_these_fields_loads_and_leaves_them_out() {
    let daemon = Daemon::spawn(&[]);
    let id = "66666666-6666-4666-8666-666666666666";
    write_task(
        daemon.data_dir(),
        id,
        json!({"attempts": [attempt(1, "implement", "passed")]}),
    );
    let task = daemon.request("task.get", json!({"id": id}));
    assert_eq!(task["id"], id);
    assert!(
        task.get("diffStat").is_none() && task.get("landedAt").is_none(),
        "{task}"
    );
    let a = &task["attempts"][0];
    assert!(
        a.get("criteriaResults").is_none() && a.get("diffStat").is_none(),
        "{a}"
    );
    daemon.shutdown_and_wait();
}

const LAND_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
name=$(printf '%s' "$input" | grep -o 'FILE_[a-z]*' | head -1 | sed 's/FILE_//')
[ -n "$name" ] && echo "$name" > "$name.txt"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

#[test]
fn a_task_that_lands_on_its_base_branch_gets_landed_at_and_the_landing_diff_stat() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", LAND_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Add one",
            "goal": "FILE_one: write the file",
            "verify": ["test -f one.txt"],
            "land": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let done = settle(&daemon, &id);
    assert_eq!(done["status"], "done", "task JSON: {done}");
    assert!(done["landedSha"].is_string(), "{done}");
    let landed_at = done["landedAt"]
        .as_i64()
        .unwrap_or_else(|| panic!("{done}"));
    assert!(landed_at >= done["createdAt"].as_i64().unwrap());
    assert_eq!(
        done["diffStat"],
        json!({"files": 1, "added": 1, "removed": 0})
    );
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(done["worktree"].as_str().unwrap());
}

const GRAPH_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
case "$input" in
  *sushi-plan*)
    case "$input" in
      *PART_A*) out '"```sushi-plan\n{\"title\":\"Part A\",\"goal\":\"PART_A: create a.txt\",\"verify\":[\"test -f a.txt\"]}\n```"' ;;
      *PART_B*) out '"```sushi-plan\n{\"title\":\"Part B\",\"goal\":\"PART_B: create b.txt next to a.txt\",\"verify\":[\"test -f a.txt && test -f b.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"Both parts\",\"goal\":\"Create a.txt, then b.txt\",\"verify\":[\"test -f a.txt && test -f b.txt\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"request\":\"PART_A create a.txt\"},{\"key\":\"b\",\"title\":\"Part B\",\"request\":\"PART_B create b.txt\",\"dependsOn\":[\"a\"]}]}\n```"' ;;
    esac
    ;;
  *PART_A*) echo a > a.txt; out "$report" ;;
  *PART_B*) test -f a.txt && echo b > b.txt; out "$report" ;;
esac
"#;

#[test]
fn subtasks_that_land_on_their_parent_get_landed_at_and_their_own_diff_stat() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", GRAPH_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["answerPolicy"] = json!(false);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        json!({"repo": repo.path().to_str().unwrap(), "request": "build both parts", "start": true}),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let asked = settle(&daemon, &parent_id);
    assert_eq!(asked["question"]["kind"], "preexisting_failure", "{asked}");
    daemon.request(
        "task.answer",
        json!({"id": parent_id, "answer": "keep this check"}),
    );
    let parent = poll_until(&daemon, &parent_id, Duration::from_secs(30), |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    let tasks = daemon.request("task.list", json!({"repo": parent["repo"]}));
    let children: Vec<&Value> = tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["parent"] == parent_id.as_str())
        .collect();
    assert_eq!(children.len(), 2, "tasks: {tasks}");
    for child in &children {
        assert_eq!(child["status"], "done", "task JSON: {child}");
        let landed_at = child["landedAt"]
            .as_i64()
            .unwrap_or_else(|| panic!("{child}"));
        assert!(landed_at >= child["createdAt"].as_i64().unwrap());
        assert_eq!(
            child["diffStat"],
            json!({"files": 1, "added": 1, "removed": 0}),
            "{child}"
        );
    }
    daemon.shutdown_and_wait();
    for t in std::iter::once(&parent).chain(children.iter().copied()) {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}
