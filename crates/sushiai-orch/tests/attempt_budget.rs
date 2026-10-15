//! Wall-clock budget failures retain the native session and resume through
//! the existing implement retry, without a report-only harness call.

mod common;

use common::attempts::run_file;
use common::harness::fake_harness_script;
use common::polling::{remove_dir_retrying, settle, wait_until};
use common::process::Daemon;
use common::repository::init_git_repo;

use serde_json::{json, Value};

const FAKE_SLOW: &str = r###"#!/bin/sh
printf '%s\n' "$*" >> "$LOG_DIR/args.log"
n=$(wc -l < "$LOG_DIR/args.log" | tr -d ' ')
cat > "$LOG_DIR/brief-$n.md"
if [ -f BUDGET_MARKER.txt ]; then
  session=sess-fresh
  cost=0.02
  case " $* " in
    *" --resume "*)
      if [ "$REJECT_RESUME" = 1 ]; then echo 'session not found' >&2; exit 1; fi
      session=sess-slow
      cost=0.03 ;;
  esac
  echo done > FINISHED_MARKER.txt
  printf '%s\n' "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"$session\"}"
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":$cost,\"usage\":{\"input_tokens\":1000,\"cache_read_input_tokens\":500,\"output_tokens\":2000},\"result\":\"\`\`\`sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n\`\`\`\"}"
  exit 0
fi
echo changed > BUDGET_MARKER.txt
if [ "$NO_SESSION" != 1 ]; then
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-slow"}'
fi
printf '%s\n' '{"type":"assistant","message":{"id":"msg-budget","model":"test-budget-model","content":[{"type":"text","text":"Working."}],"usage":{"input_tokens":1000,"cache_read_input_tokens":250,"output_tokens":0}}}'
if [ "$REPORT_FIRST" = 1 ]; then
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1000,"cache_read_input_tokens":250,"output_tokens":0},"result":"```sushi-report\n{\"outcome\":\"partial\",\"summary\":\"half done\",\"handoff\":\"the parser is rewritten, the writer is not\",\"decisions\":[],\"question\":\"\"}\n```"}'
fi
sleep 60
"###;

const FAKE_PASS: &str = r###"#!/bin/sh
cat > /dev/null
echo changed > CHANGED_MARKER.txt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"###;

struct Setup {
    daemon: Daemon,
    repo: tempfile::TempDir,
    scratch: tempfile::TempDir,
}

fn setup(script_body: &str, budget_mins: Option<f64>, extra: &[(&str, &str)]) -> Setup {
    let scratch = tempfile::tempdir().unwrap();
    let repo = init_git_repo();
    let script = fake_harness_script(scratch.path(), "fake.sh", script_body);
    let mut env = vec![
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("LOG_DIR", scratch.path().to_str().unwrap()),
    ];
    env.extend_from_slice(extra);
    let daemon = Daemon::spawn(&env);
    let mut settings = daemon.request("settings.get", json!({}));
    assert_eq!(settings["implementBudgetMins"], 45.0, "{settings}");
    settings["review"] = json!("");
    settings["sandbox"] = json!("host");
    settings["maxAttempts"] = json!(2);
    settings["tiers"] = json!({
        "mechanical": "claude-sonnet", "standard": "claude-sonnet", "hard": "claude-sonnet",
    });
    settings["prices"]["test-budget-model"] =
        json!({"input": 10.0, "cachedInput": 0.0, "output": 0.0});
    if let Some(mins) = budget_mins {
        settings["implementBudgetMins"] = json!(mins);
    }
    daemon.request("settings.set", json!({"settings": settings}));
    Setup {
        daemon,
        repo,
        scratch,
    }
}

