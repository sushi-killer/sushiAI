//! Black-box integration tests (review): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::time::Duration;

/// Review by the Claude hard route, the same fake as the implementer (`auto`
/// picks a route on the other harness).
fn review_by_claude(daemon: &Daemon) {
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
}

#[test]
fn a_review_without_a_verdict_waits_for_the_owner_instead_of_passing() {
    // One fake plays both roles: the implementer edits a file and reports;
    // the reviewer (its brief opens with "## Review") answers in prose with
    // no sushi-review block, which used to count as PASS.
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\nbrief=$(cat)\ncase \"$brief\" in\n\"## Review\"*) echo '{\"type\":\"result\",\"result\":\"Looks fine to me.\"}' ;;\n*) echo changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}' ;;\nesac\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    // Review by the Claude hard route, which is the same fake (`auto` would
    // pick a route on the other harness).
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    assert_eq!(settings["review"], "auto");
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Review case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting" || s == "done" || s == "failed"
    });
    assert_eq!(waiting["status"], "waiting", "task JSON: {waiting}");
    let question = waiting["question"]["text"].as_str().unwrap();
    assert!(question.contains("no verdict"), "{question}");

    daemon.request(
        "task.answer",
        serde_json::json!({"id": task_id, "answer": "approve"}),
    );
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert!(settled["question"].is_null(), "task JSON: {settled}");
    let decisions = settled["decisions"].to_string();
    assert!(decisions.contains("Owner: approve"), "{decisions}");
    assert!(
        decisions.contains("without a review verdict"),
        "{decisions}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Like FAKE_RETRY_SCRIPT, but it also answers the advisor: a brief that
/// says an attempt failed gets $ADVISOR_MODE ("ok" answers, "fail" exits 1
/// with no output). Every run's brief is kept as $LOG_DIR/brief.<pid>.
const FAKE_ADVISOR_SCRIPT: &str = r#"#!/bin/sh
cat > "$LOG_DIR/brief.$$"
if grep -q 'An implement attempt at this task failed' "$LOG_DIR/brief.$$"; then
  echo advisor >> "$LOG_DIR/advisor.log"
  if [ "$ADVISOR_MODE" = fail ]; then exit 1; fi
  printf '%s\n' '{"type":"result","total_cost_usd":0.03,"usage":{"input_tokens":1,"output_tokens":1},"result":"Create SECOND, not FIRST."}'
  exit 0
fi
if [ -f FIRST ]; then echo x > SECOND; else echo x > FIRST; fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"handoff\":\"quick fix\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

#[test]
fn the_advisor_runs_once_before_a_retry_and_its_advice_reaches_the_next_brief() {
    for (advisor, mode) in [(true, "ok"), (true, "fail"), (false, "ok")] {
        let scripts_dir = tempfile::tempdir().unwrap();
        let script =
            fake_harness_script(scripts_dir.path(), "fake-advisor.sh", FAKE_ADVISOR_SCRIPT);
        let daemon = Daemon::spawn(&[
            ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
            ("LOG_DIR", scripts_dir.path().to_str().unwrap()),
            ("ADVISOR_MODE", mode),
        ]);
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["review"] = serde_json::json!("");
        daemon.request("settings.set", serde_json::json!({"settings": settings}));

        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "Two tries",
                "goal": "Needs a second attempt",
                "verify": ["test -f SECOND"],
                "variant": {"advisor": advisor},
                "start": true,
            }),
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
        let label = format!("advisor={advisor} mode={mode}: {settled}");
        assert_eq!(settled["status"], "done", "{label}");
        let attempts = settled["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 2, "{label}");
        let runs = std::fs::read_to_string(scripts_dir.path().join("advisor.log"))
            .map(|t| t.lines().count())
            .unwrap_or(0);
        let brief = std::fs::read_to_string(
            daemon
                .data_dir()
                .join("tasks")
                .join(&task_id)
                .join("runs/2/brief.md"),
        )
        .unwrap();
        let cost = settled["costUsd"].as_f64().unwrap();
        if advisor && mode == "ok" {
            assert_eq!(runs, 1, "{label}");
            assert_eq!(
                attempts[0]["advice"], "Create SECOND, not FIRST.",
                "{label}"
            );
            assert!(attempts[1].get("advice").is_none(), "{label}");
            assert!(
                brief.contains("## Advisor") && brief.contains("Create SECOND, not FIRST."),
                "{brief}"
            );
            // Two implement runs plus the advisor; the advisor's cost is on
            // no attempt.
            assert!((cost - 0.05).abs() < 1e-9, "{label}");
            assert_eq!(attempts[0]["costUsd"], 0.01, "{label}");
        } else if advisor {
            assert_eq!(runs, 1, "{label}");
            assert!(attempts[0].get("advice").is_none(), "{label}");
            assert!(!brief.contains("## Advisor"), "{brief}");
        } else {
            assert_eq!(runs, 0, "{label}");
            assert!(!brief.contains("## Advisor"), "{brief}");
            assert!((cost - 0.02).abs() < 1e-9, "{label}");
        }

        let worktree = task["worktree"].as_str().unwrap().to_string();
        daemon.shutdown_and_wait();
        let _ = std::fs::remove_dir_all(worktree);
    }
}

