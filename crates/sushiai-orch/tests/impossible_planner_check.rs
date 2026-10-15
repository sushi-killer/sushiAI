//! Black-box integration tests: an impossible claim on a criterion whose
//! checks the planner wrote is settled without the owner (the answer judge
//! drops or rewrites it, or orchd drops it when no judge turn can), while a
//! criterion with the repo's or the owner's check still asks the owner. Spawns
//! the real `orchd` binary with a fake harness.

mod common;

use common::*;
use serde_json::{json, Value};

/// One fake plays every role, told apart by the brief; each reply is a JSON
/// string literal.
///
/// - the planner (its brief asks for `sushi-plan`) answers `$LOG_DIR/plan.json`;
/// - the answer judges leave `judge.ran` and answer `$JUDGE_REPLY` (the
///   planner-check judge leaves `planner-judge.ran` too);
/// - the plan review (on the planner's route when `planReview` is empty) and
///   the brief check find nothing, every other helper rules on nothing;
/// - implement run `n` (0-based) writes impl.txt, claims criterion 1
///   impossible on run 0, and evals `$IMPL_REST` on later runs.
const FAKE: &str = r#"#!/bin/sh
B="$LOG_DIR/brief.$$"
cat > "$B"
emit() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
if head -c 19 "$B" | grep -q 'Route the review'; then exit 0; fi
if head -c 9 "$B" | grep -q '## Review'; then
  emit '"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"'
  exit 0
fi
if grep -q 'written by the task.s planner' "$B"; then
  touch "$LOG_DIR/planner-judge.ran" "$LOG_DIR/judge.ran"
  emit "$JUDGE_REPLY"
  exit 0
fi
if grep -q 'A coding agent claims' "$B"; then
  touch "$LOG_DIR/judge.ran"
  emit "$JUDGE_REPLY"
  exit 0
fi
if head -c 40 "$B" | grep -q 'You review a coding task plan'; then
  emit '"{\"findings\": []}"'
  exit 0
fi
if grep -q 'sushi-plan' "$B"; then
  emit "$(cat "$LOG_DIR/plan.json")"
  exit 0
fi
if grep -q 'You judge' "$B"; then
  emit '"{}"'
  exit 0
fi
if grep -q 'You check a coding task brief' "$B"; then
  emit '"{\"contradiction\": false, \"conflict\": \"\"}"'
  exit 0
fi
# Only an implement brief (a fresh one offers the impossible block, a resumed
# one opens with the failed checks); any other helper rules on nothing.
if ! grep -q -e 'sushi-impossible' -e '^## Failed checks' "$B"; then
  emit '"{}"'
  exit 0
fi
n=$(ls "$LOG_DIR"/implement.* 2>/dev/null | wc -l | tr -d ' ')
touch "$LOG_DIR/implement.$n"
echo ok > impl.txt
if [ "$n" = 0 ]; then
  emit '"```sushi-impossible\n{\"criterion\":1,\"evidence\":\"the design has no two.txt\"}\n```\n```sushi-report\n{\"outcome\":\"blocked\",\"summary\":\"cannot\",\"decisions\":[],\"question\":\"\"}\n```"'
  exit 0
fi
eval "${IMPL_REST:-:}"
emit '"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
"#;

/// The planner's check on criterion 1.
const PLANNER_CHECK: &str = "grep -q ok two.txt";
/// The check the judge rewrites it to.
const NEW_CHECK: &str = "grep -q ok three.txt";

struct Setup {
    daemon: Daemon,
    log: tempfile::TempDir,
    repo: tempfile::TempDir,
    _scripts: tempfile::TempDir,
}

/// A daemon whose answer policy is `policy`; `judge` turns the brief-check
/// route (the judge's route) on. `files` are committed to the repo.
fn setup(env: &[(&str, &str)], policy: bool, judge: bool, files: &[(&str, &str)]) -> Setup {
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
    settings["answerPolicy"] = json!(policy);
    settings["briefCheckRoute"] = json!(if judge { "claude-sonnet" } else { "" });
    settings["planReview"] = json!("");
    settings["review"] = json!("");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    for (name, body) in files {
        std::fs::write(repo.path().join(name), body).unwrap();
    }
    if !files.is_empty() {
        git_out(repo.path(), &["add", "."]);
        git_out(repo.path(), &["commit", "-q", "-m", "fixtures"]);
    }
    Setup {
        daemon,
        log,
        repo,
        _scripts: scripts,
    }
}

