//! Black-box integration tests: a retry after "the base is fixed" waits for
//! the base branch to move, base results survive a daemon restart (a timeout
//! is never stored), and a final check that only timed out is rerun once on
//! the kept worktree without a new implement session.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// Implements (writes a marker) and, as the reviewer, answers a plain PASS.
const FAKE: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Review"*)
  printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*)
  echo changed > CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

const RETRY: &str = "retry after the base is fixed";

fn create(daemon: &Daemon, repo: &Path, final_check: &str) -> String {
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.to_str().unwrap(),
            "title": "Base retry",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [final_check],
            "start": true,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn prepare(daemon: &Daemon, edit: impl FnOnce(&mut Value)) {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    edit(&mut settings);
    daemon.request("settings.set", json!({"settings": settings}));
}

fn waiting_with_attempts(daemon: &Daemon, id: &str, n: usize) -> Value {
    wait_until(&format!("task {id} to wait after {n} attempts"), || {
        let task = daemon.request("task.get", json!({"id": id}));
        (task["status"] == "waiting" && task["attempts"].as_array().unwrap().len() == n)
            .then_some(task)
    })
}

fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |t| t.lines().count())
}

fn commit_file(repo: &Path, name: &str) {
    std::fs::write(repo.join(name), "x\n").unwrap();
    git_out(repo, &["add", name]);
    git_out(repo, &["commit", "-q", "-m", &format!("add {name}")]);
}

#[test]
fn a_retry_starts_no_attempt_until_the_base_branch_moves() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    prepare(&daemon, |_| {});
    let repo = init_git_repo();
    let id = create(&daemon, repo.path(), "test -f FIXED.txt");
    let waiting = waiting_with_attempts(&daemon, &id, 1);
    let old_base = waiting["baseSha"].as_str().unwrap().to_string();

    daemon.request("task.answer", json!({"id": id, "answer": RETRY}));
    std::thread::sleep(Duration::from_secs(3));
    let idle = daemon.request("task.get", json!({"id": id}));
    assert_eq!(idle["attempts"].as_array().unwrap().len(), 1, "{idle}");
    assert_eq!(idle["baseSha"].as_str().unwrap(), old_base, "{idle}");
    assert!(
        idle["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d.as_str().unwrap().contains("waiting for")),
        "{idle}"
    );

    commit_file(repo.path(), "FIXED.txt");
    let done = poll_until(&daemon, &id, |s| s == "done");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 2, "{done}");
    assert_ne!(done["baseSha"].as_str().unwrap(), old_base, "{done}");
    let worktree = done["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn base_results_survive_a_restart_and_the_command_is_not_run_again() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let env = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let home = data_holder.path().to_path_buf();
    let log = scripts.path().join("ran.log");
    let check = format!("echo ran >> {}; false", log.display());
    let repo = init_git_repo();

    let run = |socket: &Path| -> (String, String) {
        let mut settings = request_on(socket, "settings.get", json!({}));
        settings["review"] = json!("claude-opus");
        settings["briefCheckRoute"] = json!("");
        settings["answerPolicy"] = json!(false);
        fit_sandbox(&mut settings);
        request_on(socket, "settings.set", json!({"settings": settings}));
        let task = request_on(
            socket,
            "task.create",
            json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "Base retry",
                "goal": "Write the marker",
                "verify": ["true"],
                "finalVerify": [check],
                "start": true,
            }),
        );
        let id = task["id"].as_str().unwrap().to_string();
        let waiting = wait_until("the task to ask", || {
            let t = request_on(socket, "task.get", json!({"id": id}));
            (t["status"] == "waiting" && t["question"]["kind"] == "preexisting_failure")
                .then_some(t)
        });
        (id, waiting["worktree"].as_str().unwrap().to_string())
    };

    let socket1 = socket_of(&home);
    let first = spawn_daemon(&home, &env);
    wait_for_socket(&socket1);
    let (_, wt1) = run(&socket1);
    // The base (twice: first run and rerun) and the worktree after attempt 1.
    assert_eq!(lines(&log), 3);
    stop_daemon(&socket1);
    let _ = wait_for_exit(first);
    assert!(data_file_exists(&home));

    let socket2 = socket_of(&home);
    let second = spawn_daemon(&home, &env);
    wait_for_socket(&socket2);
    let (_, wt2) = run(&socket2);
    // Only the worktree ran it: the base answer came from disk.
    assert_eq!(lines(&log), 4);
    stop_daemon(&socket2);
    let _ = wait_for_exit(second);
    remove_dir_retrying(wt1);
    remove_dir_retrying(wt2);
}

/// The data dir of a daemon started on `home` holds the base results file.
fn data_file_exists(home: &Path) -> bool {
    fn walk(dir: &Path) -> bool {
        std::fs::read_dir(dir).is_ok_and(|rd| {
            rd.flatten().any(|e| {
                let p = e.path();
                p.file_name().is_some_and(|n| n == "base-results.json") || (p.is_dir() && walk(&p))
            })
        })
    }
    walk(home)
}

#[test]
fn a_final_that_only_timed_out_is_rerun_once_without_a_new_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    prepare(&daemon, |s| s["verifyTimeoutSecs"] = json!(2));
    let repo = init_git_repo();
    let flag = scripts.path().join("slept");
    let runs = scripts.path().join("runs.log");
    // Passes on the base (no marker); on the worktree the first run hangs
    // past the timeout, the rerun passes.
    let check = format!(
        "if [ -f CHANGED_MARKER.txt ]; then echo ran >> {runs}; if [ ! -f {f} ]; then touch {f}; sleep 60; fi; fi; true",
        f = flag.display(),
        runs = runs.display()
    );
    let id = create(&daemon, repo.path(), &check);
    let done = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    assert_eq!(lines(&runs), 2, "one initial worktree run and one retry");
    assert!(done["question"].is_null(), "{done}");
    // Real machine load selects either the final-check retry or the runner's
    // load-aware retry. Both must record the command and environment reason.
    assert_eq!(
        done["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| {
                let line = d.as_str().unwrap();
                line.contains(&check)
                    && line.contains("timed out")
                    && line.contains("(environment)")
                    && line.contains("rerun once on the same tree")
            })
            .count(),
        1,
        "{done}"
    );
    let worktree = done["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