/// Implements like the other fakes; as reviewer, answers PASS but marks a
/// criterion unmet when the brief asks for a ruling per criterion.
const FAKE_CONTRACT_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Review"*)
  case "$brief" in
  *"Rule on every acceptance criterion"*)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[],\"criteria\":[{\"criterion\":\"Marker exists\",\"met\":false,\"evidence\":\"wrong file\"}]}\n```"}' ;;
  *)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
  esac
  printf '%s\n' "$json" ;;
*)
  echo changed > CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
  printf '%s\n' "$json" ;;
esac
"###;

#[test]
fn the_reviewer_s_unmet_criterion_fails_the_attempt() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-contract.sh", FAKE_CONTRACT_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(1);
    // Review by the Claude hard route, which is the same fake.
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Contract",
            "goal": "Write the marker",
            "criteria": ["Marker exists -- check: CHANGED_MARKER.txt"],
            "verify": ["true"],
                "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "waiting", "{settled}");
    let attempt = &settled["attempts"][0];
    assert_eq!(attempt["failure"]["kind"], "review", "{settled}");
    assert_eq!(
        attempt["review"]["findings"][0], "Unmet criterion: Marker exists (wrong file)",
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn auto_review_sends_claude_work_to_the_codex_reviewer() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let marker = scripts_dir.path().join("codex-reviewed");
    let codex = fake_harness_script(
        scripts_dir.path(),
        "fake-codex.sh",
        "#!/bin/sh\ncat > /dev/null\ntouch \"$CODEX_MARKER\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"```sushi-review\\n{\\\"verdict\\\":\\\"PASS\\\",\\\"findings\\\":[]}\\n```\"}}'\n",
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
        ("CODEX_MARKER", marker.to_str().unwrap()),
    ]);
    // review stays "auto": Sonnet's work would go to the Claude hard route.
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Other family",
            "goal": "Write the marker",
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(
        settled["attempts"][0]["routeId"], "claude-sonnet",
        "{settled}"
    );
    assert!(marker.exists(), "the review never ran on Codex: {settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn review_evidence_attaches_the_attempt_s_screenshots_to_the_codex_reviewer() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nmkdir -p artifacts\necho png > artifacts/after.png\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let args_log = scripts_dir.path().join("codex-args");
    let codex = fake_harness_script(
        scripts_dir.path(),
        "fake-codex.sh",
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$CODEX_ARGS\"\ncat > \"$CODEX_ARGS.brief\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"```sushi-review\\n{\\\"verdict\\\":\\\"PASS\\\",\\\"findings\\\":[]}\\n```\"}}'\n",
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
        ("CODEX_ARGS", args_log.to_str().unwrap()),
    ]);
    let repo = init_git_repo();
    std::fs::write(repo.path().join(".gitignore"), "artifacts/\n").unwrap();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Evidence",
            "goal": "Save a screenshot",
            "verify": ["true"],
            "variant": {"reviewEvidence": true},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");
    let argv = std::fs::read_to_string(&args_log).unwrap();
    assert!(
        argv.starts_with("exec --image=") && argv.contains("artifacts/after.png"),
        "{argv}"
    );
    let brief = std::fs::read_to_string(format!("{}.brief", args_log.display())).unwrap();
    assert!(
        brief.contains("## Screenshots") && brief.contains("evidence/after.png`"),
        "{brief}"
    );
    assert!(
        brief.contains("The implementer's own account") && brief.contains("<untrusted-data>\ndone"),
        "the reviewer hears the implementer's summary: {brief}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn final_checks_run_once_after_review_passes_and_gate_the_commit() {
    // The failing check passes on the base and fails once the marker exists.
    for (final_check, status) in [
        ("test ! -f CHANGED_MARKER.txt", "waiting"),
        ("true", "done"),
    ] {
        let scripts_dir = tempfile::tempdir().unwrap();
        // Implements, and as the (Claude) reviewer answers a plain PASS.
        let script = fake_harness_script(
            scripts_dir.path(),
            "fake-contract.sh",
            FAKE_SCREENSHOT_SCRIPT,
        );
        let ran_log = scripts_dir.path().join("final.log");
        let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["maxAttempts"] = serde_json::json!(1);
        settings["review"] = serde_json::json!("claude-opus");
        daemon.request("settings.set", serde_json::json!({"settings": settings}));

        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "Final checks",
                "goal": "Write the marker",
                "verify": ["true"],
                "finalVerify": [format!("echo ran >> {}; {final_check}", ran_log.display())],
                "start": true,
            }),
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
        assert_eq!(settled["status"], status, "{final_check}: {settled}");
        let attempt = &settled["attempts"][0];
        assert_eq!(attempt["review"]["verdict"], "PASS", "{settled}");
        if status == "waiting" {
            assert!(
                attempt["failure"]["detail"]
                    .as_str()
                    .unwrap()
                    .starts_with("Final check echo ran"),
                "{settled}"
            );
            assert_eq!(settled["question"]["kind"], "attempts_failing", "{settled}");
        }
        let worktree = task["worktree"].as_str().unwrap().to_string();
        let ran = std::fs::read_to_string(&ran_log).unwrap();
        // Once on the base before the attempt, once after review passes.
        assert_eq!(ran.lines().count(), 2, "{final_check}: {ran}");
        let brief = std::fs::read_to_string(
            daemon
                .data_dir()
                .join("tasks")
                .join(&task_id)
                .join("runs/1/brief.md"),
        )
        .unwrap();
        assert!(
            brief.contains("## Final checks") && brief.contains("do not run them yourself"),
            "{brief}"
        );

        daemon.shutdown_and_wait();
        let _ = std::fs::remove_dir_all(&worktree);
    }
}

