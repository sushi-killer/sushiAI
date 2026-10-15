use super::*;

#[test]
fn a_failed_check_resumes_the_session_with_its_failure_tail_and_own_spend() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-retry.sh", FAKE_RETRY_SCRIPT);
    let args_log = scripts_dir.path().join("args.log");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("ARGS_LOG", args_log.to_str().unwrap()),
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
            "verify": ["test -f SECOND || { echo RETRY_FAILURE_TAIL; exit 7; }"],
                "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id);
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{settled}");
    assert_eq!(attempts[0]["handoff"], "tried the quick fix", "");

    let argv: Vec<String> = std::fs::read_to_string(&args_log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(argv.len(), 2, "{argv:?}");
    assert!(!argv[0].contains("--resume"), "{argv:?}");
    assert!(argv[1].contains("--resume sess-fake"), "{argv:?}");
    assert_eq!(attempts[0]["failure"]["kind"], "verify", "{settled}");
    assert!(
        (settled["costUsd"].as_f64().unwrap() - 0.02).abs() < 1e-9,
        "{settled}"
    );
    assert!(
        (attempts[1]["costUsd"].as_f64().unwrap() - 0.01).abs() < 1e-9,
        "{settled}"
    );
    assert!(
        settled["decisions"]
            .to_string()
            .contains("resumes session sess-fake"),
        "{settled}"
    );
    {
        let brief = std::fs::read_to_string(args_log.with_file_name("args.log.brief.2")).unwrap();
        let saved_brief = std::fs::read_to_string(
            daemon
                .data_dir()
                .join("tasks")
                .join(&task_id)
                .join("runs/2/brief.md"),
        )
        .unwrap();
        assert_eq!(brief, saved_brief);
        assert!(!brief.contains("## Task"), "{brief}");
        assert!(brief.contains("## Failed checks"), "{brief}");
        assert!(
            brief.contains("test -f SECOND") && brief.contains("exited 7."),
            "{brief}"
        );
        assert!(brief.contains("RETRY_FAILURE_TAIL"), "{brief}");
        assert!(brief.contains("## Report format"), "{brief}");
    }

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_rejected_resume_falls_back_to_a_full_brief_in_the_same_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "reject-resume.sh", FAKE_RETRY_SCRIPT);
    let log = scripts.path().join("args.log");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("ARGS_LOG", log.to_str().unwrap()),
        ("REJECT_RESUME", "1"),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(), "title": "Rejected resume",
            "goal": "Create SECOND", "verify": ["test -f SECOND"], "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap();
    let settled = poll_task_status(&daemon, id);
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{settled}");
    assert_eq!(attempts[1]["n"], 2);
    let args = std::fs::read_to_string(&log).unwrap();
    let argv: Vec<_> = args.lines().collect();
    assert_eq!(argv.len(), 3, "{args}");
    assert!(argv[1].contains("--resume sess-fake"), "{args}");
    assert!(!argv[2].contains("--resume"), "{args}");
    let brief = std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(id)
            .join("runs/2/brief.md"),
    )
    .unwrap();
    assert!(
        brief.contains("## Task") && brief.contains("test -f SECOND exited 1."),
        "{brief}"
    );
    assert!(settled["decisions"].to_string().contains("resume rejected without a session init; falling back to a fresh run in the same attempt"), "{settled}");
    assert!(
        (settled["costUsd"].as_f64().unwrap() - 0.02).abs() < 1e-9,
        "{settled}"
    );
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_repeated_review_finding_starts_a_fresh_implement_session() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts.path(),
        "repeat.sh",
        r###"#!/bin/sh
brief=$(cat)
case "$brief" in
'## Route the review'*) exit 0 ;;
"## Review"*)
  n=$(cat "$LOG_DIR/implements")
  if [ "$n" = 1 ]; then
    json='{"verdict":"FAIL","findings":["[P1] marker.txt:1 is wrong"]}'
  elif [ "$n" = 2 ]; then
    json='{"verdict":"FAIL","findings":[{"finding":"[P1] marker.txt:1 is wrong","id":"F-1"}]}'
  else json='{"verdict":"PASS","findings":[]}'; fi
  escaped=$(printf '%s' "$json" | sed 's/"/\\"/g')
  printf '%s\n' "{\"type\":\"result\",\"result\":\"\`\`\`sushi-review\n$escaped\n\`\`\`\"}" ;;
*"An implement attempt at this task failed"*) printf '%s\n' '{"type":"result","result":"Fix the marker."}' ;;
*)
  printf '%s\n' "$*" >> "$LOG_DIR/args.log"
  n=$(cat "$LOG_DIR/implements" 2>/dev/null || echo 0)
  n=$((n+1)); echo "$n" > "$LOG_DIR/implements"
  echo "$n" > marker.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###,
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("LOG_DIR", scripts.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request("task.create", serde_json::json!({
        "repo": repo.path().to_str().unwrap(), "title": "Repeated review",
        "goal": "Fix marker.txt", "verify": ["true"], "start": true,
        "variant": {"tierRoutes": {"mechanical":"claude-sonnet", "standard":"claude-sonnet", "hard":"claude-sonnet"}},
    }));
    let id = task["id"].as_str().unwrap();
    let settled = poll_task_status(&daemon, id);
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 3, "{settled}");
    assert_eq!(attempts[1]["failure"]["kind"], "review", "{settled}");
    // The reviewer's finding id is appended to the repeated finding.
    let repeated = attempts[1]["review"]["repeated"].as_array().unwrap();
    assert_eq!(repeated.len(), 1, "{settled}");
    assert!(
        repeated[0]
            .as_str()
            .unwrap()
            .starts_with("[P1] marker.txt:1 is wrong"),
        "{settled}"
    );
    assert!(
        attempts.iter().all(|a| a["routeId"] == "claude-sonnet"),
        "{settled}"
    );
    assert!(
        settled["decisions"]
            .to_string()
            .contains("a review finding repeated"),
        "{settled}"
    );
    let args = std::fs::read_to_string(scripts.path().join("args.log")).unwrap();
    assert_eq!(args.lines().count(), 3, "{args}");
    assert!(!args.contains("--resume"), "{args}");
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
