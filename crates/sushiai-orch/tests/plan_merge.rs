//! Black-box integration tests (plan merge): subtasks that touch the same
//! file become one task with an ordered checklist; parts with their own
//! paths and checks stay children.

mod common;

use common::*;

/// Plans the request as `$PARTS` (a ready `subtasks` JSON array body), plans
/// a child from its `PART_` marker, and implements by writing the lines the
/// brief asks for. `$ATTEMPT_LINES` is how many lines of `shared.txt` the
/// first attempt writes; later attempts write five. Every run of the task's
/// verify command appends the line count of `shared.txt` to `$VERIFY_LOG`.
const MERGE_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
emit() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"'"$1"'"}'
}
report='```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```'
case "$input" in
  *sushi-plan*)
    case "$input" in
      *PART_X*) emit '```sushi-plan\n{\"title\":\"Part X\",\"goal\":\"PART_X: x.txt\",\"verify\":[\"test -f x.txt\"]}\n```' ;;
      *PART_Y*) emit '```sushi-plan\n{\"title\":\"Part Y\",\"goal\":\"PART_Y: y.txt\",\"verify\":[\"test -f y.txt\"]}\n```' ;;
      *) emit '```sushi-plan\n{\"title\":\"Whole\",\"goal\":\"WHOLE build it\",\"verify\":[\"echo $(cat shared.txt 2>/dev/null | wc -l) >> '"$VERIFY_LOG"'; true\"],\"subtasks\":'"$PARTS"'}\n```' ;;
    esac
    ;;
  *PART_X*) echo x > x.txt; emit "$report" ;;
  *PART_Y*) echo y > y.txt; emit "$report" ;;
  *)
    n=$(ls "$ATTEMPTS_DIR" 2>/dev/null | wc -l | tr -d ' ')
    touch "$ATTEMPTS_DIR/attempt.$$"
    lines=5
    if [ "$n" = "0" ]; then lines="$ATTEMPT_LINES"; fi
    i=1
    : > shared.txt
    while [ "$i" -le "$lines" ]; do echo "$i" >> shared.txt; i=$((i+1)); done
    emit "$report"
    ;;
esac
"#;

const FIVE_PARTS: &str = r#"[{\"key\":\"s1\",\"title\":\"One\",\"request\":\"STEP_1 write line 1\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 1 shared.txt\"},{\"key\":\"s2\",\"title\":\"Two\",\"request\":\"STEP_2 write line 2\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 2 shared.txt\",\"dependsOn\":[\"s1\"]},{\"key\":\"s3\",\"title\":\"Three\",\"request\":\"STEP_3 write line 3\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 3 shared.txt\"},{\"key\":\"s4\",\"title\":\"Four\",\"request\":\"STEP_4 write line 4\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 4 shared.txt\"},{\"key\":\"s5\",\"title\":\"Five\",\"request\":\"STEP_5 write line 5\",\"paths\":[\"shared.txt\"],\"check\":\"grep -qx 5 shared.txt\"}]"#;

const TWO_PARTS: &str = r#"[{\"key\":\"x\",\"title\":\"Part X\",\"request\":\"PART_X write x.txt\",\"paths\":[\"x.txt\"],\"check\":\"test -f x.txt\"},{\"key\":\"y\",\"title\":\"Part Y\",\"request\":\"PART_Y write y.txt\",\"paths\":[\"y.txt\"],\"check\":\"test -f y.txt\"}]"#;

struct Run {
    task: serde_json::Value,
    verify_log: Vec<String>,
    daemon: Daemon,
    _dir: tempfile::TempDir,
    _repo: tempfile::TempDir,
}

fn run(parts: &str, first_attempt_lines: &str) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(dir.path(), "fake-merge.sh", MERGE_SCRIPT);
    let attempts = dir.path().join("attempts");
    std::fs::create_dir_all(&attempts).unwrap();
    let verify_log = dir.path().join("verify.log");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS", parts),
        ("ATTEMPTS_DIR", attempts.to_str().unwrap()),
        ("ATTEMPT_LINES", first_attempt_lines),
        ("VERIFY_LOG", verify_log.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["planner"] = serde_json::json!("claude-sonnet");
    settings["tiers"]["hard"] = serde_json::json!("claude-sonnet");
    settings["tiers"]["standard"] = serde_json::json!("claude-sonnet");
    settings["tiers"]["mechanical"] = serde_json::json!("claude-sonnet");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let created = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build it in steps",
            "start": true,
        }),
    );
    let id = created["id"].as_str().unwrap().to_string();
    let task = poll_until(&daemon, &id, |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    let verify_log = std::fs::read_to_string(&verify_log)
        .map(|t| t.lines().map(|l| l.trim().to_string()).collect())
        .unwrap_or_default();
    Run {
        task,
        verify_log,
        daemon,
        _dir: dir,
        _repo: repo,
    }
}

fn children(r: &Run) -> Vec<serde_json::Value> {
    let listed = r
        .daemon
        .request("task.list", serde_json::json!({"repo": r.task["repo"]}));
    listed
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["parent"] == r.task["id"])
        .cloned()
        .collect()
}

#[test]
fn five_same_file_subtasks_become_one_task_with_a_five_item_checklist() {
    let r = run(FIVE_PARTS, "5");
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert!(children(&r).is_empty(), "no children expected");
    let checklist = r.task["checklist"].as_array().unwrap();
    assert_eq!(checklist.len(), 5, "{}", r.task);
    for (i, item) in checklist.iter().enumerate() {
        assert!(
            item["text"]
                .as_str()
                .unwrap()
                .contains(&format!("STEP_{}", i + 1)),
            "{item}"
        );
        assert_eq!(item["passes"], true, "{item}");
    }
    assert!(
        decisions_of(&r.task)
            .iter()
            .any(|l| l == "Plan gate: merged 5 subtasks into one task with a checklist"),
        "{}",
        r.task
    );
    r.daemon.shutdown_and_wait();
}

#[test]
fn two_disjoint_parts_stay_two_children() {
    let r = run(TWO_PARTS, "5");
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    let kids = children(&r);
    assert_eq!(kids.len(), 2, "{kids:?}");
    assert!(r.task["checklist"].is_null(), "{}", r.task);
    for kid in &kids {
        assert_eq!(kid["status"], "done", "{kid}");
        assert!(
            kid["request"].as_str().unwrap().contains("Check: test -f"),
            "{kid}"
        );
    }
    r.daemon.shutdown_and_wait();
}

#[test]
fn a_failing_checklist_item_fails_the_attempt_by_name_and_the_full_verify_runs_after() {
    // The first attempt writes four lines: item five fails.
    let r = run(FIVE_PARTS, "4");
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    let attempts: Vec<&serde_json::Value> = r.task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert_eq!(attempts.len(), 2, "{}", r.task);
    assert_eq!(attempts[0]["failure"]["kind"], "verify", "{}", attempts[0]);
    let detail = attempts[0]["failure"]["detail"].as_str().unwrap();
    assert!(detail.contains("STEP_5"), "{detail}");
    assert!(!detail.contains("STEP_4"), "{detail}");
    // The full verify never saw the four-line file; it ran on the five-line one.
    assert!(
        !r.verify_log.contains(&"4".to_string()),
        "{:?}",
        r.verify_log
    );
    assert!(
        r.verify_log.contains(&"5".to_string()),
        "{:?}",
        r.verify_log
    );
    r.daemon.shutdown_and_wait();
}

fn decisions_of(task: &serde_json::Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str().map(str::to_string))
        .collect()
}