#[test]
fn a_failed_final_check_shows_the_failing_test_not_the_bundler_noise() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-contract.sh",
        FAKE_SCREENSHOT_SCRIPT,
    );
    let check = fake_harness_script(
        scripts_dir.path(),
        "noisy-check.sh",
        "#!/bin/sh\n[ -f CHANGED_MARKER.txt ] || exit 0\ni=0\nwhile [ $i -lt 300 ]; do echo \"WARNING bundler chunk $i is large\" >&2; i=$((i+1)); done\necho 'test a_broken_thing ... FAILED'\necho \"thread 'a_broken_thing' panicked at t.rs:3:5:\"\necho 'assertion failed: boom_message'\nj=0\nwhile [ $j -lt 300 ]; do echo \"test fine_$j ... ok\"; j=$((j+1)); done\nexit 1\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(1);
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Noisy final check",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [check.to_str().unwrap()],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(30));
    assert_eq!(settled["status"], "waiting", "{settled}");
    let detail = settled["attempts"][0]["failure"]["detail"]
        .as_str()
        .unwrap();
    assert!(detail.contains("a_broken_thing ... FAILED"), "{detail}");
    assert!(detail.contains("boom_message"), "{detail}");
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(&worktree);
}

/// Implements; saves a screenshot only once its brief says none was saved
/// (the evidence failure's feedback); as the reviewer answers PASS.
const FAKE_SCREENSHOT_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Review"*)
  printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*)
  echo changed > CHANGED_MARKER.txt
  case "$brief" in
  *"no image was saved"*) mkdir -p artifacts/ui; echo png > artifacts/ui/panel.png ;;
  esac
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

