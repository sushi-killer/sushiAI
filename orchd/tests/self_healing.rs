//! Black-box integration tests: orchd repairs a task's own brief without the
//! owner (infeasible checks, checks broken on the base, a criterion unmet
//! twice, missing evidence, review scope, owner answers against criteria).

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// One fake plays every role, told apart by the brief; each reply is a JSON
/// string literal taken from the environment.
///
/// - review run `r` (0-based) replies `$REVIEW_<r>`, else `$REVIEW_REST`; a
///   brief that contains `$PASS_IF_BRIEF_HAS` gets `$REVIEW_PASS` instead;
/// - the healing judges ("You judge ...") reply `$JUDGE_FEASIBILITY`,
///   `$JUDGE_RULING` or `$JUDGE_ANSWER` by their first words, and leave a
///   `judge.<n>` file each;
/// - the brief check finds nothing;
/// - implement run `n` (0-based) evals `$IMPL_FIRST` when n is 0, else
///   `$IMPL_REST`, then reports complete.
const FAKE: &str = r#"#!/bin/sh
B="$LOG_DIR/brief.$$"
cat > "$B"
emit() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
if head -c 9 "$B" | grep -q '## Review'; then
  r=$(ls "$LOG_DIR"/review.* 2>/dev/null | wc -l | tr -d ' ')
  touch "$LOG_DIR/review.$r"
  if [ -n "$PASS_IF_BRIEF_HAS" ] && grep -q "$PASS_IF_BRIEF_HAS" "$B"; then
    emit "$REVIEW_PASS"
    exit 0
  fi
  eval "reply=\${REVIEW_$r:-\$REVIEW_REST}"
  emit "$reply"
  exit 0
fi
if grep -q 'You judge' "$B"; then
  j=$(ls "$LOG_DIR"/judge.* 2>/dev/null | wc -l | tr -d ' ')
  touch "$LOG_DIR/judge.$j"
  if grep -q 'whether a check command' "$B"; then emit "$JUDGE_FEASIBILITY"
  elif grep -q 'keeps failing review' "$B"; then emit "$JUDGE_RULING"
  else emit "$JUDGE_ANSWER"; fi
  exit 0
fi
if grep -q 'You check a coding task brief' "$B"; then
  emit '"{\"contradiction\": false, \"conflict\": \"\"}"'
  exit 0
fi
n=$(ls "$LOG_DIR"/implement.* 2>/dev/null | wc -l | tr -d ' ')
touch "$LOG_DIR/implement.$n"
echo changed > CHANGED_MARKER.txt
if [ "$n" = 0 ]; then eval "$IMPL_FIRST"; else eval "$IMPL_REST"; fi
emit '"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
"#;

struct Setup {
    daemon: Daemon,
    log: tempfile::TempDir,
    repo: tempfile::TempDir,
    _scripts: tempfile::TempDir,
}

