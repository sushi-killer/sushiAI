//! Black-box integration tests: a final check that already fails on the
//! task's base commit goes to the owner, after the first attempt, instead of
//! being retried. The base runs are background jobs the first attempt does
//! not wait for.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

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

fn start(daemon: &Daemon, repo: &Path, final_check: &str) -> String {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    daemon.request("settings.set", json!({"settings": settings}));
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.to_str().unwrap(),
            "title": "Pre-existing",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [final_check],
            "start": true,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn spawn(scripts: &Path) -> Daemon {
    let script = fake_harness_script(scripts, "fake.sh", FAKE);
    Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())])
}

fn waiting_with_attempts(daemon: &Daemon, id: &str, n: usize) -> Value {
    poll_until(daemon, id, |s| s == "waiting");
    wait_until(&format!("task {id} to wait after {n} attempts"), || {
        let task = daemon.request("task.get", json!({"id": id}));
        (task["status"] == "waiting" && task["attempts"].as_array().unwrap().len() == n)
            .then_some(task)
    })
}

fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |t| t.lines().count())
}

#[test]
fn a_final_check_that_fails_twice_on_base_is_asked_after_the_first_attempt_and_leaves_no_scratch_dir(
) {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("ran.log");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    // The worktree prints one more line than the base: not the base's failure alone.
    let check = format!(
        "echo ran >> {}; echo boom; [ -f CHANGED_MARKER.txt ] && echo changed; exit 3",
        log.display()
    );
    let id = start(&daemon, repo.path(), &check);

    let task = waiting_with_attempts(&daemon, &id, 1);
    assert_eq!(task["question"]["kind"], "preexisting_failure", "{task}");
    assert_eq!(task["attempts"][0]["failure"]["kind"], "verify", "{task}");
    let base = task["baseSha"].as_str().unwrap();
    assert_eq!(
        task["question"]["text"].as_str().unwrap(),
        format!("{check} already fails on base {}: boom", &base[..7])
    );
    assert_eq!(
        task["question"]["options"],
        json!([RETRY, "drop all", "stop"])
    );
    assert_eq!(
        lines(&log),
        3,
        "the base run, its one re-run and the run on the worktree"
    );

    let listed = git_out(repo.path(), &["worktree", "list"]);
    assert_eq!(listed.lines().count(), 2, "{listed}");
    let scratch = daemon.data_dir().join("base-runs");
    let left = std::fs::read_dir(&scratch).map_or(0, |d| d.count());
    assert_eq!(left, 0, "the temp base dir is gone");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn two_finals_failing_on_base_ask_one_question_after_the_first_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let mut daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let (first, second) = (
        "echo one; [ -f CHANGED_MARKER.txt ] && echo changed; exit 3",
        "echo two; [ -f CHANGED_MARKER.txt ] && echo changed; exit 4",
    );
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    daemon.request("settings.set", json!({"settings": settings}));
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Two finals",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [first, second],
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();

    let task = waiting_with_attempts(&daemon, &id, 1);
    let implement = task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .count();
    assert_eq!(implement, 1, "{task}");
    assert_eq!(
        task["question"]["commands"],
        json!([first, second]),
        "{task}"
    );
    assert_eq!(
        task["question"]["options"],
        json!([RETRY, "drop all", "stop"])
    );
    let text = task["question"]["text"].as_str().unwrap();
    assert!(text.contains(first) && text.contains(second), "{text}");
    let history = task["questionHistory"].as_array().map_or(0, Vec::len);
    assert_eq!(history, 0, "{task}");

    let script = scripts.path().join("fake.sh");
    daemon.restart(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let resumed = waiting_with_attempts(&daemon, &id, 1);
    assert_eq!(resumed["question"]["commands"], json!([first, second]));
    assert_eq!(resumed["question"]["kind"], "preexisting_failure");
    assert_eq!(
        resumed["questionHistory"].as_array().map_or(0, Vec::len),
        0,
        "{resumed}"
    );
    daemon.request("task.answer", json!({"id": id, "answer": "drop all"}));
    let done = poll_until(&daemon, &id, |s| s == "done");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 2, "{done}");
    assert!(
        done["finalVerify"].as_array().is_none_or(Vec::is_empty),
        "{done}"
    );
    let attempts = done["attempts"].as_array().unwrap();
    assert!(attempts.last().unwrap()["failure"].is_null(), "{done}");
    let decisions = done["decisions"].as_array().unwrap();
    for check in [first, second] {
        let dropped = format!("Orchestrator: dropped check {check}");
        assert_eq!(
            decisions
                .iter()
                .filter(|d| d.as_str() == Some(dropped.as_str()))
                .count(),
            1,
            "{decisions:?}"
        );
        let recorded = format!(
            "Owner: base check {check} on {} -> drop all",
            &task["baseSha"].as_str().unwrap()[..7]
        );
        assert_eq!(
            decisions
                .iter()
                .filter(|d| d.as_str() == Some(recorded.as_str()))
                .count(),
            1,
            "{decisions:?}"
        );
    }
    assert_eq!(
        decisions
            .iter()
            .filter(|d| d.as_str() == Some("Owner: drop all"))
            .count(),
        1,
        "{decisions:?}"
    );
    assert_eq!(
        done["questionHistory"].as_array().unwrap().len(),
        1,
        "{done}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_final_whose_missing_target_the_plan_creates_is_an_ordinary_failure_not_a_question() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    daemon.request("settings.set", json!({"settings": settings}));
    // The fake implementer never creates PLANNED.txt: the final fails on the
    // worktree and on the base, but its target is absent on the base and
    // lies under the task's paths, so the plan explains the failure.
    let check = "test -f PLANNED.txt";
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Planned file",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [check],
            "paths": ["PLANNED.txt"],
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let done = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert!(
        done["question"]["kind"] != "preexisting_failure",
        "no pre-existing question: {done}"
    );
    // The failure is the attempt's own: it is retried, not asked about.
    assert!(done["attempts"].as_array().unwrap().len() > 1, "{done}");
    assert_eq!(done["attempts"][0]["failure"]["kind"], "verify", "{done}");
    assert!(
        done["questionHistory"]
            .as_array()
            .is_none_or(|h| h.iter().all(|q| q["kind"] != "preexisting_failure")),
        "{done}"
    );
    let worktree = done["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_check_that_passes_on_base_still_retries() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    // Fails only where the implementer's marker exists.
    let id = start(&daemon, repo.path(), "test ! -f CHANGED_MARKER.txt");
    let task = poll_until(&daemon, &id, |s| matches!(s, "waiting" | "failed" | "done"));
    assert!(
        task["attempts"].as_array().unwrap().len() > 1,
        "retried as before: {task}"
    );
    assert!(!task["question"]["text"]
        .as_str()
        .unwrap_or("")
        .contains("already fails on base"));
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn drop_all_removes_the_check_and_the_first_attempt_lands() {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("ran.log");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let check = format!("echo ran >> {}; false", log.display());
    let id = start(&daemon, repo.path(), &check);
    waiting_with_attempts(&daemon, &id, 1);

    daemon.request("task.answer", json!({"id": id, "answer": "drop all"}));
    let done = poll_until(&daemon, &id, |s| s == "done");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    // An empty list is left out of the JSON.
    assert!(
        done["finalVerify"].as_array().is_none_or(Vec::is_empty),
        "{done}"
    );
    assert!(done["attempts"][0]["failure"].is_null(), "{done}");
    let decisions = done["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(
            |d| d.as_str().unwrap().contains(&check) && d.as_str().unwrap().contains("dropped")
        ),
        "{decisions:?}"
    );
    let branch = done["branch"].as_str().unwrap();
    let subject = git_out(repo.path(), &["log", "-1", "--format=%s", branch]);
    assert!(subject.contains("Pre-existing"), "{subject}");
    let listed = git_out(repo.path(), &["worktree", "list"]);
    assert_eq!(listed.lines().count(), 1, "{listed}");

    daemon.shutdown_and_wait();
}

#[test]
fn drop_all_never_lands_the_attempt_without_a_review_verdict() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let id = start(&daemon, repo.path(), "false");
    waiting_with_attempts(&daemon, &id, 1);

    daemon.request("task.answer", json!({"id": id, "answer": "drop all"}));
    let done = poll_until(&daemon, &id, |s| s == "done");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    assert_eq!(done["attempts"][0]["review"]["verdict"], "PASS", "{done}");
    assert_eq!(done["attempts"][0]["status"], "passed", "{done}");

    daemon.shutdown_and_wait();
}

#[test]
fn retry_after_the_base_is_fixed_moves_to_the_new_base_and_runs_one_more_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let id = start(&daemon, repo.path(), "test -f FIXED.txt");
    let waiting = waiting_with_attempts(&daemon, &id, 1);
    let old_base = waiting["baseSha"].as_str().unwrap().to_string();

    std::fs::write(repo.path().join("FIXED.txt"), "fixed\n").unwrap();
    git_out(repo.path(), &["add", "FIXED.txt"]);
    git_out(repo.path(), &["commit", "-q", "-m", "fix the base"]);
    daemon.request("task.answer", json!({"id": id, "answer": RETRY}));

    let done = poll_until(&daemon, &id, |s| s == "done");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 2, "{done}");
    assert_ne!(done["baseSha"].as_str().unwrap(), old_base);
    assert!(
        done["decisions"].as_array().unwrap().iter().any(|d| d
            .as_str()
            .unwrap()
            .starts_with("Rebase: carried the work onto")),
        "{done}"
    );
    let worktree = done["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_final_the_work_turns_green_asks_nothing_and_stays() {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("ran.log");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    // Fails on the base, passes once the implementer's marker exists.
    let check = format!("echo ran >> {}; test -f CHANGED_MARKER.txt", log.display());
    let id = start(&daemon, repo.path(), &check);
    let done = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    assert_eq!(done["finalVerify"][0], check.as_str(), "{done}");
    assert!(done["question"].is_null(), "{done}");
    let worktree = done["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn an_owner_retry_waits_for_the_base_to_move_keeps_the_check_and_runs_a_second_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let count = scripts.path().join("count");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    // Fails on its first three runs (the base, its re-run, the worktree after
    // attempt 1), passes on the fourth.
    let check = format!(
        "n=$(cat {c} 2>/dev/null || echo 0); echo $((n+1)) > {c}; [ \"$n\" -ge 3 ]",
        c = count.display()
    );
    let id = start(&daemon, repo.path(), &check);
    let waiting = waiting_with_attempts(&daemon, &id, 1);
    let base_sha = waiting["baseSha"].as_str().unwrap().to_string();

    // Nothing moved on the base: no attempt starts until it does.
    daemon.request("task.answer", json!({"id": id, "answer": RETRY}));
    std::thread::sleep(std::time::Duration::from_secs(3));
    let idle = daemon.request("task.get", json!({"id": id}));
    assert_eq!(idle["attempts"].as_array().unwrap().len(), 1, "{idle}");
    assert_eq!(idle["finalVerify"][0], check.as_str(), "{idle}");
    assert_eq!(idle["baseSha"], base_sha.as_str(), "{idle}");
    assert!(idle["question"].is_null(), "{idle}");
    std::fs::write(repo.path().join("OTHER.txt"), "other\n").unwrap();
    git_out(repo.path(), &["add", "OTHER.txt"]);
    git_out(repo.path(), &["commit", "-q", "-m", "move the base"]);
    let done = poll_until(&daemon, &id, |s| s == "done");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 2, "{done}");
    assert_eq!(done["finalVerify"][0], check.as_str(), "{done}");
    let decisions: Vec<&str> = done["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    let retries = decisions
        .iter()
        .filter(|d| **d == format!("Owner: {RETRY}"))
        .count();
    assert_eq!(retries, 1, "{decisions:?}");
    let line = format!("Owner: base check {check} on {} -> {RETRY}", &base_sha[..7]);
    assert!(decisions.contains(&line.as_str()), "{decisions:?}");

    let worktree = done["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_slow_final_on_base_does_not_delay_the_first_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("ci.log");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    // A synthetic `npm run ci`: it logs a start line, sleeps twice the deadline,
    // then fails. The attempt must start within one deadline, so well before it ends.
    let manifest = format!(
        "{{\"name\":\"synthetic\",\"version\":\"1.0.0\",\"scripts\":{{\"ci\":\"echo start >> {}; sleep {}; exit 1\"}}}}\n",
        log.display(),
        deadline().as_secs() * 2
    );
    std::fs::write(repo.path().join("package.json"), manifest).unwrap();
    git_out(repo.path(), &["add", "package.json"]);
    git_out(repo.path(), &["commit", "-q", "-m", "add the ci script"]);
    // No brief check: the clock starts at the task's creation.
    let mut settings = daemon.request("settings.get", json!({}));
    settings["briefCheckRoute"] = json!("");
    daemon.request("settings.set", json!({"settings": settings}));
    let id = start(&daemon, repo.path(), "npm run ci");

    let (task, attempt_started) = wait_until("the implement attempt to start", || {
        let task = daemon.request("task.get", json!({"id": id}));
        let implement = task["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["stage"] == "implement")
            .cloned();
        implement.map(|a| (task, a))
    });
    // The attempt starts within one deadline, long before the final could have ended.
    let from = task["createdAt"].as_u64().unwrap();
    let started = attempt_started["startedAt"].as_u64().unwrap();
    let budget_ms = deadline().as_millis() as u64;
    assert!(
        started.saturating_sub(from) < budget_ms,
        "attempt 1 started {} ms after the brief check: {task}",
        started.saturating_sub(from)
    );

    // Stop the task; the daemon's shutdown ends the background job.
    daemon.request("task.stop", json!({"id": id}));
    let worktree = task["worktree"].as_str().unwrap().to_string();
    let data = daemon.data_dir().to_path_buf();
    daemon.shutdown_and_wait();
    let left = std::fs::read_dir(data.join("base-runs")).map_or(0, |d| d.count());
    assert_eq!(left, 0, "no base-runs scratch dir is left");
    remove_dir_retrying(worktree);
}

#[test]
fn a_second_task_on_the_same_base_runs_no_base_command() {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("ran.log");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    // Fails everywhere (base and worktree), so both tasks look the pair up.
    let check = format!("echo ran >> {}; exit 3", log.display());
    let first = start(&daemon, repo.path(), &check);
    let waiting = waiting_with_attempts(&daemon, &first, 1);
    let base = waiting["baseSha"].as_str().unwrap().to_string();
    let after_first = lines(&log);
    assert_eq!(after_first, 3, "base run, re-run, worktree run");

    let second = start(&daemon, repo.path(), &check);
    let task = waiting_with_attempts(&daemon, &second, 1);
    assert_eq!(task["baseSha"].as_str().unwrap(), base, "{task}");
    // Only the second task's own run on its worktree adds a line.
    assert_eq!(lines(&log), after_first + 1, "no base command ran again");
    let dir = daemon.data_dir().join("tasks").join(&second);
    assert!(!dir.join("preflight-base").exists());
    let runs = dir.join("runs");
    let found = std::fs::read_dir(&runs)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.path().join("final-base").exists() || e.path().join("preflight-base").exists());
    assert!(!found, "no base folder in the second task's runs");

    daemon.shutdown_and_wait();
    for t in [waiting, task] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn an_answer_matches_its_option_whatever_its_case_or_trailing_punctuation() {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("ran.log");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let check = format!("echo ran >> {}; false", log.display());
    let id = start(&daemon, repo.path(), &check);
    waiting_with_attempts(&daemon, &id, 1);

    daemon.request("task.answer", json!({"id": id, "answer": "Drop all."}));
    let done = poll_until(&daemon, &id, |s| s == "done");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    assert!(
        done["finalVerify"].as_array().is_none_or(Vec::is_empty),
        "{done}"
    );
    let dropped = format!("Orchestrator: dropped check {check}");
    let decisions = done["decisions"].as_array().unwrap();
    assert!(
        decisions
            .iter()
            .any(|d| d.as_str() == Some(dropped.as_str())),
        "{decisions:?}"
    );
    let base_sha = done["baseSha"].as_str().unwrap();
    assert!(
        decisions.iter().any(|d| {
            let d = d.as_str().unwrap();
            d.contains(&check) && d.contains(&base_sha[..7]) && d.starts_with("Owner: base check")
        }),
        "{decisions:?}"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn a_drop_answered_after_a_restart_sticks_for_a_single_task() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake.sh", FAKE);
    let env = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let home = data_holder.path().to_path_buf();
    let log = scripts.path().join("ran.log");
    let check = format!("echo ran >> {}; false", log.display());

    let socket1 = socket_of(&home);
    let first = spawn_daemon(&home, &env);
    wait_for_socket(&socket1);
    let mut settings = request_on(&socket1, "settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    settings["briefCheckRoute"] = json!("");
    settings["planReview"] = serde_json::json!("");
    settings["answerPolicy"] = json!(false);
    fit_sandbox(&mut settings);
    request_on(&socket1, "settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let task = request_on(
        &socket1,
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Pre-existing",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [check],
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let wait_waiting = |socket: &Path| {
        wait_until("the task to wait", || {
            let t = request_on(socket, "task.get", json!({"id": id}));
            (t["status"] == "waiting").then_some(t)
        })
    };
    let waiting = wait_waiting(&socket1);
    assert_eq!(
        waiting["question"]["kind"], "preexisting_failure",
        "{waiting}"
    );
    let base_sha = waiting["baseSha"].as_str().unwrap().to_string();
    stop_daemon(&socket1);
    let _ = wait_for_exit(first);

    let socket2 = socket_of(&home);
    let second = spawn_daemon(&home, &env);
    wait_for_socket(&socket2);
    let still = wait_waiting(&socket2);
    assert_eq!(still["question"]["kind"], "preexisting_failure", "{still}");
    request_on(
        &socket2,
        "task.answer",
        json!({"id": id, "answer": "drop all"}),
    );
    let done = wait_until("the task to end", || {
        let t = request_on(&socket2, "task.get", json!({"id": id}));
        matches!(t["status"].as_str(), Some("done" | "failed")).then_some(t)
    });
    assert_eq!(done["status"], "done", "{done}");
    // The question came after attempt 1; the restarted loop has no live
    // attempt to resume, so the answer is followed by one more, which lands.
    assert_eq!(done["attempts"].as_array().unwrap().len(), 2, "{done}");
    for field in ["verify", "finalVerify"] {
        let list = done[field].as_array();
        assert!(
            list.is_none_or(|l| l.iter().all(|c| c.as_str() != Some(check.as_str()))),
            "{done}"
        );
    }
    let decisions: Vec<&str> = done["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert!(
        decisions.contains(&format!("Orchestrator: dropped check {check}").as_str()),
        "{decisions:?}"
    );
    let line = format!(
        "Owner: base check {check} on {} -> drop all",
        &base_sha[..7]
    );
    assert!(decisions.contains(&line.as_str()), "{decisions:?}");
    let drops = decisions
        .iter()
        .filter(|d| **d == "Owner: drop all")
        .count();
    assert_eq!(drops, 1, "{decisions:?}");

    stop_daemon(&socket2);
    let _ = wait_for_exit(second);
    remove_dir_retrying(task["worktree"].as_str().unwrap());
}

#[test]
fn a_check_that_fails_once_on_the_base_and_then_passes_is_not_asked_about() {
    let scripts = tempfile::tempdir().unwrap();
    let count = scripts.path().join("count");
    let release = scripts.path().join("release-base-rerun");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    // The worktree passes while the base's second run waits for the test.
    let check = format!(
        "if [ -f CHANGED_MARKER.txt ]; then exit 0; fi; n=$(cat {c} 2>/dev/null || echo 0); echo $((n+1)) > {c}; [ \"$n\" -ge 1 ] || exit 1; while [ ! -f {release} ]; do sleep 0.05; done",
        c = count.display(),
        release = release.display()
    );
    let id = start(&daemon, repo.path(), &check);
    let done = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    assert!(done["question"].is_null(), "{done}");
    let base = done["baseSha"].as_str().unwrap();
    let flaky = format!(
        "Orchestrator: base check {check} flaky on {}: passed on rerun",
        &base[..8]
    );
    wait_until("the background base rerun to start", || {
        (std::fs::read_to_string(&count).unwrap_or_default().trim() == "2").then_some(())
    });
    std::fs::write(release, "go").unwrap();
    let done = wait_until("the late flaky base-check decision", || {
        let task = daemon.request("task.get", json!({"id": id}));
        task["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d == &json!(flaky))
            .then_some(task)
    });
    assert!(
        done["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d == &json!(flaky)),
        "{done}"
    );
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    assert!(done["question"].is_null(), "{done}");
    let worktree = done["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn stop_leaves_the_task_stopped() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let id = start(&daemon, repo.path(), "false");
    let waiting = waiting_with_attempts(&daemon, &id, 1);
    daemon.request("task.answer", json!({"id": id, "answer": "stop"}));
    let stopped = daemon.request("task.get", json!({"id": id}));
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    let worktree = waiting["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