#[test]
fn a_visual_criterion_needs_a_saved_image_and_the_copies_outlive_the_worktree() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-shot.sh", FAKE_SCREENSHOT_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_by_claude(&daemon);
    let repo = init_git_repo();
    std::fs::write(repo.path().join(".gitignore"), "artifacts/\n").unwrap();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Visual",
            "goal": "Change the panel",
            "criteria": ["The panel shows thumbnails -- check: screenshot under artifacts/"],
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(30));
    assert_eq!(settled["status"], "done", "{settled}");

    let attempts = settled["attempts"].as_array().unwrap();
    let implement: Vec<_> = attempts
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert_eq!(implement[0]["failure"]["kind"], "evidence", "{settled}");
    let feedback = implement[0]["failure"]["detail"].as_str().unwrap();
    assert!(
        feedback.contains("The panel shows thumbnails") && feedback.contains("under artifacts/"),
        "{feedback}"
    );
    assert!(implement[0].get("evidence").is_none(), "{settled}");
    assert_eq!(
        implement[1]["failure"],
        serde_json::Value::Null,
        "{settled}"
    );

    // Gone with the worktree, still listed and still on disk.
    let worktree = task["worktree"].as_str().unwrap().to_string();
    let _ = std::fs::remove_dir_all(&worktree);
    let got = daemon.request("task.get", serde_json::json!({"id": task_id}));
    let listed: Vec<&str> = got["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["evidence"].as_array().into_iter().flatten())
        .filter_map(|e| e.as_str())
        .collect();
    assert_eq!(listed.len(), 1, "{got}");
    assert!(listed[0].contains("/runs/2/evidence/"), "{listed:?}");
    assert_eq!(std::fs::read_to_string(listed[0]).unwrap().trim(), "png");
    // "png\n" as a data URL.
    let url = daemon.request(
        "task.evidence",
        serde_json::json!({"id": task_id, "path": listed[0]}),
    );
    assert_eq!(url["dataUrl"], "data:image/png;base64,cG5nCg==", "{url}");

    daemon.shutdown_and_wait();
}

/// Attempt 1 writes source and an image and its review FAILs with a plain
/// finding; attempt 2 writes `ATTEMPT_TWO_FILE` and no image, its review
/// PASSes. Counters in the scripts dir tell the runs apart.
const FAKE_RETRY_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
d=$(dirname "$0")
case "$brief" in
"## Review"*)
  echo x >> "$d/review.count"
  if [ "$(wc -l < "$d/review.count" | tr -d ' ')" = 1 ]; then
    printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"P1: the label is wrong\"]}\n```"}'
  else
    printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}'
  fi ;;
*)
  echo x >> "$d/impl.count"
  if [ "$(wc -l < "$d/impl.count" | tr -d ' ')" = 1 ]; then
    mkdir -p src artifacts/ui
    echo one > src/panel.txt
    echo png > artifacts/ui/panel.png
  else
    mkdir -p ATTEMPT_TWO_DIR
    echo two > ATTEMPT_TWO_FILE
  fi
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

