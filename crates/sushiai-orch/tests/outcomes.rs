//! Black-box outcome fixtures: one test per typed task outcome. A fake harness
//! answers every agent turn with canned output (no model call), and each test
//! asserts the final status plus the one decision, question or notice that
//! defines the outcome. These tests are the safety net for code that decides
//! outcomes: they must keep passing when that code is moved or deleted.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// One claude `result` line that carries `text` as its answer.
fn reply(text: &str) -> String {
    let line = json!({"type": "result", "total_cost_usd": 0.01,
        "usage": {"input_tokens": 1, "output_tokens": 1}, "result": text});
    format!("printf '%s\\n' '{line}'")
}

fn report(outcome: &str, question: &str) -> String {
    let body =
        json!({"outcome": outcome, "summary": "done", "decisions": [], "question": question});
    format!("```sushi-report\n{body}\n```")
}

fn review_verdict(verdict: &str, findings: Value) -> String {
    let body = json!({"verdict": verdict, "findings": findings});
    reply(&format!("```sushi-review\n{body}\n```"))
}

/// One implement run: the shell `work` (it edits the worktree), then its
/// session line and a `sushi-report` with the given outcome.
fn implement(work: &str, outcome: &str, question: &str) -> String {
    let init = json!({"type": "system", "subtype": "init", "session_id": "sess-fake"});
    format!(
        "{work}; printf '%s\\n' '{init}'; {}",
        reply(&report(outcome, question))
    )
}

const MARK: &str = "echo changed > CHANGED_MARKER.txt";