/// A JSON string literal, as the fake's `emit` wants its reply.
fn js(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

fn review_reply(json_body: &str) -> String {
    js(&format!("```sushi-review\n{json_body}\n```"))
}

/// A daemon whose cheap judge route and review route both run the fake.
fn setup(env: &[(&str, &str)], review: &str) -> Setup {
    let scripts = tempfile::tempdir().unwrap();
    let path = fake_harness_script(scripts.path(), "fake-claude.sh", FAKE);
    let log = tempfile::tempdir().unwrap();
    let mut vars = vec![
        ("ORCHD_CLAUDE_BIN", path.to_str().unwrap()),
        ("LOG_DIR", log.path().to_str().unwrap()),
        ("IMPL_FIRST", ":"),
        ("IMPL_REST", ":"),
    ];
    vars.extend_from_slice(env);
    let daemon = Daemon::spawn(&vars);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["briefCheckRoute"] = json!("claude-sonnet");
    settings["review"] = json!(review);
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
        "title": "Heal",
        "goal": "Make a change",
        "criteria": ["Something observable -- check: a test"],
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

fn count(dir: &Path, prefix: &str) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .count()
}

fn implements(task: &Value) -> Vec<&Value> {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect()
}

fn decisions(task: &Value) -> String {
    task["decisions"].to_string()
}

fn orchd_assumptions(task: &Value) -> Vec<Value> {
    task["assumptions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a["by"] == "orchd")
        .collect()
}

fn cleanup(s: Setup, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap().to_string();
    s.daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn a_check_that_needs_a_harness_the_repo_lacks_is_rewritten_and_recorded_as_an_assumption() {
    let feasibility = js(
        "{\"verdict\":\"rewrite\",\"command\":\"test -f CHANGED_MARKER.txt\",\"criterion\":0,\"criterionText\":\"Clicking opens the panel -- check: read the handler\",\"reason\":\"the repo has no DOM test runner\"}",
    );
    let s = setup(&[("JUDGE_FEASIBILITY", &feasibility)], "");
    let id = create(
        &s,
        json!({
            "criteria": ["Clicking opens the panel -- check: DOM click test"],
            "verify": ["no-such-dom-test-runner tests/click.dom"],
        }),
    );
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(
        task["verify"],
        json!(["test -f CHANGED_MARKER.txt"]),
        "{task}"
    );
    assert_eq!(
        task["criteria"],
        json!(["Clicking opens the panel -- check: read the handler"]),
        "{task}"
    );
    assert!(
        decisions(&task).contains("cannot run on this repository"),
        "{task}"
    );
    let assumed = orchd_assumptions(&task);
    assert_eq!(assumed.len(), 1, "{task}");
    assert!(assumed[0]["answer"]
        .as_str()
        .unwrap()
        .contains("test -f CHANGED_MARKER.txt"));
    assert_eq!(assumed[0]["overturned"], false);
    assert!(assumed[0]["evidence"]
        .as_str()
        .unwrap()
        .contains("no DOM test runner"));
    assert_eq!(implements(&task).len(), 1, "no attempt was spent: {task}");
    cleanup(s, &task);
}

#[test]
fn a_verify_command_failing_on_the_base_for_an_unrelated_reason_is_non_gating() {
    let feasibility =
        js("{\"verdict\":\"unrelated\",\"reason\":\"the lint config is broken on main\"}");
    let s = setup(&[("JUDGE_FEASIBILITY", &feasibility)], "");
    let id = create(
        &s,
        json!({"verify": ["echo broken lint; exit 3"], "finalVerify": ["echo broken lint; exit 3"]}),
    );
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(task["verify"], json!([]), "{task}");
    assert_eq!(task["finalVerify"], Value::Null, "{task}");
    assert_eq!(
        task["briefCheck"]["nonGating"],
        json!(["echo broken lint; exit 3"]),
        "{task}"
    );
    assert!(
        decisions(&task).contains("for a reason unrelated to the task")
            && decisions(&task).contains("non-gating"),
        "{task}"
    );
    assert_eq!(
        orchd_assumptions(&task).len(),
        1,
        "one command, judged once: {task}"
    );
    assert_eq!(implements(&task).len(), 1, "{task}");
    cleanup(s, &task);
}

#[test]
fn a_check_that_fails_on_the_base_as_it_should_stays_as_written() {
    let feasibility =
        js("{\"verdict\":\"feasible\",\"reason\":\"the work is meant to make it pass\"}");
    let s = setup(&[("JUDGE_FEASIBILITY", &feasibility)], "");
    let id = create(&s, json!({"verify": ["test -f CHANGED_MARKER.txt"]}));
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(
        task["verify"],
        json!(["test -f CHANGED_MARKER.txt"]),
        "{task}"
    );
    assert!(orchd_assumptions(&task).is_empty(), "{task}");
    cleanup(s, &task);
}

#[test]
fn the_same_unmet_criterion_twice_goes_to_the_judge_and_an_infeasible_one_is_amended() {
    let unmet = review_reply(
        "{\"verdict\":\"FAIL\",\"findings\":[],\"criteria\":[{\"criterion\":\"Clicking opens the panel\",\"met\":false,\"evidence\":\"there is no DOM click test\"}]}",
    );
    let pass = review_reply("{\"verdict\":\"PASS\",\"findings\":[]}");
    let ruling = js(
        "{\"verdict\":\"infeasible as written\",\"criterion\":\"Clicking opens the panel -- check: read the handler in src/handler.rs\",\"reason\":\"the repo has no DOM test harness\"}",
    );
    let s = setup(
        &[
            ("REVIEW_REST", &unmet),
            ("REVIEW_PASS", &pass),
            ("PASS_IF_BRIEF_HAS", "read the handler in src/handler.rs"),
            ("JUDGE_RULING", &ruling),
        ],
        "claude-sonnet",
    );
    let id = create(
        &s,
        json!({"criteria": ["Clicking opens the panel -- check: DOM click test"]}),
    );
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(implements(&task).len(), 2, "{task}");
    assert_eq!(
        task["criteria"],
        json!(["Clicking opens the panel -- check: read the handler in src/handler.rs"]),
        "{task}"
    );
    assert_eq!(count(s.log.path(), "judge."), 1, "one ruling only");
    assert!(decisions(&task).contains("infeasible as written"), "{task}");
    let assumed = orchd_assumptions(&task);
    assert_eq!(assumed.len(), 1, "{task}");
    assert!(assumed[0]["question"]
        .as_str()
        .unwrap()
        .contains("Clicking opens the panel"));
    // The amended criterion was reviewed in the same attempt.
    assert_eq!(count(s.log.path(), "review."), 3, "{task}");
    cleanup(s, &task);
}

#[test]
fn a_code_gap_ruling_leaves_the_criterion_and_the_retries_go_on() {
    let unmet = review_reply(
        "{\"verdict\":\"FAIL\",\"findings\":[],\"criteria\":[{\"criterion\":\"Clicking opens the panel\",\"met\":false,\"evidence\":\"the handler is missing\"}]}",
    );
    let ruling = js("{\"verdict\":\"code gap\",\"reason\":\"the handler is not written\"}");
    let s = setup(
        &[("REVIEW_REST", &unmet), ("JUDGE_RULING", &ruling)],
        "claude-sonnet",
    );
    let id = create(
        &s,
        json!({"criteria": ["Clicking opens the panel -- check: read the handler"]}),
    );
    let task = poll_until(&s.daemon, &id, Duration::from_secs(90), |st| {
        matches!(st, "done" | "failed" | "stopped" | "waiting")
    });
    assert_ne!(task["status"], "done", "{task}");
    assert_eq!(
        task["criteria"],
        json!(["Clicking opens the panel -- check: read the handler"]),
        "{task}"
    );
    assert_eq!(
        count(s.log.path(), "judge."),
        1,
        "judged once per criterion"
    );
    assert!(decisions(&task).contains("judged a code gap"), "{task}");
    assert!(implements(&task).len() >= 3, "{task}");
    cleanup(s, &task);
}

#[test]
fn a_named_screenshot_command_is_run_by_orchd_and_never_fails_an_attempt_for_evidence() {
    let s = setup(&[], "");
    let id = create(
        &s,
        json!({
            "criteria": ["The panel is open -- check: screenshot under artifacts/"],
            "screenshot": "mkdir -p artifacts && echo png > artifacts/panel.png",
        }),
    );
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    let attempts = implements(&task);
    assert_eq!(attempts.len(), 1, "{task}");
    assert!(attempts[0]["failure"].is_null(), "{task}");
    let evidence = attempts[0]["evidence"].as_array().unwrap();
    assert_eq!(evidence.len(), 1, "{task}");
    assert!(evidence[0].as_str().unwrap().ends_with("panel.png"));
    assert!(Path::new(evidence[0].as_str().unwrap()).exists());
    cleanup(s, &task);
}

#[test]
fn a_screenshot_command_that_fails_is_a_decision_not_a_failed_attempt() {
    let s = setup(&[], "");
    let id = create(
        &s,
        json!({
            "criteria": ["The panel is open -- check: screenshot under artifacts/"],
            "screenshot": "mkdir -p artifacts && echo png > artifacts/panel.png; exit 4",
        }),
    );
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert!(decisions(&task).contains("exit 4 exited 4"), "{task}");
    cleanup(s, &task);
}

/// Attempt 1 saves a screenshot and a UI file, then fails verify (`SECOND`
/// is missing); attempt 2 is `$IMPL_REST`.
fn earlier_images(impl_rest: &str, review: &str) -> (Setup, String) {
    let pass = review_reply("{\"verdict\":\"PASS\",\"findings\":[]}");
    let s = setup(
        &[
            (
                "IMPL_FIRST",
                "mkdir -p artifacts && echo png > artifacts/shot.png && echo a > ui.css",
            ),
            ("IMPL_REST", impl_rest),
            ("REVIEW_REST", &pass),
        ],
        review,
    );
    let id = create(
        &s,
        json!({
            "criteria": ["The panel is open -- check: screenshot under artifacts/"],
            "verify": ["test -f SECOND"],
            "screenshot": "true",
        }),
    );
    (s, id)
}

#[test]
fn a_screenshot_command_that_leaves_the_named_images_missing_still_fails_the_attempt() {
    let s = setup(&[], "");
    let id = create(
        &s,
        json!({
            "criteria": ["The panel is open -- check: screenshot artifacts/panel.png"],
            "screenshot": "mkdir -p artifacts && echo png > artifacts/other.png",
        }),
    );
    let task = finished(&s, &id);
    assert_eq!(task["status"], "waiting", "{task}");
    let first = implements(&task)[0];
    assert_eq!(first["failure"]["kind"], "evidence", "{task}");
    let detail = first["failure"]["detail"].as_str().unwrap();
    assert!(detail.contains("artifacts/panel.png"), "{detail}");
    assert!(detail.contains("screenshot command"), "{detail}");
    cleanup(s, &task);
}

#[test]
fn a_visual_criterion_without_any_screenshot_command_does_not_fail_the_attempt() {
    let s = setup(&[], "");
    let id = create(
        &s,
        json!({"criteria": ["The panel is open -- check: screenshot under artifacts/"]}),
    );
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    let attempts = implements(&task);
    assert_eq!(attempts.len(), 1, "{task}");
    assert!(attempts[0]["failure"].is_null(), "{task}");
    assert!(
        decisions(&task).contains("no screenshot command is set"),
        "{task}"
    );
    let ups = task["briefCheck"]["followUps"].as_array().unwrap();
    assert!(
        ups.iter()
            .any(|u| u.as_str().unwrap().contains("The panel is open")),
        "{task}"
    );
    cleanup(s, &task);
}

#[test]
fn images_of_an_earlier_attempt_count_when_no_ui_file_it_changed_has_changed() {
    let (s, id) = earlier_images("echo x > SECOND", "claude-sonnet");
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    let attempts = implements(&task);
    assert_eq!(attempts.len(), 2, "{task}");
    assert_eq!(attempts[0]["failure"]["kind"], "verify", "{task}");
    assert!(attempts[1]["failure"].is_null(), "{task}");
    assert_eq!(attempts[1]["evidenceFrom"], 1, "{task}");
    assert!(decisions(&task).contains("reused attempt 1's evidence"));
    // The reviewer is shown the image with the attempt that produced it.
    let labelled = std::fs::read_dir(s.log.path())
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .any(|b| b.starts_with("## Review") && b.contains("shot.png` (attempt 1)"));
    assert!(labelled, "the review brief labels the image with attempt 1");
    cleanup(s, &task);
}

#[test]
fn images_of_an_earlier_attempt_do_not_count_once_a_ui_file_it_changed_changed_again() {
    let (s, id) = earlier_images("echo x > SECOND; echo b > ui.css", "");
    let task = finished(&s, &id);
    assert_eq!(task["status"], "waiting", "{task}");
    let attempts = implements(&task);
    assert!(attempts.len() >= 2, "{task}");
    assert_eq!(attempts[1]["failure"]["kind"], "evidence", "{task}");
    assert!(attempts[1]["evidenceFrom"].is_null(), "{task}");
    cleanup(s, &task);
}

#[test]
fn after_the_first_attempt_p2_and_unrelated_findings_do_not_cost_an_attempt_and_go_to_the_report() {
    let first = review_reply(
        "{\"verdict\":\"FAIL\",\"findings\":[\"P1: CHANGED_MARKER.txt:1 - wrong content\"]}",
    );
    let second = review_reply(
        "{\"verdict\":\"FAIL\",\"findings\":[\"P2: CHANGED_MARKER.txt:1 - naming\",\"P1: legacy/other.rs:9 - an old bug\"]}",
    );
    let s = setup(
        &[("REVIEW_0", &first), ("REVIEW_REST", &second)],
        "claude-sonnet",
    );
    let id = create(&s, json!({}));
    let task = finished(&s, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(implements(&task).len(), 2, "{task}");
    assert!(decisions(&task).contains("recorded as PASS"), "{task}");
    let ups = task["briefCheck"]["followUps"].as_array().unwrap();
    assert_eq!(ups.len(), 2, "{task}");
    let report = task["report"].as_str().unwrap();
    let follow_ups = report.split("## Follow-ups").nth(1).unwrap();
    assert!(follow_ups.contains("naming"), "{report}");
    assert!(follow_ups.contains("legacy/other.rs:9"), "{report}");
    cleanup(s, &task);
}

#[test]
fn after_the_first_attempt_a_p1_about_the_tasks_own_work_still_blocks() {
    let blocking = review_reply(
        "{\"verdict\":\"FAIL\",\"findings\":[\"P1: CHANGED_MARKER.txt:1 - wrong content\",\"P2: nit\"]}",
    );
    let s = setup(&[("REVIEW_REST", &blocking)], "claude-sonnet");
    let id = create(&s, json!({}));
    let task = poll_until(&s.daemon, &id, Duration::from_secs(90), |st| {
        matches!(st, "done" | "failed" | "stopped" | "waiting")
    });
    assert_ne!(task["status"], "done", "{task}");
    assert!(implements(&task).len() >= 3, "{task}");
    // The P2 is a follow-up, not something the next attempt is told to fix.
    let second = implements(&task)[1];
    assert_eq!(
        second["review"]["findings"].as_array().unwrap().len(),
        1,
        "{task}"
    );
    cleanup(s, &task);
}

#[test]
fn an_owner_answer_that_contradicts_a_criterion_amends_it_before_the_next_review() {
    let no_verdict = js("Looks fine to me.");
    let unmet = review_reply(
        "{\"verdict\":\"FAIL\",\"findings\":[],\"criteria\":[{\"criterion\":\"The button is blue\",\"met\":false,\"evidence\":\"it is not blue\"}]}",
    );
    let pass = review_reply("{\"verdict\":\"PASS\",\"findings\":[]}");
    let answer_judge = js(
        "{\"amend\":[{\"criterion\":0,\"text\":\"The button is green -- check: read the CSS\"}],\"reason\":\"the owner chose green\"}",
    );
    let s = setup(
        &[
            ("REVIEW_0", &no_verdict),
            ("REVIEW_1", &no_verdict),
            ("REVIEW_REST", &unmet),
            ("REVIEW_PASS", &pass),
            ("PASS_IF_BRIEF_HAS", "The button is green"),
            ("JUDGE_ANSWER", &answer_judge),
        ],
        "claude-sonnet",
    );
    let id = create(
        &s,
        json!({"criteria": ["The button is blue -- check: read the CSS"]}),
    );
    let waiting = poll_until(&s.daemon, &id, Duration::from_secs(60), |st| {
        st == "waiting"
    });
    assert_eq!(
        waiting["question"]["kind"], "review_no_verdict",
        "{waiting}"
    );
    s.daemon.request(
        "task.answer",
        json!({"id": id, "answer": "make the button green, not blue"}),
    );
    let task = poll_until(&s.daemon, &id, Duration::from_secs(60), |st| st == "done");
    assert_eq!(
        task["criteria"],
        json!(["The button is green -- check: read the CSS"]),
        "{task}"
    );
    assert!(
        decisions(&task).contains("contradicts the owner's answer"),
        "{task}"
    );
    let assumed = orchd_assumptions(&task);
    assert_eq!(assumed.len(), 1, "{task}");
    assert!(assumed[0]["answer"].as_str().unwrap().contains("green"));
    assert_eq!(count(s.log.path(), "judge."), 1, "{task}");
    // The review that passed saw the amended criterion.
    let saw_green = std::fs::read_dir(s.log.path())
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .any(|b| b.starts_with("## Review") && b.contains("- The button is green"));
    assert!(saw_green);
    cleanup(s, &task);
}