fn run_retry_task(second_file: &str) -> (Daemon, serde_json::Value, String) {
    let scripts_dir = tempfile::tempdir().unwrap();
    let dir = std::path::Path::new(second_file)
        .parent()
        .unwrap()
        .to_str()
        .unwrap();
    let body = FAKE_RETRY_SCRIPT
        .replace("ATTEMPT_TWO_DIR", dir)
        .replace("ATTEMPT_TWO_FILE", second_file);
    let script = fake_harness_script(scripts_dir.path(), "fake-retry.sh", &body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_by_claude(&daemon);
    let repo = init_git_repo();
    std::fs::write(repo.path().join(".gitignore"), "artifacts/\n").unwrap();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Visual retry",
            "goal": "Change the panel",
            "criteria": ["The panel shows thumbnails -- check: screenshot under artifacts/"],
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(40));
    // The scripts dir must outlive the run.
    drop(scripts_dir);
    (daemon, settled, task_id)
}

#[test]
fn a_retry_that_changed_only_tests_reuses_the_accepted_evidence() {
    let (daemon, settled, task_id) = run_retry_task("tests/panel.test.txt");
    assert_eq!(settled["status"], "done", "{settled}");
    let implement: Vec<_> = settled["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert_eq!(implement.len(), 2, "{settled}");
    assert!(implement[1].get("failure").is_none(), "{settled}");
    let paths: Vec<&str> = implement[1]["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e.as_str())
        .collect();
    assert_eq!(paths.len(), 1, "{settled}");
    assert!(paths[0].contains("/runs/1/evidence/"), "{paths:?}");
    assert_eq!(implement[1]["evidenceFrom"], 1, "{settled}");
    assert!(implement[0].get("evidenceFrom").is_none(), "{settled}");
    let brief = std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(&task_id)
            .join("runs/2/review/brief.md"),
    )
    .unwrap();
    assert!(brief.contains(paths[0]), "{brief}");
    assert!(brief.contains("saved by attempt 1"), "{brief}");
    let decisions: Vec<&str> = settled["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect();
    assert!(
        decisions
            .iter()
            .any(|d| d.contains("attempt 2") && d.contains("attempt 1")),
        "{decisions:?}"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn a_retry_that_changed_source_still_needs_new_evidence() {
    let (daemon, settled, _task_id) = run_retry_task("src/other.txt");
    let implement: Vec<_> = settled["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert!(implement.len() >= 2, "{settled}");
    assert_eq!(implement[1]["failure"]["kind"], "evidence", "{settled}");
    let detail = implement[1]["failure"]["detail"].as_str().unwrap();
    assert!(detail.contains("src/other.txt"), "{detail}");
    assert!(implement[1].get("evidenceFrom").is_none(), "{settled}");
    daemon.shutdown_and_wait();
}

#[test]
fn a_gif_or_svg_saved_for_a_visual_criterion_counts_as_evidence() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let body = FAKE_SCREENSHOT_SCRIPT.replace("panel.png", "panel.svg");
    let script = fake_harness_script(scripts_dir.path(), "fake-svg.sh", &body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_by_claude(&daemon);
    let repo = init_git_repo();
    std::fs::write(repo.path().join(".gitignore"), "artifacts/\n").unwrap();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Visual svg",
            "goal": "Change the panel",
            "criteria": ["Shows a chart -- check: svg image under artifacts/"],
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(30));
    assert_eq!(settled["status"], "done", "{settled}");
    let listed: Vec<String> = settled["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|a| a["evidence"].as_array().into_iter().flatten())
        .filter_map(|e| e.as_str().map(String::from))
        .collect();
    assert_eq!(listed.len(), 1, "{settled}");
    assert!(listed[0].ends_with("panel.svg"), "{listed:?}");
    let url = daemon.request(
        "task.evidence",
        serde_json::json!({"id": task_id, "path": listed[0]}),
    );
    assert!(
        url["dataUrl"]
            .as_str()
            .unwrap()
            .starts_with("data:image/svg+xml;base64,"),
        "{url}"
    );
    daemon.shutdown_and_wait();
}

/// Implements (appending, so every attempt's diff differs), answers the
/// advisor, and reviews: FAIL with a plain finding at first, and once the
/// review brief carries the previous review, FAIL with the same finding
/// marked `repeat`. Every review brief is kept as $LOG_DIR/review.<pid>.
const FAKE_REPEAT_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
*"An implement attempt at this task failed"*)
  echo advisor >> "$LOG_DIR/advisor.log"
  printf '%s\n' '{"type":"result","total_cost_usd":0.03,"usage":{"input_tokens":1,"output_tokens":1},"result":"Fix the null check."}' ;;
"## Review"*)
  printf '%s' "$brief" > "$LOG_DIR/review.$$"
  case "$brief" in
  *"## Previous review"*)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[{\"finding\":\"P1: a.rs:1 - null check missing\",\"repeat\":true}]}\n```"}' ;;
  *)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"P1: a.rs:1 - null check missing\"]}\n```"}' ;;
  esac
  printf '%s\n' "$json" ;;
*)
  echo changed >> CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

