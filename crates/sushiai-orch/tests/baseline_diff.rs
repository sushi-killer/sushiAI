//! Black-box tests of the baseline diff: a final check that fails on the
//! base too is judged by which tests fail, not by its exit code. The fake
//! checks print synthetic runner output (TAP lines).

mod common;

use common::harness::fake_harness_script;
use common::polling::{poll_until, remove_dir_retrying, wait_until};
use common::process::Daemon;
use common::repository::init_git_repo;

use serde_json::{json, Value};
use std::path::Path;

/// Implements (writes a marker) and, as the reviewer, answers a plain PASS.
const FAKE: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
'## Route the review'*) exit 0 ;;
"## Review"*)
  printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*)
  echo changed > CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

const RETRY: &str = "retry after the base is fixed";

fn spawn(scripts: &Path, env: &[(&str, &str)]) -> Daemon {
    let script = fake_harness_script(scripts, "fake.sh", FAKE);
    let mut vars = vec![("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    vars.extend_from_slice(env);
    Daemon::spawn(&vars)
}

fn start(daemon: &Daemon, repo: &Path, final_check: &str) -> String {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    daemon.request("settings.set", json!({"settings": settings}));
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.to_str().unwrap(),
            "title": "Red base",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [final_check],
            "start": true,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn decisions(task: &Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

fn asked_about_the_base(task: &Value) -> bool {
    task["question"]["kind"] == "preexisting_failure"
        || task["questionHistory"]
            .as_array()
            .is_some_and(|h| h.iter().any(|q| q["kind"] == "preexisting_failure"))
}

fn finish(daemon: Daemon, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_task_that_fails_only_the_tests_the_base_fails_passes_with_a_waiver() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path(), &[]);
    let repo = init_git_repo();
    let check = "printf 'ok 1 - fine\\nnot ok 2 - alpha\\nnot ok 3 - beta\\n'; exit 1";
    let id = start(&daemon, repo.path(), check);

    let done = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    assert!(!asked_about_the_base(&done), "{done}");
    // The shared check stays.
    assert_eq!(done["finalVerify"][0], check, "{done}");
    let base = done["baseSha"].as_str().unwrap();
    let line = format!(
        "Orchestrator: base failures on {} accepted: alpha, beta (check: {check})",
        &base[..8]
    );
    assert!(decisions(&done).contains(&line), "{:?}", decisions(&done));
    finish(daemon, &done);
}

#[test]
fn a_new_failing_test_is_an_ordinary_failure_the_agent_fixes() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path(), &[]);
    let repo = init_git_repo();
    // The base fails `alpha`; the implementer's marker adds `beta`.
    let check = "printf 'not ok 1 - alpha\\n'; case \"$PWD\" in */base-runs/*);; *) printf 'not ok 2 - beta\\n';; esac; exit 1";
    let id = start(&daemon, repo.path(), check);

    let task = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert_eq!(task["attempts"][0]["failure"]["kind"], "verify", "{task}");
    assert!(
        task["attempts"][0]["failure"]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("beta"),
        "{task}"
    );
    assert!(task["attempts"].as_array().unwrap().len() > 1, "{task}");
    assert!(!asked_about_the_base(&task), "{task}");
    assert!(
        decisions(&task)
            .iter()
            .all(|d| !d.contains("base failures on")),
        "{:?}",
        decisions(&task)
    );
    finish(daemon, &task);
}

#[test]
fn two_runs_that_each_show_a_new_failure_are_an_ordinary_failure() {
    let scripts = tempfile::tempdir().unwrap();
    let count = scripts.path().join("count");
    let daemon = spawn(scripts.path(), &[]);
    let repo = init_git_repo();
    // Every run on the worktree names a different extra test.
    let check = format!(
        "n=$(cat {c} 2>/dev/null || echo 0); echo $((n+1)) > {c}; printf 'not ok 1 - alpha\\n'; case \"$PWD\" in */base-runs/*);; *) printf 'not ok 9 - flaky%s\\n' \"$n\";; esac; exit 1",
        c = count.display()
    );
    let id = start(&daemon, repo.path(), &check);

    let task = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert_eq!(task["attempts"][0]["failure"]["kind"], "verify", "{task}");
    assert!(task["attempts"].as_array().unwrap().len() > 1, "{task}");
    assert!(!asked_about_the_base(&task), "{task}");
    assert!(
        decisions(&task)
            .iter()
            .all(|d| !d.contains("base failures on")),
        "{:?}",
        decisions(&task)
    );
    finish(daemon, &task);
}

#[test]
fn a_failure_that_does_not_come_back_on_the_rerun_is_flaky_and_waived() {
    let scripts = tempfile::tempdir().unwrap();
    let flag = scripts.path().join("flag");
    let daemon = spawn(scripts.path(), &[]);
    let repo = init_git_repo();
    // The first run on the worktree adds `beta`; the rerun shows the base's set.
    let check = format!(
        "printf 'not ok 1 - alpha\\n'; case \"$PWD\" in */base-runs/*);; *) if [ ! -f {f} ]; then touch {f}; printf 'not ok 2 - beta\\n'; fi;; esac; exit 1",
        f = flag.display()
    );
    let id = start(&daemon, repo.path(), &check);

    let done = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    assert!(
        decisions(&done)
            .iter()
            .any(|d| d.contains("base failures on") && d.contains("differed between reruns")),
        "{:?}",
        decisions(&done)
    );
    finish(daemon, &done);
}

#[test]
fn a_check_the_task_is_meant_to_fix_is_never_waived() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path(), &[]);
    let repo = init_git_repo();
    let check = "printf 'not ok 1 - alpha\\n'; exit 1";
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    daemon.request("settings.set", json!({"settings": settings}));
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Fix the red test",
            "goal": format!("Make this check pass: {check}"),
            "verify": ["true"],
            "finalVerify": [check],
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let task = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert_ne!(task["status"], "done", "{task}");
    assert_eq!(task["attempts"][0]["failure"]["kind"], "verify", "{task}");
    assert!(
        decisions(&task)
            .iter()
            .all(|d| !d.contains("base failures on")),
        "{:?}",
        decisions(&task)
    );
    finish(daemon, &task);
}