fn start(s: &Setup, verify: &str, variant: Value) -> String {
    let task = s.daemon.request(
        "task.create",
        json!({
            "repo": s.repo.path().to_str().unwrap(),
            "title": "Budget", "goal": "Write the marker",
            "criteria": ["The marker is finished"],
            "verify": [verify], "variant": variant,
            "land": false, "start": true,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn cleanup(s: Setup, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap().to_string();
    s.daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

fn first_attempt(task: &Value) -> &Value {
    &task["attempts"].as_array().unwrap()[0]
}

fn argv(s: &Setup) -> Vec<String> {
    std::fs::read_to_string(s.scratch.path().join("args.log"))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn delivered_brief(s: &Setup, n: u32) -> String {
    std::fs::read_to_string(s.scratch.path().join(format!("brief-{n}.md"))).unwrap()
}

fn assert_run_cost(
    s: &Setup,
    task: &Value,
    runs: u64,
    total: f64,
    (input, cached, output): (u64, u64, u64),
) {
    let cost = s
        .daemon
        .request("costs.summary", json!({"groupBy": ["stage"]}));
    assert_eq!(cost["totals"]["runs"], runs, "{cost}");
    assert_eq!(
        cost["totals"]["tokens"],
        json!({"input": input, "cached": cached, "output": output}),
        "{cost}"
    );
    assert!(
        (cost["totals"]["costUsd"].as_f64().unwrap() - total).abs() < 1e-9,
        "{cost}"
    );
    assert!(
        (task["costUsd"].as_f64().unwrap() - total).abs() < 1e-9,
        "{task}"
    );
    for attempt in task["attempts"].as_array().unwrap() {
        let report = s
            .daemon
            .data_dir()
            .join("tasks")
            .join(task["id"].as_str().unwrap())
            .join("runs")
            .join(attempt["n"].as_u64().unwrap().to_string())
            .join("report.events.jsonl");
        assert!(!report.exists(), "{}", report.display());
    }
}

#[test]
fn the_brief_asks_for_targeted_tests_and_the_budget_defaults_to_45_minutes() {
    let s = setup(FAKE_PASS, None, &[]);
    let id = start(&s, "test -f CHANGED_MARKER.txt", json!({}));
    let task = settle(&s.daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    let brief = run_file(&s.daemon, &id, "brief.md");
    assert!(
        !brief.contains("Run the verification commands yourself"),
        "{brief}"
    );
    assert!(
        brief.contains("Run only the targeted tests for what you changed"),
        "{brief}"
    );
    cleanup(s, &task);
}

#[test]
fn a_timeout_without_a_result_keeps_its_early_session_usage_and_changed_files() {
    let s = setup(FAKE_SLOW, Some(0.05), &[]);
    let id = start(
        &s,
        "test -f FINISHED_MARKER.txt",
        json!({"maxCostUsd": 0.005}),
    );
    let task = settle(&s.daemon, &id);
    assert_eq!(task["status"], "waiting", "{task}");
    let attempt = first_attempt(&task);
    assert_eq!(attempt["failure"]["kind"], "time_budget", "{task}");
    assert_eq!(attempt["sessionId"], "sess-slow", "{task}");
    assert_eq!(
        attempt["usage"],
        json!({"input": 1000, "cached": 250, "output": 0}),
        "{task}"
    );
    assert_eq!(attempt["costEstimated"], true, "{task}");
    let handoff = attempt["handoff"].as_str().unwrap();
    assert!(handoff.contains("0.05 minute time budget"), "{task}");
    assert!(handoff.contains("BUDGET_MARKER.txt"), "{task}");
    assert!(attempt["summary"].is_null(), "{task}");
    let events = run_file(&s.daemon, &id, "events.jsonl");
    assert!(events.contains("\"session_id\":\"sess-slow\""), "{events}");
    assert!(!events.contains("\"type\":\"result\""), "{events}");
    assert!(
        attempt["changedFiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "BUDGET_MARKER.txt"),
        "{task}"
    );
    assert_eq!(argv(&s).len(), 1);
    assert_run_cost(&s, &task, 1, 0.01, (1000, 250, 0));
    cleanup(s, &task);
}

#[test]
fn a_timeout_keeps_an_already_available_report_without_an_extra_turn() {
    let s = setup(FAKE_SLOW, Some(0.05), &[("REPORT_FIRST", "1")]);
    let id = start(
        &s,
        "test -f FINISHED_MARKER.txt",
        json!({"maxCostUsd": 0.005}),
    );
    let task = settle(&s.daemon, &id);
    let attempt = first_attempt(&task);
    assert_eq!(attempt["failure"]["kind"], "time_budget", "{task}");
    assert_eq!(attempt["summary"], "half done", "{task}");
    assert_eq!(
        attempt["handoff"], "the parser is rewritten, the writer is not",
        "{task}"
    );
    assert_eq!(argv(&s).len(), 1);
    assert_run_cost(&s, &task, 1, 0.01, (1000, 250, 0));
    cleanup(s, &task);
}

#[test]
fn a_time_budget_retry_resumes_the_native_session_with_the_current_contract_and_own_spend() {
    let s = setup(FAKE_SLOW, Some(0.05), &[]);
    let id = start(&s, "test -f FINISHED_MARKER.txt", json!({}));
    let task = settle(&s.daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    let attempts = task["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{task}");
    assert_eq!(attempts[0]["failure"]["kind"], "time_budget", "{task}");
    assert_eq!(attempts[1]["sessionId"], "sess-slow", "{task}");
    assert_eq!(
        attempts[0]["usage"],
        json!({"input": 1000, "cached": 250, "output": 0}),
        "{task}"
    );
    assert_eq!(
        attempts[1]["usage"],
        json!({"input": 1000, "cached": 500, "output": 2000}),
        "{task}"
    );
    assert!(
        (attempts[1]["costUsd"].as_f64().unwrap() - 0.02).abs() < 1e-9,
        "{task}"
    );
    assert_eq!(attempts[0]["routeId"], attempts[1]["routeId"], "{task}");
    assert_eq!(attempts[0]["model"], attempts[1]["model"], "{task}");
    let args = argv(&s);
    assert_eq!(args.len(), 2, "{args:?}");
    assert!(!args[0].contains("--resume"), "{args:?}");
    assert!(args[1].contains("--resume sess-slow"), "{args:?}");
    let brief = delivered_brief(&s, 2);
    assert!(brief.contains("time budget"), "{brief}");
    assert!(brief.contains("The marker is finished"), "{brief}");
    assert!(brief.contains("test -f FINISHED_MARKER.txt"), "{brief}");
    assert!(!brief.contains("fix the failed checks"), "{brief}");
    assert_run_cost(&s, &task, 2, 0.03, (2000, 750, 2000));
    cleanup(s, &task);
}

#[test]
fn a_time_budget_without_a_session_starts_the_next_attempt_fresh() {
    let s = setup(FAKE_SLOW, Some(0.05), &[("NO_SESSION", "1")]);
    let id = start(&s, "test -f FINISHED_MARKER.txt", json!({}));
    let task = settle(&s.daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    let attempts = task["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{task}");
    assert!(attempts[0]["sessionId"].is_null(), "{task}");
    let args = argv(&s);
    assert_eq!(args.len(), 2, "{args:?}");
    assert!(args.iter().all(|a| !a.contains("--resume")), "{args:?}");
    assert!(delivered_brief(&s, 2).contains("## Task"));
    assert_run_cost(&s, &task, 2, 0.03, (2000, 750, 2000));
    cleanup(s, &task);
}

#[test]
fn a_route_change_after_timeout_starts_the_next_attempt_fresh() {
    let s = setup(FAKE_SLOW, Some(0.05), &[]);
    let id = start(&s, "test -f FINISHED_MARKER.txt", json!({}));
    wait_until("the first session init", || {
        let task = s.daemon.request("task.get", json!({"id": id}));
        (task["attempts"][0]["sessionId"] == "sess-slow").then_some(())
    });
    let mut settings = s.daemon.request("settings.get", json!({}));
    settings["tiers"] = json!({
        "mechanical": "claude-opus", "standard": "claude-opus", "hard": "claude-opus",
    });
    s.daemon
        .request("settings.set", json!({"settings": settings}));
    let task = settle(&s.daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    let attempts = task["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{task}");
    assert_eq!(attempts[0]["failure"]["kind"], "time_budget", "{task}");
    assert_eq!(attempts[0]["routeId"], "claude-sonnet", "{task}");
    assert_eq!(attempts[1]["routeId"], "claude-opus", "{task}");
    let args = argv(&s);
    assert_eq!(args.len(), 2, "{args:?}");
    assert!(args.iter().all(|a| !a.contains("--resume")), "{args:?}");
    assert!(delivered_brief(&s, 2).contains("## Task"));
    assert_run_cost(&s, &task, 2, 0.03, (2000, 750, 2000));
    cleanup(s, &task);
}

#[test]
fn an_unavailable_time_budget_session_uses_the_existing_fresh_run_fallback() {
    let s = setup(FAKE_SLOW, Some(0.05), &[("REJECT_RESUME", "1")]);
    let id = start(&s, "test -f FINISHED_MARKER.txt", json!({}));
    let task = settle(&s.daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(task["attempts"].as_array().unwrap().len(), 2, "{task}");
    let args = argv(&s);
    assert_eq!(args.len(), 3, "{args:?}");
    assert!(args[1].contains("--resume sess-slow"), "{args:?}");
    assert!(!args[2].contains("--resume"), "{args:?}");
    assert!(delivered_brief(&s, 2).contains("time budget"));
    assert!(delivered_brief(&s, 3).contains("## Task"));
    assert_run_cost(&s, &task, 3, 0.03, (2000, 750, 2000));
    cleanup(s, &task);
}

#[test]
fn cancellation_does_not_start_a_budget_report_or_retry() {
    let s = setup(FAKE_SLOW, Some(0.5), &[]);
    let id = start(&s, "test -f FINISHED_MARKER.txt", json!({}));
    wait_until("the running session and usage", || {
        let task = s.daemon.request("task.get", json!({"id": id}));
        let events = s
            .daemon
            .data_dir()
            .join("tasks")
            .join(&id)
            .join("runs/1/events.jsonl");
        (task["attempts"][0]["sessionId"] == "sess-slow"
            && std::fs::read_to_string(events)
                .unwrap_or_default()
                .contains("msg-budget"))
        .then_some(())
    });
    s.daemon.request("task.stop", json!({"id": id}));
    let task = wait_until("the cancelled attempt to settle", || {
        let task = s.daemon.request("task.get", json!({"id": id}));
        (task["status"] == "stopped" && task["attempts"][0]["status"] == "interrupted")
            .then_some(task)
    });
    assert!(first_attempt(&task)["failure"].is_null(), "{task}");
    assert_eq!(task["attempts"].as_array().unwrap().len(), 1, "{task}");
    assert_eq!(argv(&s).len(), 1);
    assert_run_cost(&s, &task, 1, 0.01, (1000, 250, 0));
    cleanup(s, &task);
}

#[test]
fn an_owner_amendment_replaces_the_contract_delivered_to_the_same_timed_out_session() {
    let s = setup(FAKE_SLOW, Some(0.05), &[]);
    let old_command = "test -f FINISHED_MARKER.txt && test -f OBSOLETE_MARKER.txt";
    let current_command = "test -f FINISHED_MARKER.txt";
    let created = s.daemon.request("task.create", json!({
        "repo": s.repo.path().to_str().unwrap(), "title": "Amended budget", "goal": "Finish the marker",
        "criteria": ["Obsolete marker requirement", "Finish the marker"],
        "verify": [old_command], "checks": [{"criterion": 0, "run": old_command}],
        "land": false, "start": true,
    }));
    let id = created["id"].as_str().unwrap().to_string();
    wait_until("the session before the owner amendment", || {
        let task = s.daemon.request("task.get", json!({"id": id}));
        (task["attempts"][0]["sessionId"] == "sess-slow").then_some(())
    });
    let amended = s.daemon.request(
        "task.amend",
        json!({
            "id": id, "criteria": ["The owner replacement contract"],
            "verify": [current_command], "finalVerify": [current_command],
            "checks": [{"criterion": 0, "run": current_command}],
        }),
    );
    assert_eq!(amended["pending"], true, "{amended}");
    let task = settle(&s.daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    let attempts = task["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{task}");
    assert_eq!(attempts[0]["failure"]["kind"], "time_budget", "{task}");
    assert_eq!(attempts[0]["sessionId"], attempts[1]["sessionId"], "{task}");
    let args = argv(&s);
    assert_eq!(args.len(), 2, "{args:?}");
    assert!(args[1].contains("--resume sess-slow"), "{args:?}");
    let brief = delivered_brief(&s, 2);
    let current = brief.split("## Previous attempt evidence").next().unwrap();
    assert!(
        current.contains("The owner replacement contract"),
        "{brief}"
    );
    assert!(current.contains(current_command), "{brief}");
    assert!(!current.contains("OBSOLETE_MARKER.txt"), "{brief}");
    assert!(!current.contains("Obsolete marker requirement"), "{brief}");
    assert!(!brief.contains("fix the failed checks"), "{brief}");
    assert_run_cost(&s, &task, 2, 0.03, (2000, 750, 2000));
    cleanup(s, &task);
}
