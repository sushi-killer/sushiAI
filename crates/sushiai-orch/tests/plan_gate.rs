//! Black-box integration tests (plan gate): a fake harness plays the planner,
//! the plan reviewer and the implementer, and logs every kind of call.

mod common;

use common::*;

/// Answers by what it is asked. A planner brief that carries the gate's
/// findings gets `$SECOND_DRAFT`, any other `$FIRST_DRAFT`; the review prompt
/// gets `$REVIEW_RESULT`. Every call is appended to `$CALLS_LOG`; the
/// planner's briefs are kept as `$CALLS_LOG.plan<N>`.
const FAKE_GATED_SCRIPT: &str = r###"#!/bin/sh
input="$(cat)"
emit() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"'"$1"'"}'
}
case "$input" in
  *"You review a coding task plan"*)
    echo review >> "$CALLS_LOG"
    emit "$REVIEW_RESULT"
    ;;
  *sushi-plan*)
    case "$input" in
      *"## Plan gate findings"*)
        echo replan >> "$CALLS_LOG"
        printf '%s' "$input" > "$CALLS_LOG.replan"
        emit '```sushi-plan\n'"$SECOND_DRAFT"'\n```'
        ;;
      *)
        echo plan >> "$CALLS_LOG"
        emit '```sushi-plan\n'"$FIRST_DRAFT"'\n```'
        ;;
    esac
    ;;
  *)
    echo implement >> "$CALLS_LOG"
    printf '%s' "$input" > "$CALLS_LOG.implement"
    echo "changed" > CHANGED_MARKER.txt
    emit '```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```'
    ;;
esac
"###;

const NO_FINDINGS: &str = r#"{\"findings\": []}"#;

const CLEAN_DRAFT: &str = r#"{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"tier\":\"hard\",\"criteria\":[\"Toggle visible in settings -- check: sh -c true\"],\"verify\":[\"true\"],\"questions\":[]}"#;

const HANDOFF_DRAFT: &str = r#"{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"tier\":\"hard\",\"criteria\":[\"Toggle visible in settings -- check: sh -c true\",\"The implementer's handoff lists the changed files -- check: pytest tests/test_handoff.py::test_lists\"],\"verify\":[\"true\"],\"questions\":[]}"#;

const REPORT_LISTS_DRAFT: &str = r#"{\"title\":\"Report failures\",\"goal\":\"List the failing tests\",\"tier\":\"hard\",\"criteria\":[\"The report lists every failing test by name -- check: pytest tests/test_report.py::test_lists\"],\"verify\":[\"true\"],\"questions\":[]}"#;

const JVM_DOTNET_RUBY_DRAFT: &str = r#"{\"title\":\"Build the parts\",\"goal\":\"Build the parts\",\"tier\":\"hard\",\"criteria\":[\"Build passes -- check: mvn test\",\"Serialises -- check: dotnet test\",\"Renders -- check: bundle exec rspec spec/render_spec.rb\"],\"verify\":[\"true\"],\"questions\":[]}"#;

struct Run {
    task: serde_json::Value,
    calls: Vec<String>,
    log: std::path::PathBuf,
    daemon: Daemon,
    _scripts: tempfile::TempDir,
    _repo: tempfile::TempDir,
}

fn run(first: &str, second: &str, review_route: &str, review_result: &str) -> Run {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-gated.sh", FAKE_GATED_SCRIPT);
    let log = scripts.path().join("calls.log");
    std::fs::write(&log, "").unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("CALLS_LOG", log.to_str().unwrap()),
        ("FIRST_DRAFT", first),
        ("SECOND_DRAFT", second),
        ("REVIEW_RESULT", review_result),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["tiers"]["hard"] = serde_json::json!("claude-sonnet");
    settings["planner"] = serde_json::json!("claude-sonnet");
    settings["planReview"] = serde_json::json!(review_route);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let created = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": true,
        }),
    );
    let id = created["id"].as_str().unwrap().to_string();
    let task = poll_until(&daemon, &id, |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    let calls = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    Run {
        task,
        calls,
        log,
        daemon,
        _scripts: scripts,
        _repo: repo,
    }
}

fn count(calls: &[String], kind: &str) -> usize {
    calls.iter().filter(|c| *c == kind).count()
}

