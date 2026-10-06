//! Black-box integration tests (settings.scopedChecks): a command a scoped
//! entry matches only runs when the task's diff touches the entry's paths.

mod common;

use common::*;
use serde_json::{json, Value};

/// The implementer writes `$IMPL_FILE` (creating its directory) and reports
/// done; no reviewer, no plan.
const FAKE: &str = r#"#!/bin/sh
cat > "$LOG_DIR/brief.$$"
mkdir -p "$(dirname "$IMPL_FILE")"
echo changed > "$IMPL_FILE"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

const MARKER_CMD: &str = r#"touch "$LOG_DIR/desktop-ran""#;

fn run_with_diff_in(file: &str) -> (Value, std::path::PathBuf, tempfile::TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    let path = fake_harness_script(scripts.path(), "fake-claude.sh", FAKE);
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", path.to_str().unwrap()),
        ("LOG_DIR", log.path().to_str().unwrap()),
        ("IMPL_FILE", file),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["sandbox"] = json!("host");
    settings["scopedChecks"] = json!([
        {"command": MARKER_CMD, "paths": ["electron/**", "src/app/**"]}
    ]);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Scoped",
            "goal": "Write the file",
            "verify": [MARKER_CMD, "true"],
        }),
    );
    let done = settle(&daemon, task["id"].as_str().unwrap());
    let marker = log.path().join("desktop-ran");
    let worktree = done["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
    drop(scripts);
    (done, marker, log)
}

fn decisions(task: &Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_task_that_touches_only_orchd_skips_the_scoped_check() {
    let (done, marker, _log) = run_with_diff_in("orchd/src/x.rs");
    assert_eq!(done["status"], "done", "{done}");
    assert!(!marker.exists(), "the scoped command ran");
    let skip =
        format!("Orchestrator: skipped {MARKER_CMD}: no change under electron/**, src/app/**");
    assert!(decisions(&done).contains(&skip), "{done}");
}

#[test]
fn a_task_that_touches_electron_runs_the_scoped_check() {
    let (done, marker, _log) = run_with_diff_in("electron/main.cjs");
    assert_eq!(done["status"], "done", "{done}");
    assert!(marker.exists(), "the scoped command did not run");
    assert!(
        !decisions(&done).iter().any(|d| d.contains("skipped")),
        "{done}"
    );
}
