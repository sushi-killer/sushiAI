//! Black-box integration tests for `task.amend`: the amendment of a task with
//! a live loop reaches the loop, and grounded checks are baselined on the base.

mod common;

use common::*;
use serde_json::json;
use std::time::Duration;

#[test]
fn an_amendment_during_an_attempt_reaches_the_next_review_brief() {
    let scripts = tempfile::tempdir().unwrap();
    let started = scripts.path().join("started");
    let go = scripts.path().join("go");
    let claude = fake_harness_script(
        scripts.path(),
        "fake-claude.sh",
        &format!(
            "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\ntouch {started}\nwhile [ ! -f {go} ]; do sleep 0.1; done\nprintf '%s\\n' '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}}'\nprintf '%s\\n' '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"```sushi-report\\n{{\\\\\"outcome\\\\\":\\\\\"complete\\\\\",\\\\\"summary\\\\\":\\\\\"done\\\\\",\\\\\"decisions\\\\\":[],\\\\\"question\\\\\":\\\\\"\\\\\"}}\\n```\"}}'\n",
            started = started.display(),
            go = go.display()
        ),
    );
    let brief_log = scripts.path().join("codex-brief");
    let codex = fake_harness_script(
        scripts.path(),
        "fake-codex.sh",
        "#!/bin/sh\ncat > \"$BRIEF_LOG\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"```sushi-review\\n{\\\"verdict\\\":\\\"PASS\\\",\\\"findings\\\":[]}\\n```\"}}'\n",
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
        ("BRIEF_LOG", brief_log.to_str().unwrap()),
    ]);
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Amend",
            "goal": "Write a marker",
            "criteria": ["the OLDCRITERION holds"],
            "verify": ["true"],
            "variant": {"reviewOtherFamily": true},
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let begun = std::time::Instant::now();
    while !started.exists() {
        assert!(
            begun.elapsed() < Duration::from_secs(20),
            "attempt never began"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let amended = daemon.request(
        "task.amend",
        json!({"id": id, "criteria": ["the NEWCRITERION holds"]}),
    );
    assert_eq!(amended["pending"], true, "{amended}");
    std::fs::write(&go, "").unwrap();

    let done = poll_task_status(&daemon, &id, Duration::from_secs(30));
    assert_eq!(done["status"], "done", "{done}");
    let brief = std::fs::read_to_string(&brief_log).unwrap();
    assert!(brief.contains("NEWCRITERION"), "{brief}");
    assert!(!brief.contains("OLDCRITERION"), "{brief}");
    assert_eq!(done["criteria"], json!(["the NEWCRITERION holds"]));
    assert!(done["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d.as_str().unwrap().starts_with("Amended: criteria")));

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn amended_checks_are_baselined_on_the_base_and_only_failing_ones_gate() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Grounded amend",
            "goal": "Something",
            "criteria": ["a", "b"],
            "variant": {"groundedChecks": true},
            "start": false,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let result = daemon.request(
        "task.amend",
        json!({
            "id": id,
            "checks": [
                {"criterion": 0, "run": "test -f NOPE.txt"},
                {"criterion": 1, "run": "test -f README.md"},
            ],
            "heldOut": {"criterion": 0, "run": "test -f NOPE2.txt"},
        }),
    );
    assert_eq!(result["pending"], false, "{result}");
    let saved = daemon.request("task.get", json!({"id": id}));
    assert_eq!(saved["checks"][0]["baseline"], "fail", "{saved}");
    assert_eq!(saved["checks"][1]["baseline"], "pass", "{saved}");
    assert_eq!(saved["heldOut"]["baseline"], "fail", "{saved}");
    let decisions: Vec<&str> = saved["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert!(
        decisions.contains(&"Amended: checks, heldOut"),
        "{decisions:?}"
    );
    assert!(
        decisions
            .iter()
            .any(|d| d.contains("already passes on base")),
        "{decisions:?}"
    );
    assert!(
        decisions.iter().all(|d| !d.contains("NOPE2")),
        "the held-out command never appears: {decisions:?}"
    );
    assert_eq!(
        git_out(repo.path(), &["worktree", "list"]).lines().count(),
        2
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}