fn decisions(task: &serde_json::Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

fn criteria(task: &serde_json::Value) -> Vec<String> {
    task["criteria"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_string())
        .collect()
}

fn findings(task: &serde_json::Value) -> Vec<serde_json::Value> {
    task["planGate"]["findings"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn a_typed_report_only_finding_removes_its_criterion_without_a_replan() {
    // The finding names criterion 1; criterion 0 must stay.
    let review = r#"{\"findings\": [{\"kind\": \"report-only\", \"criterion\": 1, \"severity\": \"high\", \"text\": \"Only the agent's own handoff proves it\"}]}"#;
    let r = run(HANDOFF_DRAFT, HANDOFF_DRAFT, "claude-sonnet", review);
    assert_eq!(count(&r.calls, "review"), 1, "calls: {:?}", r.calls);
    assert_eq!(count(&r.calls, "replan"), 0, "calls: {:?}", r.calls);
    let stored = criteria(&r.task);
    assert_eq!(stored.len(), 1, "criteria: {stored:?}");
    assert!(
        stored[0].starts_with("Toggle visible"),
        "criteria: {stored:?}"
    );
    let lines = decisions(&r.task);
    assert!(
        lines.iter().any(|l| {
            l.starts_with("Plan gate: removed the report-only criterion") && l.contains("handoff")
        }),
        "{lines:?}"
    );
    assert_eq!(
        r.task["request"], "add dark mode to the settings screen",
        "{}",
        r.task
    );
    r.daemon.shutdown_and_wait();
}

#[test]
fn a_report_named_criterion_with_a_real_check_is_kept_without_a_report_only_finding() {
    let r = run(
        REPORT_LISTS_DRAFT,
        REPORT_LISTS_DRAFT,
        "claude-sonnet",
        NO_FINDINGS,
    );
    assert_eq!(count(&r.calls, "review"), 1, "calls: {:?}", r.calls);
    assert_eq!(count(&r.calls, "replan"), 0, "calls: {:?}", r.calls);
    let stored = criteria(&r.task);
    assert!(
        stored.iter().any(|c| c.starts_with("The report lists")),
        "criteria: {stored:?}"
    );
    assert!(
        !decisions(&r.task)
            .iter()
            .any(|l| l.contains("removed the report-only criterion")),
        "{}",
        r.task
    );
    r.daemon.shutdown_and_wait();
}

#[test]
fn commands_of_mvn_dotnet_and_rspec_are_not_flagged_by_code() {
    let r = run(
        JVM_DOTNET_RUBY_DRAFT,
        JVM_DOTNET_RUBY_DRAFT,
        "claude-sonnet",
        NO_FINDINGS,
    );
    assert_eq!(count(&r.calls, "replan"), 0, "calls: {:?}", r.calls);
    assert!(findings(&r.task).is_empty(), "{}", r.task);
    r.daemon.shutdown_and_wait();
}

#[test]
fn an_unusable_review_reply_takes_the_invalid_review_path() {
    // `Wrong split` is not one of the five kinds: the reply does not deserialize.
    let review = r#"{\"findings\": [{\"kind\": \"Wrong split\", \"severity\": \"critical\", \"text\": \"Two parts edit one file\"}]}"#;
    let r = run(CLEAN_DRAFT, CLEAN_DRAFT, "claude-sonnet", review);
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(count(&r.calls, "replan"), 0, "calls: {:?}", r.calls);
    assert!(findings(&r.task).is_empty(), "{}", r.task);
    let lines = decisions(&r.task);
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("Plan gate:") && l.contains("gave no usable answer")),
        "{lines:?}"
    );
    r.daemon.shutdown_and_wait();
}

#[test]
fn a_critical_plan_review_finding_triggers_one_replan_and_no_second_review() {
    let review = r#"{\"findings\": [{\"severity\": \"critical\", \"kind\": \"missing-requirement\", \"requestItem\": \"persist the choice\", \"text\": \"Nothing persists the choice\"}]}"#;
    let r = run(CLEAN_DRAFT, CLEAN_DRAFT, "claude-sonnet", review);
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(count(&r.calls, "review"), 1, "calls: {:?}", r.calls);
    assert_eq!(count(&r.calls, "replan"), 1, "calls: {:?}", r.calls);
    assert_eq!(count(&r.calls, "plan"), 1, "calls: {:?}", r.calls);
    let brief = std::fs::read_to_string(format!("{}.replan", r.log.display())).unwrap();
    assert!(brief.contains("Nothing persists the choice"), "{brief}");
    assert_eq!(r.task["planGate"]["replanned"], true, "{}", r.task);
    assert_eq!(r.task["planGate"]["reviewed"], true, "{}", r.task);
    r.daemon.shutdown_and_wait();
}

#[test]
fn a_medium_finding_reaches_the_implementer_brief() {
    let review = r#"{\"findings\": [{\"severity\": \"medium\", \"kind\": \"wrong-approach\", \"text\": \"Prefer the existing theme hook\"}]}"#;
    let r = run(CLEAN_DRAFT, CLEAN_DRAFT, "claude-sonnet", review);
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(count(&r.calls, "replan"), 0, "calls: {:?}", r.calls);
    assert!(
        findings(&r.task)
            .iter()
            .any(|f| f["text"] == "Prefer the existing theme hook" && f["severity"] == "MEDIUM"),
        "{}",
        r.task
    );
    let brief = std::fs::read_to_string(format!("{}.implement", r.log.display())).unwrap();
    assert!(brief.contains("## Plan notes"), "{brief}");
    assert!(brief.contains("Prefer the existing theme hook"), "{brief}");
    r.daemon.shutdown_and_wait();
}

#[test]
fn an_empty_plan_review_setting_runs_the_review_on_the_planner_route() {
    let r = run(CLEAN_DRAFT, CLEAN_DRAFT, "", NO_FINDINGS);
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(count(&r.calls, "review"), 1, "calls: {:?}", r.calls);
    let lines = decisions(&r.task);
    assert!(
        lines.iter().any(|l| {
            l.starts_with("Plan gate: plan review by claude-sonnet")
                && l.contains("the planner's route")
        }),
        "{lines:?}"
    );
    r.daemon.shutdown_and_wait();
}

#[test]
fn a_review_route_that_does_not_exist_falls_back_to_the_planner_route_and_says_so() {
    let r = run(CLEAN_DRAFT, CLEAN_DRAFT, "no-such-route", NO_FINDINGS);
    assert_eq!(r.task["status"], "done", "task JSON: {}", r.task);
    assert_eq!(count(&r.calls, "review"), 1, "calls: {:?}", r.calls);
    let lines = decisions(&r.task);
    assert!(
        lines.iter().any(|l| {
            l.starts_with("Plan gate:")
                && l.contains("no-such-route")
                && l.contains("the planner's route")
        }),
        "{lines:?}"
    );
    r.daemon.shutdown_and_wait();
}
