//! Black-box integration tests (answer policy): spawn the real `orchd` binary
//! with a fake harness and drive it over its NDJSON unix socket.

mod common;

use common::harness::fake_harness_script;
use common::polling::{poll_until, remove_dir_retrying};
use common::process::Daemon;
use common::repository::init_git_repo;

use serde_json::{json, Value};

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
if head -c 19 "$LOG_DIR/brief.$$" | grep -q 'Route the review'; then exit 0; fi
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

struct Setup {
    daemon: Daemon,
    log: tempfile::TempDir,
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
        log,
        repo: init_git_repo(),
        _scripts: scripts,
    }
}

fn create(s: &Setup, extra: Value) -> String {
    let mut params = json!({
        "repo": s.repo.path().to_str().unwrap(),
        "title": "Policy",
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
    poll_until(&s.daemon, id, |st| {
        matches!(st, "done" | "failed" | "stopped" | "waiting")
    })
}

fn policy_assumptions(task: &Value) -> Vec<Value> {
    task["assumptions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|a| a["by"] == "policy" || a["by"] == "judge")
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn runs(s: &Setup, prefix: &str) -> usize {
    std::fs::read_dir(s.log.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .count()
}

fn cleanup(s: Setup, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap().to_string();
    s.daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_review_that_never_gives_a_verdict_commits_unreviewed_and_asks_nobody() {
    // No review ever gives a verdict: the same route reads the attempt twice,
    // another route once. Then the attempt is committed, marked unreviewed,
    // and nothing re-implements. The owner is not asked.
    let s = setup(&[("PASS_REVIEW_FROM", "99")], |settings| {
        settings["review"] = json!("claude-sonnet");
    });
    let id = create(&s, json!({}));
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "task JSON: {task}");
    assert!(task["question"].is_null(), "{task}");
    assert_eq!(runs(&s, "implement."), 1, "{task}");
    assert!(runs(&s, "review.") >= 2, "{task}");
    let marked: Vec<Value> = policy_assumptions(&task)
        .into_iter()
        .filter(|a| a["kind"] == "review_no_verdict")
        .collect();
    assert_eq!(marked.len(), 1, "{task}");
    assert!(
        task["decisions"]
            .to_string()
            .contains("committed unreviewed"),
        "{task}"
    );
    cleanup(s, &task);
}

#[test]
fn a_review_without_a_verdict_is_reviewed_again_before_anyone_is_asked() {
    for policy in [true, false] {
        let s = setup(&[("PASS_REVIEW_FROM", "1")], |settings| {
            settings["review"] = json!("claude-sonnet");
            settings["answerPolicy"] = json!(policy);
        });
        let id = create(&s, json!({}));
        let task = finished(&s, &id);
        assert_eq!(task["status"], "done", "policy {policy}: {task}");
        assert_eq!(runs(&s, "implement."), 1, "{task}");
        assert_eq!(runs(&s, "review."), 2, "{task}");
        let implements = task["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["stage"] == "implement")
            .count();
        assert_eq!(implements, 1, "{task}");
        assert!(policy_assumptions(&task).is_empty(), "{task}");
        assert!(task["question"].is_null(), "{task}");
        let noted = std::fs::read_dir(s.log.path())
            .unwrap()
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter(|b| b.starts_with("## Review") && b.contains("no parseable verdict"))
            .count();
        assert_eq!(noted, 1, "only the second review carries the note");
        cleanup(s, &task);
    }
}

#[test]
fn an_impossible_claim_with_true_evidence_is_dropped_by_the_judge() {
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
            settings["planReview"] = serde_json::json!("");
        },
    );
    let id = create(
        &s,
        json!({
                        "checks": [{"criterion": 2, "run": "grep -q ok impl2.txt"}],
        }),
    );
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "task JSON: {task}");
    assert_eq!(task["criteria"], json!(["zero", "two"]));
    let assumed = policy_assumptions(&task);
    assert_eq!(assumed.len(), 1, "{task}");
    assert_eq!(assumed[0]["by"], "judge");
    assert_eq!(assumed[0]["kind"], "impossible");
    assert_eq!(assumed[0]["answer"], "drop criterion");
    assert!(assumed[0]["evidence"]
        .as_str()
        .unwrap()
        .contains("does not exist"));
    assert!(s.log.path().join("judge.ran").exists());
    cleanup(s, &task);
}

#[test]
fn a_verified_impossible_claim_on_a_shared_check_criterion_goes_to_the_owner() {
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
            settings["planReview"] = serde_json::json!("");
        },
    );
    let id = create(
        &s,
        json!({
            "criteria": ["zero", "one -- check: npm test", "two"],
                        "checks": [{"criterion": 2, "run": "grep -q ok impl2.txt"}],
        }),
    );
    let task = finished(&s, &id);
    assert_eq!(task["status"], "waiting", "task JSON: {task}");
    assert_eq!(task["question"]["kind"], "impossible", "{task}");
    assert_eq!(task["criteria"].as_array().unwrap().len(), 3, "{task}");
    assert!(policy_assumptions(&task).is_empty(), "{task}");
    assert!(s.log.path().join("judge.ran").exists());
    // The owner (no source) may drop it.
    s.daemon
        .request("task.answer", json!({"id": id, "answer": "drop criterion"}));
    let after = poll_until(&s.daemon, &id, |st| {
        matches!(st, "done" | "failed" | "stopped")
    });
    assert_eq!(after["criteria"].as_array().unwrap().len(), 2, "{after}");
    cleanup(s, &after);
}

#[test]
fn an_impossible_claim_with_false_evidence_is_retried() {
    let s = setup(
        &[
            ("IMPOSSIBLE", "1"),
            (
                "JUDGE_REPLY",
                r#""{\"verified\": false, \"reason\": \"the file exists\"}""#,
            ),
        ],
        |settings| {
            settings["review"] = json!("");
            settings["briefCheckRoute"] = json!("claude-sonnet");
            settings["planReview"] = serde_json::json!("");
        },
    );
    let id = create(
        &s,
        json!({
                        "checks": [{"criterion": 2, "run": "grep -q ok impl2.txt"}],
        }),
    );
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "task JSON: {task}");
    assert_eq!(task["criteria"], json!(["zero", "one", "two"]));
    let assumed = policy_assumptions(&task);
    assert_eq!(assumed.len(), 1, "{task}");
    assert_eq!(assumed[0]["by"], "judge");
    assert_eq!(assumed[0]["answer"], "retry");
    assert!(task["attempts"].as_array().unwrap().len() >= 2, "{task}");
    cleanup(s, &task);
}