#[test]
fn a_repeated_review_finding_calls_the_advisor_and_tiers_up_then_the_second_escalates() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-repeat.sh", FAKE_REPEAT_SCRIPT);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("LOG_DIR", scripts_dir.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(6);
    settings["answerPolicy"] = serde_json::json!(false);
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Repeat",
            "goal": "Write the marker",
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(30));
    assert_eq!(settled["status"], "waiting", "{settled}");
    assert_eq!(settled["question"]["kind"], "attempts_failing", "{settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    let implements: Vec<&serde_json::Value> = attempts
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    // Attempt 1: a first finding. Attempt 2: the same finding, marked as a
    // repeat -> advisor and a tier up. Attempt 3: repeated again -> waiting.
    assert_eq!(implements.len(), 3, "{settled}");
    assert!(
        implements[0]["review"].get("repeated").is_none(),
        "{settled}"
    );
    assert_eq!(
        implements[1]["review"]["repeated"][0], "P1: a.rs:1 - null check missing",
        "{settled}"
    );
    assert_eq!(implements[0]["routeId"], "claude-sonnet", "{settled}");
    assert_eq!(implements[1]["routeId"], "claude-sonnet", "{settled}");
    assert_eq!(implements[2]["routeId"], "claude-opus", "{settled}");
    assert_eq!(implements[1]["advice"], "Fix the null check.", "{settled}");
    assert!(implements[0].get("advice").is_none(), "{settled}");
    let runs = std::fs::read_to_string(scripts_dir.path().join("advisor.log"))
        .map(|t| t.lines().count())
        .unwrap_or(0);
    assert_eq!(runs, 1, "{settled}");
    let reviews: Vec<String> = std::fs::read_dir(scripts_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("review."))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect();
    assert_eq!(reviews.len(), 3, "one review per attempt");
    assert_eq!(
        reviews
            .iter()
            .filter(|b| b.contains("## Previous review") && b.contains("null check missing"))
            .count(),
        2,
        "attempts 2 and 3 carry the previous findings"
    );
    let decisions = settled["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(|d| d
            .as_str()
            .unwrap()
            .contains("a review finding repeated -> advisor, tier hard")),
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Implements (appending), answers the advisor, and reviews: FAIL with a
/// plain finding first, FAIL with it marked `repeat` once the review brief
/// carries the previous review, PASS once it names a dropped finding. An
/// implement brief that shows a review failure gets a report disputing it.
const FAKE_DISPUTE_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
*"An implement attempt at this task failed"*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.03,"usage":{"input_tokens":1,"output_tokens":1},"result":"Fix the null check."}' ;;
"## Review"*)
  case "$brief" in
  *"## Findings dropped by a tie-break judge"*)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
  *"## Previous review"*)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[{\"finding\":\"P1: a.rs:1 - null check missing\",\"repeat\":true}]}\n```"}' ;;
  *)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"P1: a.rs:1 - null check missing\"]}\n```"}' ;;
  esac
  printf '%s\n' "$json" ;;