/// The fake harness. A review brief (it opens with "## Review") runs `review`,
/// a pull request draft brief gets an empty draft, an advisor brief (a failed
/// attempt) gets plain advice, any other run runs `implement`. Both are shell
/// snippets that may read `$LOG_DIR`.
fn fake(dir: &Path, review: &str, implement: &str) -> PathBuf {
    let advice = reply("Try the other approach.");
    let draft = reply(r#"{"branch":"","title":"","body":""}"#);
    let body = format!(
        "#!/bin/sh\nbrief=$(cat)\ncase \"$brief\" in\n\"## Review\"*) {review} ;;\n\"## Pull request draft\"*) {draft} ;;\n*\"An implement attempt at this task failed\"*) {advice} ;;\n*) {implement} ;;\nesac\n"
    );
    fake_harness_script(dir, "fake-claude.sh", &body)
}

fn spawn(scripts: &Path, review: &str, implement: &str, env: &[(&str, &str)]) -> Daemon {
    let harness = fake(scripts, review, implement);
    let log = scripts.to_str().unwrap();
    let mut vars = vec![
        ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
        ("LOG_DIR", log),
    ];
    vars.extend_from_slice(env);
    Daemon::spawn(&vars)
}

/// Sets the review route and any other settings for this daemon.
fn configure(daemon: &Daemon, review: &str, extra: Value) {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!(review);
    for (key, value) in extra.as_object().unwrap() {
        settings[key] = value.clone();
    }
    daemon.request("settings.set", json!({"settings": settings}));
}

/// Creates and starts a task with a trivial goal; `extra` overrides fields.
fn create(daemon: &Daemon, repo: &Path, extra: Value) -> String {
    let mut params = json!({"repo": repo.to_str().unwrap(), "title": "Outcome",
        "goal": "Write the marker", "criteria": [], "verify": ["true"], "start": true});
    for (key, value) in extra.as_object().unwrap() {
        params[key] = value.clone();
    }
    daemon.request("task.create", params)["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn finish(daemon: Daemon, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

fn attempts(task: &Value) -> &Vec<Value> {
    task["attempts"].as_array().unwrap()
}

#[test]
fn a_task_whose_checks_and_review_pass_is_done_and_lands() {
    let scripts = tempfile::tempdir().unwrap();
    let pass = review_verdict("PASS", json!([]));
    let daemon = spawn(scripts.path(), &pass, &implement(MARK, "complete", ""), &[]);
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    configure(&daemon, "claude-opus", json!({}));
    let id = create(&daemon, repo.path(), json!({"land": true}));

    let task = poll_task_status(&daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(attempts(&task).len(), 1, "{task}");
    assert_eq!(
        task["landedSha"],
        git_out(repo.path(), &["rev-parse", "work"])
    );
    finish(daemon, &task);
}

#[test]
fn a_review_fail_then_pass_is_done_on_the_second_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let fail = review_verdict("FAIL", json!(["needs work"]));
    let pass = review_verdict("PASS", json!([]));
    let review = format!(
        "n=$(cat \"$LOG_DIR/reviews\" 2>/dev/null || echo 0); echo $((n+1)) > \"$LOG_DIR/reviews\"; if [ \"$n\" -eq 0 ]; then {fail}; else {pass}; fi"
    );
    let daemon = spawn(
        scripts.path(),
        &review,
        &implement(MARK, "complete", ""),
        &[],
    );
    let repo = init_git_repo();
    configure(&daemon, "claude-opus", json!({}));
    let id = create(&daemon, repo.path(), json!({}));

    let task = poll_task_status(&daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(attempts(&task).len(), 2, "{task}");
    assert_eq!(attempts(&task)[0]["failure"]["kind"], "review", "{task}");
    assert_eq!(attempts(&task)[1]["status"], "passed", "{task}");
    finish(daemon, &task);
}

#[test]
fn a_check_that_fails_only_on_the_task_is_not_done_on_attempt_one() {
    let scripts = tempfile::tempdir().unwrap();
    // The first run breaks the marker, the second repairs it; the base never
    // has the marker, so the check passes there.
    let work = "n=$(cat \"$LOG_DIR/impls\" 2>/dev/null || echo 0); echo $((n+1)) > \"$LOG_DIR/impls\"; if [ \"$n\" -eq 0 ]; then echo bad; else echo good; fi > CHANGED_MARKER.txt";
    let daemon = spawn(scripts.path(), "", &implement(work, "complete", ""), &[]);
    let repo = init_git_repo();
    configure(&daemon, "", json!({}));
    let check = r#"if [ "$(cat CHANGED_MARKER.txt 2>/dev/null)" = bad ]; then echo 'marker is bad'; exit 1; fi"#;
    let id = create(&daemon, repo.path(), json!({"finalVerify": [check]}));

    let task = poll_task_status(&daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(attempts(&task).len(), 2, "{task}");
    assert_eq!(attempts(&task)[0]["status"], "failed", "{task}");
    assert_eq!(attempts(&task)[0]["failure"]["kind"], "verify", "{task}");
    assert!(task["question"].is_null(), "{task}");
    finish(daemon, &task);
}

#[test]
fn a_check_that_fails_the_same_way_on_the_base_asks_the_owner_about_the_base() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path(), "", &implement(MARK, "complete", ""), &[]);
    let repo = init_git_repo();
    configure(&daemon, "", json!({}));
    // Identical output on the base and on the task, and no runner lines to
    // tell the failures apart: today this is a question, not a waiver.
    let check = "echo 'coverage 12.4% is below the gate'; exit 1";
    let id = create(&daemon, repo.path(), json!({"finalVerify": [check]}));

    let task = poll_task_status(&daemon, &id);
    assert_eq!(task["status"], "waiting", "{task}");
    assert_eq!(task["question"]["kind"], "preexisting_failure", "{task}");
    assert_eq!(attempts(&task).len(), 1, "{task}");
    finish(daemon, &task);
}

#[test]
fn a_blocked_report_waits_for_the_owner_with_an_agent_question() {
    let scripts = tempfile::tempdir().unwrap();
    let blocked = implement(MARK, "blocked", "Which one should it be?");
    let daemon = spawn(scripts.path(), "", &blocked, &[]);
    let repo = init_git_repo();
    configure(&daemon, "", json!({}));
    let id = create(&daemon, repo.path(), json!({}));

    let task = poll_task_status(&daemon, &id);
    assert_eq!(task["status"], "waiting", "{task}");
    assert_eq!(task["question"]["kind"], "agent_question", "{task}");
    assert_eq!(
        task["question"]["text"], "Which one should it be?",
        "{task}"
    );
    // Today the attempt keeps status `failed`; only the failure kind says blocked.
    assert_eq!(attempts(&task)[0]["status"], "failed", "{task}");
    assert_eq!(attempts(&task)[0]["failure"]["kind"], "blocked", "{task}");
    finish(daemon, &task);
}

#[test]
fn a_refused_write_outside_the_worktree_waits_for_one_permission_answer() {
    let scripts = tempfile::tempdir().unwrap();
    // The run finishes with a refused Edit outside its worktree and no change.
    let refused = r#"printf '%s\n' '{"type":"result","total_cost_usd":0.01,"result":"I could not write it.","permission_denials":[{"tool_name":"Edit","tool_use_id":"toolu_1","tool_input":{"file_path":"/elsewhere/secret.rs"}}]}'"#;
    let daemon = spawn(scripts.path(), "", refused, &[]);
    let repo = init_git_repo();
    configure(&daemon, "", json!({}));
    let id = create(&daemon, repo.path(), json!({}));

    let task = poll_task_status(&daemon, &id);
    assert_eq!(task["status"], "waiting", "{task}");
    assert_eq!(task["question"]["kind"], "permission", "{task}");
    assert!(
        task["question"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Allow /elsewhere/secret.rs for this task?"),
        "{task}"
    );
    assert_eq!(
        attempts(&task).len(),
        1,
        "no second attempt before the answer: {task}"
    );
    finish(daemon, &task);
}

#[test]
fn attempts_that_keep_failing_end_failed_with_one_failure_report_notice() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path(), "", &implement(MARK, "complete", ""), &[]);
    let repo = init_git_repo();
    configure(&daemon, "", json!({"maxAttempts": 2}));
    let id = create(&daemon, repo.path(), json!({"verify": ["false"]}));

    let task = poll_task_status(&daemon, &id);
    assert_eq!(task["status"], "failed", "{task}");
    assert_eq!(
        attempts(&task).len(),
        3,
        "maxAttempts=2, then one changed approach: {task}"
    );
    assert!(task["question"].is_null(), "{task}");
    // The daemon names the repo by its resolved path, so ask with the task's own.
    let repo_path = task["repo"].as_str().unwrap().to_string();
    let notices = || {
        let thread = daemon.request("chat.get", json!({"repo": repo_path}));
        thread["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["text"].as_str())
            .filter(|t| t.contains("failed after 3 attempt(s)"))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let found = wait_until("the failure report notice", || {
        let found = notices();
        (!found.is_empty()).then_some(found)
    });
    assert_eq!(found.len(), 1, "{found:?}");
    finish(daemon, &task);
}

#[test]
fn a_review_without_a_verdict_is_committed_unreviewed_and_its_pull_request_says_so() {
    let scripts = tempfile::tempdir().unwrap();
    let gh_log = scripts.path().join("gh.log");
    let gh = fake_harness_script(
        scripts.path(),
        "fake-gh.sh",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\ncase \"$1 $2\" in\n  \"pr list\") exit 0 ;;\n  \"pr create\") echo https://github.example/acme/app/pull/7 ;;\nesac\n",
            gh_log.display()
        ),
    );
    let prose = reply("Looks fine to me.");
    let daemon = spawn(
        scripts.path(),
        &prose,
        &implement(MARK, "complete", ""),
        &[("ORCHD_GH_BIN", gh.to_str().unwrap())],
    );
    let repo = init_git_repo();
    git_out(repo.path(), &["branch", "-M", "main"]);
    let remote = tempfile::tempdir().unwrap();
    git_out(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
    git_out(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git_out(repo.path(), &["push", "-q", "origin", "main"]);
    let repo_path = repo.path().to_str().unwrap();
    configure(
        &daemon,
        "claude-opus",
        json!({"prOnDoneRepos": {repo_path: true}}),
    );
    let id = create(&daemon, repo.path(), json!({}));

    let task = poll_task_status(&daemon, &id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(task["unreviewed"], true, "{task}");
    assert!(task.get("landedSha").is_none(), "{task}");
    assert!(
        task["decisions"]
            .to_string()
            .contains("committed unreviewed"),
        "{task}"
    );
    let log = wait_until("the pull request to be created", || {
        std::fs::read_to_string(&gh_log)
            .ok()
            .filter(|l| l.contains("pr create"))
    });
    assert!(
        log.contains("Unreviewed: no review route gave a verdict"),
        "{log}"
    );
    finish(daemon, &task);
}

#[test]
#[ignore = "the planner helper timeout is fixed at 15 minutes inside the daemon: no setting or env var shortens it, so a black-box fixture cannot reach it. The unit test planner_timed_out_is_tried_once_more_with_twice_the_time_then_fails (src/engine/plan.rs) covers the outcome"]
fn a_planner_that_times_out_is_resumed_once_and_then_fails() {
    panic!("needs a planner timeout a black-box test can set");
}

#[test]
fn a_task_stopped_by_the_owner_ends_stopped_with_no_further_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    // The run starts and then holds until it is stopped; `$LOG_DIR/runs`
    // counts the runs the daemon starts.
    let hold = "echo 1 >> \"$LOG_DIR/runs\"; exec sleep 60";
    let daemon = spawn(scripts.path(), "", hold, &[]);
    let repo = init_git_repo();
    configure(&daemon, "", json!({}));
    let id = create(&daemon, repo.path(), json!({}));
    let runs = scripts.path().join("runs");
    wait_until("the first run to start", || runs.exists().then_some(()));

    daemon.request("task.stop", json!({"id": id}));
    let task = poll_task_status(&daemon, &id);
    assert_eq!(task["status"], "stopped", "{task}");
    assert_eq!(attempts(&task).len(), 1, "{task}");
    assert_eq!(std::fs::read_to_string(&runs).unwrap().lines().count(), 1);
    finish(daemon, &task);
}
