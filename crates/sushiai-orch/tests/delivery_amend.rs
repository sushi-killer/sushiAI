mod common;

use common::process::Daemon;

use common::harness::fake_harness_script;
use common::polling::{remove_dir_retrying, settle, wait_until};
use common::repository::{git_out, init_git_repo};

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Release(PathBuf);

impl Drop for Release {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "release");
    }
}

fn assert_committed_contract(repo: &Path, task: &Value) {
    let branch = task["branch"].as_str().unwrap();
    let files = git_out(repo, &["ls-tree", "-r", "--name-only", branch]);
    assert!(
        files.lines().any(|file| file == "CURRENT_VERIFY_RAN"),
        "{files}"
    );
    assert!(
        files.lines().any(|file| file == "CURRENT_FINAL_RAN"),
        "{files}"
    );
    let receipt = git_out(
        repo,
        &[
            "log",
            "-1",
            "--format=%(trailers:key=Task-Id,valueonly)%x00%(trailers:key=Attempt,valueonly)",
            branch,
        ],
    );
    let (id, attempt) = receipt.split_once('\0').unwrap();
    assert_eq!(id.trim(), task["id"].as_str().unwrap());
    assert_eq!(attempt.trim(), "1");
}

#[test]
fn an_owner_edit_during_final_checks_rechecks_the_same_attempt_before_delivery() {
    let scratch = tempfile::tempdir().unwrap();
    let args = scratch.path().join("harness-args.log");
    let script = fake_harness_script(
        scratch.path(),
        "late-amend.sh",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$ARGS_LOG"
cat > /dev/null
echo changed > CHANGED_MARKER.txt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-late-amend"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#,
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("ARGS_LOG", args.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["sandbox"] = json!("host");
    settings["review"] = json!("");
    settings["maxAttempts"] = json!(1);
    settings["tiers"] = json!({"mechanical": "claude-sonnet", "standard": "claude-sonnet", "hard": "claude-sonnet"});
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let entered = scratch.path().join("final-check-entered");
    let released = scratch.path().join("final-check-released");
    let _release = Release(released.clone());
    let old_final = format!(
        "if test -f CHANGED_MARKER.txt; then touch '{}'; while ! test -f '{}'; do sleep 0.01; done; fi",
        entered.display(), released.display()
    );
    let created = daemon.request(
        "task.create",
        json!({
            "repo": repo.path(), "title": "Late owner edit", "goal": "Finish the marker",
            "criteria": ["Original contract"], "verify": ["test -f CHANGED_MARKER.txt"],
            "finalVerify": [old_final], "land": false, "start": true,
        }),
    );
    let id = created["id"].as_str().unwrap();
    wait_until("the final check after the implement run", || {
        entered.exists().then_some(())
    });
    let current_verify = "test -f CHANGED_MARKER.txt && touch CURRENT_VERIFY_RAN";
    let current_final = "test -f CURRENT_VERIFY_RAN && touch CURRENT_FINAL_RAN";
    let amended = daemon.request(
        "task.amend",
        json!({
            "id": id, "criteria": ["Current owner contract"],
            "verify": [current_verify], "finalVerify": [current_final],
        }),
    );
    assert_eq!(amended["pending"], true, "{amended}");
    std::fs::write(&released, "release").unwrap();
    let saved = settle(&daemon, id);
    assert_eq!(saved["status"], "done", "{saved}");
    assert_eq!(saved["criteria"], json!(["Current owner contract"]));
    assert_eq!(saved["verify"], json!([current_verify]));
    assert_eq!(saved["finalVerify"], json!([current_final]));
    let attempts = saved["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1, "{saved}");
    assert_eq!(attempts[0]["n"], 1);
    assert_eq!(attempts[0]["sessionId"], "sess-late-amend");
    assert_eq!(attempts[0]["status"], "passed");
    assert!(
        (saved["costUsd"].as_f64().unwrap() - 0.01).abs() < 1e-9,
        "{saved}"
    );
    assert_eq!(std::fs::read_to_string(args).unwrap().lines().count(), 1);
    assert_committed_contract(repo.path(), &saved);
    let costs = daemon.request("costs.summary", json!({"groupBy": ["stage"]}));
    assert_eq!(costs["totals"]["runs"], 1, "{costs}");
    assert!(
        (costs["totals"]["costUsd"].as_f64().unwrap() - 0.01).abs() < 1e-9,
        "{costs}"
    );
    daemon.shutdown_and_wait();
    remove_dir_retrying(Path::new(created["worktree"].as_str().unwrap()));
}