#[test]
fn an_unrelated_task_that_edits_the_file_of_the_failing_test_is_still_waived() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path(), &[]);
    let repo = init_git_repo();
    // The failing test lives in the file the task writes, but nothing in the
    // request names the check or the test.
    let check = "printf 'ok 1 - fine\\nnot ok 2 - CHANGED_MARKER.txt::alpha\\n'; exit 1";
    let id = start(&daemon, repo.path(), check);

    let done = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert_eq!(done["status"], "done", "{done}");
    assert!(
        decisions(&done)
            .iter()
            .any(|d| d.contains("base failures on") && d.contains("CHANGED_MARKER.txt::alpha")),
        "{:?}",
        decisions(&done)
    );
    assert!(
        decisions(&done).iter().all(|d| !d.contains("kept check")),
        "{:?}",
        decisions(&done)
    );
    finish(daemon, &done);
}

#[test]
fn a_gate_with_no_test_lines_is_not_waived_on_an_equal_fingerprint() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path(), &[]);
    let repo = init_git_repo();
    let check = "case \"$PWD\" in */base-runs/*) echo 'coverage 85.2% is below the gate';; *) echo 'coverage 12.4% is below the gate';; esac; exit 1";
    let id = start(&daemon, repo.path(), check);
    poll_until(&daemon, &id, |s| s == "waiting");
    let task = daemon.request("task.get", json!({"id": id}));
    assert_eq!(task["question"]["kind"], "preexisting_failure", "{task}");
    finish(daemon, &task);
}

#[test]
fn outputs_that_no_runner_explains_and_that_differ_still_ask_the_owner() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path(), &[]);
    let repo = init_git_repo();
    let check = "case \"$PWD\" in */base-runs/*) echo 'strange base output';; *) echo 'strange task output';; esac; exit 1";
    let id = start(&daemon, repo.path(), check);

    poll_until(&daemon, &id, |s| s == "waiting");
    let task = daemon.request("task.get", json!({"id": id}));
    assert_eq!(task["question"]["kind"], "preexisting_failure", "{task}");
    finish(daemon, &task);
}

#[test]
fn an_owner_retry_waits_a_bounded_time_for_the_base_and_then_asks_again() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path(), &[("ORCHD_BASE_WAIT_SECS", "3")]);
    let repo = init_git_repo();
    let check = "case \"$PWD\" in */base-runs/*) echo 'strange base output';; *) echo 'strange task output';; esac; exit 1";
    let id = start(&daemon, repo.path(), check);
    poll_until(&daemon, &id, |s| s == "waiting");

    daemon.request("task.answer", json!({"id": id, "answer": RETRY}));
    // While it waits it shows why, as waiting work, not as a silent run.
    let waiting = wait_until("the base wait to show its reason", || {
        let task = daemon.request("task.get", json!({"id": id}));
        (task["status"] == "queued" && task["wait"]["kind"] == "dependency").then_some(task)
    });
    assert!(
        waiting["wait"]["titles"][0]
            .as_str()
            .unwrap()
            .starts_with("a new commit on "),
        "{waiting}"
    );
    // Nothing moves the base: the wait ends by itself and the check is asked again.
    let asked = wait_until("the wait to run out", || {
        let task = daemon.request("task.get", json!({"id": id}));
        decisions(&task)
            .iter()
            .any(|d| d.contains("did not move past"))
            .then_some(task)
    });
    assert!(
        decisions(&asked)
            .iter()
            .any(|d| d.contains("waited out base")),
        "{:?}",
        decisions(&asked)
    );
    let again = poll_until(&daemon, &id, |s| s == "waiting");
    assert_eq!(again["question"]["kind"], "preexisting_failure", "{again}");
    finish(daemon, &again);
}
