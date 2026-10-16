//! Criterion-only checks retain their authority through the public amendment RPC.
mod common;

use common::polling::remove_dir_retrying;
use common::process::Daemon;
use common::repository::init_git_repo;

use serde_json::{json, Value};

fn criterion_task(daemon: &Daemon, repo: &std::path::Path, source: Option<&str>) -> Value {
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo, "title": "Criterion authority", "goal": "Keep the check",
            "criteria": ["Works -- check: ./scripts/gate.sh"], "verify": [], "start": false
        }),
    );
    let path = daemon
        .data_dir()
        .join("tasks")
        .join(task["id"].as_str().unwrap())
        .join("task.json");
    let mut saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved["verify"] = json!([]);
    saved["finalVerify"] = json!([]);
    saved["checks"] = json!([]);
    saved.as_object_mut().unwrap().remove("heldOut");
    saved["plannerTests"] = match source {
        Some(source) => json!({"sources": [{"command": "./scripts/gate.sh", "source": source}]}),
        None => json!({}),
    };
    std::fs::write(path, serde_json::to_vec(&saved).unwrap()).unwrap();
    daemon.request("task.get", json!({"id": task["id"]}))
}

#[test]
fn a_public_amendment_cannot_hide_a_shared_criterion_check_in_prose_or_a_prefix() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let mut worktrees = Vec::new();
    for source in [Some("owner"), Some("repo"), None] {
        let task = criterion_task(&daemon, repo.path(), source);
        worktrees.push(task["worktree"].as_str().unwrap().to_owned());
        for replacement in [
            "Works; ./scripts/gate.sh was mentioned",
            "Works -- check: ./scripts/gate.sh-extra",
        ] {
            let error = daemon.request_error(
                "task.amend",
                json!({
                    "id": task["id"], "source": "overseer", "criteria": [replacement]
                }),
            );
            assert!(
                error.contains("only the owner may drop"),
                "{source:?}: {error}"
            );
            let unchanged = daemon.request("task.get", json!({"id": task["id"]}));
            assert_eq!(unchanged["criteria"], task["criteria"]);
            assert_eq!(unchanged["checks"], task["checks"]);
        }
        daemon.request(
            "task.amend",
            json!({"id": task["id"], "criteria": ["Works; ./scripts/gate.sh was mentioned"]}),
        );
        let owned = daemon.request("task.get", json!({"id": task["id"]}));
        assert_eq!(
            owned["criteria"],
            json!(["Works; ./scripts/gate.sh was mentioned"])
        );
    }
    let task = criterion_task(&daemon, repo.path(), Some("planner"));
    worktrees.push(task["worktree"].as_str().unwrap().to_owned());
    daemon.request("task.amend", json!({"id": task["id"], "source": "overseer", "criteria": ["Works; ./scripts/gate.sh was mentioned"]}));
    let planned = daemon.request("task.get", json!({"id": task["id"]}));
    assert_eq!(
        planned["criteria"],
        json!(["Works; ./scripts/gate.sh was mentioned"])
    );
    daemon.shutdown_and_wait();
    for worktree in worktrees {
        remove_dir_retrying(worktree);
    }
}