*)
  echo changed >> CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  case "$brief" in
  *"The last attempt failed review"*)
    report='```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\",\"disputes\":[{\"finding\":\"a.rs:1 - null check missing\",\"rebuttal\":\"the check is at a.rs:1\",\"evidence\":[\"a.rs:1\"]}]}\n```' ;;
  *)
    report='```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```' ;;
  esac
  printf '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"%s"}\n' "$report" ;;
esac
"###;

fn judge_script(finding_valid: bool) -> String {
    format!(
        "#!/bin/sh\n[ \"$1\" = --version ] && {{ echo codex-cli 0.1; exit 0; }}\ncat > /dev/null\necho judge >> \"$LOG_DIR/judge.log\"\nprintf '%s\\n' '{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"```sushi-judge\\n{{\\\"finding_valid\\\": {finding_valid}, \\\"why\\\": \\\"the ruling\\\"}}\\n```\"}}}}'\n"
    )
}

/// Runs the dispute flow with a judge answering `finding_valid`; returns the
/// settled task and the number of judge runs.
fn run_dispute_task(finding_valid: bool, wait: Duration) -> (serde_json::Value, usize) {
    let scripts_dir = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(scripts_dir.path(), "fake-dispute.sh", FAKE_DISPUTE_SCRIPT);
    let codex = fake_harness_script(
        scripts_dir.path(),
        "fake-judge.sh",
        &judge_script(finding_valid),
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
        ("LOG_DIR", scripts_dir.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(6);
    settings["answerPolicy"] = serde_json::json!(false);
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Dispute",
            "goal": "Write the marker",
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, wait);
    let runs = std::fs::read_to_string(scripts_dir.path().join("judge.log"))
        .map(|t| t.lines().count())
        .unwrap_or(0);
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
    (settled, runs)
}

fn decision_lines(task: &serde_json::Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str().map(String::from))
        .collect()
}

#[test]
fn a_disputed_repeated_finding_judged_invalid_is_dropped_and_the_task_commits() {
    let (settled, runs) = run_dispute_task(false, Duration::from_secs(30));
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(runs, 1, "{settled}");
    let assumption = settled["assumptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["by"] == "review-judge")
        .unwrap_or_else(|| panic!("no review-judge assumption: {settled}"));
    assert_eq!(assumption["answer"], "finding dropped", "{settled}");
    assert_eq!(assumption["kind"], "review_dispute", "{settled}");
    assert_eq!(assumption["attempt"], 2, "{settled}");
    assert_eq!(assumption["evidence"], "the ruling", "{settled}");
    assert!(
        decision_lines(&settled).iter().any(|d| d
            .starts_with("Orchestrator: disputed review finding dropped by the judge (")
            && d.contains("null check missing")),
        "{settled}"
    );
}

#[test]
fn a_disputed_finding_judged_valid_goes_on_to_the_existing_escalation() {
    let (settled, runs) = run_dispute_task(true, Duration::from_secs(30));
    assert_eq!(settled["status"], "waiting", "{settled}");
    assert_eq!(settled["question"]["kind"], "attempts_failing", "{settled}");
    assert_eq!(runs, 1, "the finding is judged once: {settled}");
    let implements: Vec<&serde_json::Value> = settled["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect();
    assert_eq!(implements.len(), 3, "{settled}");
    assert_eq!(implements[1]["failure"]["kind"], "review", "{settled}");
    assert_eq!(implements[2]["routeId"], "claude-opus", "{settled}");
    let decisions = decision_lines(&settled);
    assert!(
        decisions
            .iter()
            .any(|d| d.contains("a review finding repeated -> advisor, tier hard")),
        "{settled}"
    );
    assert!(
        decisions
            .iter()
            .any(|d| d.starts_with("Orchestrator: disputed review finding kept by the judge (")),
        "{settled}"
    );
    assert!(settled["assumptions"]
        .as_array()
        .map(|a| a.iter().all(|x| x["by"] != "review-judge"))
        .unwrap_or(true));
}