#[test]
fn a_protected_path_question_still_reaches_the_owner() {
    let s = setup(&[("PASS_REVIEW_FROM", "0")], |settings| {
        settings["review"] = json!("");
        settings["protectedPaths"] = json!(["change.0.txt"]);
    });
    let id = create(&s, json!({}));
    let task = finished(&s, &id);
    assert_eq!(task["status"], "waiting", "task JSON: {task}");
    assert!(
        task["question"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Change touches protected path"),
        "{task}"
    );
    assert_eq!(task["question"]["kind"], "protected_path");
    assert!(policy_assumptions(&task).is_empty(), "{task}");
    cleanup(s, &task);
}

#[test]
fn a_budget_question_still_reaches_the_owner() {
    let s = setup(&[("COST", "0.5"), ("PASS_REVIEW_FROM", "0")], |settings| {
        settings["review"] = json!("claude-sonnet");
    });
    let id = create(&s, json!({"variant": {"maxCostUsd": 0.1}}));
    let task = finished(&s, &id);
    assert_eq!(task["status"], "waiting", "task JSON: {task}");
    assert_eq!(task["question"]["kind"], "budget", "{task}");
    assert!(policy_assumptions(&task).is_empty(), "{task}");
    cleanup(s, &task);
}

#[test]
fn with_the_policy_off_a_review_without_a_verdict_still_commits_unreviewed() {
    let s = setup(&[("PASS_REVIEW_FROM", "99")], |settings| {
        settings["review"] = json!("claude-sonnet");
        settings["answerPolicy"] = json!(false);
    });
    let id = create(&s, json!({}));
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "task JSON: {task}");
    assert!(task["question"].is_null(), "{task}");
    cleanup(s, &task);
}
