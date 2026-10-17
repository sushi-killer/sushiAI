//! The fail-fast branch of the baseline diff against a real tiny cargo
//! project (no dependencies, offline) with two test binaries: the lib, where
//! `tests::x` fails on the base, and `tests/b.rs`. A plain `cargo test` stops
//! after the lib, so orchd re-runs both sides with `--no-fail-fast`.

mod common;

use common::harness::fake_harness_script;
use common::polling::{poll_until, remove_dir_retrying};
use common::process::Daemon;
use common::repository::{git_out, init_git_repo};

use serde_json::{json, Value};
use std::path::Path;

/// Implements (writes a marker; a goal with BREAK_B also breaks tests/b.rs)
/// and, as the reviewer, answers a plain PASS.
const FAKE: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
'## Route the review'*) exit 0 ;;
"## Review"*)
  printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*)
  echo changed > CHANGED_MARKER.txt
  case "$brief" in
  *BREAK_B*) printf '#[test]\nfn bt() { assert_eq!(1, 2); }\n' > tests/b.rs ;;
  esac
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

fn cargo_available() -> bool {
    std::process::Command::new("cargo")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn git(repo: &Path, args: &[&str]) {
    git_out(repo, args);
}

/// A repo whose lib test `x` fails and whose `tests/b.rs` passes.
fn cargo_repo() -> tempfile::TempDir {
    let repo = init_git_repo();
    let dir = repo.path();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("tests")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"tiny\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/lib.rs"),
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn x() { assert_eq!(2, 3); }\n    #[test]\n    fn y() {}\n}\n",
    )
    .unwrap();
    std::fs::write(dir.join("tests/b.rs"), "#[test]\nfn bt() {}\n").unwrap();
    std::fs::write(dir.join(".gitignore"), "target/\nCargo.lock\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "tiny cargo project"]);
    repo
}

fn run(goal: &str) -> Option<(Daemon, Value)> {
    if !cargo_available() {
        eprintln!("SKIP baseline_cargo: cargo is not on PATH");
        return None;
    }
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    // No shared CARGO_TARGET_DIR: the checkouts have one fingerprint, so a
    // shared directory would hand the base the task's compiled test binary.
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = cargo_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Tiny",
            "goal": goal,
            "verify": ["true"],
            "finalVerify": ["cargo test --offline"],
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let task = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    // Keep the repo alive until the task settled.
    drop((scripts, repo));
    Some((daemon, task))
}

fn decisions(task: &Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

fn finish(daemon: Daemon, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_task_that_breaks_a_later_test_binary_is_an_ordinary_failure() {
    let Some((daemon, task)) = run("Write the marker BREAK_B") else {
        return;
    };
    assert_eq!(task["attempts"][0]["failure"]["kind"], "verify", "{task}");
    assert!(
        task["attempts"][0]["failure"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("bt"),
        "{task}"
    );
    assert!(
        decisions(&task)
            .iter()
            .all(|d| !d.contains("base failures on")),
        "{:?}",
        decisions(&task)
    );
    finish(daemon, &task);
}

// ci-platform: macos-only (failed on macOS while ubuntu passed, CI run 38931522638)
#[test]
fn a_task_that_breaks_nothing_new_is_waived_after_the_full_rerun() {
    let Some((daemon, task)) = run("Write the marker") else {
        return;
    };
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(task["attempts"].as_array().unwrap().len(), 1, "{task}");
    assert!(
        decisions(&task)
            .iter()
            .any(|d| d.contains("base failures on")
                && d.contains("tests::x")
                && d.contains("--no-fail-fast")),
        "{:?}",
        decisions(&task)
    );
    finish(daemon, &task);
}