/// A JSON string literal, as the fake's `emit` wants its reply.
fn js(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

/// The plan the fake planner answers: criterion 1 is checked by `check`, and
/// `verify` lists it too when `in_verify`.
fn plan(check: &str, in_verify: bool) -> Value {
    let mut verify = vec!["true".to_string()];
    if in_verify {
        verify.push(check.to_string());
    }
    json!({
        "title": "Two files",
        "goal": "Write impl.txt and two.txt",
        "tier": "standard",
        "criteria": ["impl.txt exists", format!("two.txt says ok -- check: {check}")],
        "verify": verify,
        "checks": [
            {"criterion": 0, "run": "test -f impl.txt"},
            {"criterion": 1, "run": check},
        ],
    })
}

/// Creates and starts a task the fake planner drafts from `plan`.
fn create_planned(s: &Setup, plan: Value, request: &str) -> String {
    std::fs::write(
        s.log.path().join("plan.json"),
        js(&format!("```sushi-plan\n{plan}\n```")),
    )
    .unwrap();
    let task = s.daemon.request(
        "task.create",
        json!({
            "repo": s.repo.path().to_str().unwrap(),
            "request": request,
            "start": true,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn finished(s: &Setup, id: &str) -> Value {
    settle(&s.daemon, id)
}

fn automatic_answers(task: &Value) -> Vec<Value> {
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

fn implements(task: &Value) -> usize {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .count()
}

fn source_of(task: &Value, command: &str) -> Value {
    task["plannerTests"]["sources"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["command"] == command))
        .map(|s| s["source"].clone())
        .unwrap_or(Value::Null)
}

/// No question reached the owner: none is open and none in the history was
/// answered by the owner.
fn assert_no_owner_question(task: &Value) {
    assert!(task["question"].is_null(), "{task}");
    let by_owner = task["questionHistory"]
        .as_array()
        .map(|h| h.iter().filter(|q| q["answeredBy"] == "owner").count())
        .unwrap_or(0);
    assert_eq!(by_owner, 0, "{task}");
}

fn assert_criterion_dropped(task: &Value) {
    assert_eq!(task["status"], "done", "task JSON: {task}");
    assert_no_owner_question(task);
    assert_eq!(task["criteria"], json!(["impl.txt exists"]), "{task}");
    let runs: Vec<&str> = task["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["run"].as_str())
        .collect();
    assert_eq!(runs, vec!["test -f impl.txt"], "{task}");
}

fn cleanup(s: Setup, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap_or_default().to_string();
    s.daemon.shutdown_and_wait();
    if !worktree.is_empty() {
        remove_dir_retrying(worktree);
    }
}

#[test]
fn a_planner_check_criterion_judged_drop_is_dropped_without_the_owner() {
    // The answer policy is off: the judge still runs for a planner check.
    let reply = js(
        r#"{"verdict": "drop", "command": "", "criterionText": "", "reason": "the design has no second file"}"#,
    );
    let s = setup(&[("JUDGE_REPLY", &reply)], false, true, &[]);
    let id = create_planned(&s, plan(PLANNER_CHECK, false), "write the files");
    let task = finished(&s, &id);
    assert_criterion_dropped(&task);
    assert!(s.log.path().join("planner-judge.ran").exists());
    let answers = automatic_answers(&task);
    assert_eq!(answers.len(), 1, "{task}");
    assert_eq!(answers[0]["by"], "judge");
    assert_eq!(answers[0]["kind"], "impossible");
    assert_eq!(answers[0]["answer"], "drop criterion");
    assert_eq!(answers[0]["overturned"], false);
    assert!(answers[0]["evidence"]
        .as_str()
        .unwrap()
        .contains("the design has no second file"));
    assert!(
        task["decisions"]
            .to_string()
            .contains("dropped criterion 1"),
        "{task}"
    );
    cleanup(s, &task);
}

#[test]
fn a_planner_check_criterion_judged_rewrite_gets_the_new_check_and_continues() {
    let reply = js(&format!(
        r#"{{"verdict": "rewrite", "command": "{NEW_CHECK}", "criterionText": "", "reason": "the design writes three.txt"}}"#
    ));
    // Later runs meet the rewritten check, never the old one.
    let s = setup(
        &[
            ("JUDGE_REPLY", &reply),
            ("IMPL_REST", "echo ok > three.txt"),
        ],
        true,
        true,
        &[],
    );
    let id = create_planned(&s, plan(PLANNER_CHECK, false), "write the files");
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "task JSON: {task}");
    assert_no_owner_question(&task);
    assert!(implements(&task) >= 2, "{task}");
    assert_eq!(
        task["criteria"],
        json!([
            "impl.txt exists",
            format!("two.txt says ok -- check: {NEW_CHECK}")
        ]),
        "{task}"
    );
    let checks: Vec<(u64, &str)> = task["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["criterion"].as_u64().unwrap(), c["run"].as_str().unwrap()))
        .collect();
    assert_eq!(
        checks,
        vec![(0, "test -f impl.txt"), (1, NEW_CHECK)],
        "{task}"
    );
    assert_eq!(task["checks"][1]["baseline"], "fail", "{task}");
    assert_eq!(source_of(&task, NEW_CHECK), "planner", "{task}");
    let line = task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .find(|d| d.contains("rewritten to"))
        .unwrap_or_else(|| panic!("no rewrite line: {task}"))
        .to_string();
    assert!(
        line.contains(PLANNER_CHECK)
            && line.contains(NEW_CHECK)
            && line.contains("the design writes three.txt"),
        "{line}"
    );
    let answers = automatic_answers(&task);
    assert_eq!(answers.len(), 1, "{task}");
    assert_eq!(answers[0]["by"], "judge");
    assert_eq!(answers[0]["kind"], "impossible");
    assert_eq!(answers[0]["overturned"], false);
    cleanup(s, &task);
}

#[test]
fn a_planner_check_criterion_without_a_judge_is_dropped_without_the_owner() {
    // No judge route with the policy off, and a judge whose reply is unusable
    // with the policy on: orchd drops the criterion itself, as an assumption.
    for (policy, judge, why) in [
        (false, false, "no judge route"),
        (true, true, "no usable reply"),
    ] {
        let reply = js("I am not sure.");
        let s = setup(&[("JUDGE_REPLY", &reply)], policy, judge, &[]);
        let id = create_planned(&s, plan(PLANNER_CHECK, false), "write the files");
        let task = finished(&s, &id);
        assert_criterion_dropped(&task);
        assert_eq!(s.log.path().join("judge.ran").exists(), judge, "{task}");
        let answers = automatic_answers(&task);
        assert_eq!(answers.len(), 1, "{task}");
        assert_eq!(answers[0]["by"], "policy");
        assert_eq!(answers[0]["kind"], "impossible");
        assert_eq!(answers[0]["answer"], "drop criterion");
        assert_eq!(answers[0]["overturned"], false);
        let evidence = answers[0]["evidence"].as_str().unwrap();
        assert!(evidence.contains(why), "{evidence}");
        assert!(evidence.contains("written by the planner"), "{evidence}");
        cleanup(s, &task);
    }
}

/// A judge that would drop the criterion either way: it must not reach a
/// shared check.
fn dropping_judge() -> String {
    js(
        r#"{"verdict": "drop", "verified": true, "command": "", "criterionText": "", "reason": "seems so"}"#,
    )
}

fn assert_asks_the_owner(task: &Value) {
    assert_eq!(task["status"], "waiting", "task JSON: {task}");
    assert_eq!(task["question"]["kind"], "impossible", "{task}");
    assert_eq!(task["criteria"].as_array().unwrap().len(), 2, "{task}");
    assert!(automatic_answers(task).is_empty(), "{task}");
}

#[test]
fn an_impossible_repo_check_criterion_still_asks_the_owner() {
    // two.txt is on the base and the verify command reads it: not a file the
    // planner made, so the check is the repo's.
    let s = setup(
        &[("JUDGE_REPLY", &dropping_judge())],
        true,
        true,
        &[("two.txt", "ok\n")],
    );
    let id = create_planned(&s, plan(PLANNER_CHECK, true), "write the files");
    let task = finished(&s, &id);
    assert_eq!(source_of(&task, PLANNER_CHECK), "repo", "{task}");
    assert_asks_the_owner(&task);
    cleanup(s, &task);
}

#[test]
fn an_impossible_owner_check_criterion_still_asks_the_owner() {
    let s = setup(&[("JUDGE_REPLY", &dropping_judge())], true, true, &[]);
    let id = create_planned(
        &s,
        plan(PLANNER_CHECK, false),
        &format!("write the files; check it with {PLANNER_CHECK}"),
    );
    let task = finished(&s, &id);
    assert_eq!(source_of(&task, PLANNER_CHECK), "owner", "{task}");
    assert_asks_the_owner(&task);
    cleanup(s, &task);
}
