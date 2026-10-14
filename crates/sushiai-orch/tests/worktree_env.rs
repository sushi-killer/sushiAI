//! Black-box integration tests: a committed `.sushiai/setup.json` prepares every
//! new task worktree (and base-check worktree) before anything runs in it.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

/// Implements (writes a marker) and reports complete; the review is off.
const PASS: &str = "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n";

fn spawn(body: &str) -> (Daemon, tempfile::TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", body);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("ORCHD_SETUP_RETRY_MS", "20"),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    daemon.request("settings.set", json!({"settings": settings}));
    (daemon, scripts)
}

/// Writes `files` (path, content) into the repo and commits them.
fn commit_files(repo: &Path, files: &[(&str, &str)]) {
    for (rel, content) in files {
        let path = repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    for args in [
        &["add", "-f", "."][..],
        &["commit", "-q", "-m", "add files"][..],
    ] {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
}

fn create(daemon: &Daemon, repo: &Path, extra: Value) -> Value {
    let mut params = json!({
        "repo": repo.to_str().unwrap(),
        "title": "Env case",
        "goal": "Make a trivial change",
        "criteria": [],
        "verify": ["true"],
        "start": true,
    });
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    daemon.request("task.create", params)
}

fn canon(p: &Path) -> String {
    std::fs::canonicalize(p).unwrap().display().to_string()
}

#[test]
fn setup_runs_once_in_a_new_worktree_with_the_root_and_worktree_paths() {
    let (daemon, scripts) = spawn(PASS);
    let repo = init_git_repo();
    let log = scripts.path().join("setup.log");
    let setup = json!({"setup": format!(
        "echo \"$ORCHD_ROOT_PATH|$ORCHD_WORKTREE_PATH|$(pwd -P)\" >> {}", log.display()
    )});
    commit_files(repo.path(), &[(".sushiai/setup.json", &setup.to_string())]);

    let task = create(&daemon, repo.path(), json!({}));
    let id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &id);
    assert_eq!(settled["status"], "done", "{settled}");

    let text = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "setup ran exactly once: {text}");
    let parts: Vec<&str> = lines[0].split('|').collect();
    assert_eq!(canon(Path::new(parts[0])), canon(repo.path()));
    assert_eq!(parts[1], task["worktree"].as_str().unwrap());
    // The worktree is gone once the task is done: compare through its parent.
    let wt = Path::new(parts[1]);
    assert_eq!(
        Path::new(parts[2]),
        Path::new(&canon(wt.parent().unwrap())).join(wt.file_name().unwrap())
    );
    daemon.shutdown_and_wait();
}

#[test]
fn a_failing_setup_fails_the_task_with_the_output_tail_and_no_attempt() {
    let ran = tempfile::tempdir().unwrap();
    let harness_ran = ran.path().join("harness-ran");
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\ntouch {}\n",
        harness_ran.display()
    );
    let (daemon, _scripts) = spawn(&body);
    let repo = init_git_repo();
    commit_files(
        repo.path(),
        &[(
            ".sushiai/setup.json",
            r#"{"setup": "echo install-broke-here >&2; exit 4"}"#,
        )],
    );

    let task = create(&daemon, repo.path(), json!({}));
    let id = task["id"].as_str().unwrap().to_string();
    let settled = poll_until(&daemon, &id, |s| s == "failed");
    assert!(
        settled["decisions"]
            .to_string()
            .contains("install-broke-here"),
        "{settled}"
    );
    // Tried again twice before it failed.
    assert!(
        settled["decisions"]
            .to_string()
            .contains("trying again (2/2)"),
        "{settled}"
    );
    assert_eq!(
        settled["attempts"].as_array().unwrap().len(),
        0,
        "{settled}"
    );
    assert!(!harness_ran.exists(), "the harness never ran");
    daemon.shutdown_and_wait();
}

#[test]
fn a_setup_that_fails_once_is_tried_again_and_the_task_goes_on() {
    let marks = tempfile::tempdir().unwrap();
    let mark = marks.path().join("first-try-made");
    let (daemon, _scripts) = spawn(PASS);
    let repo = init_git_repo();
    let setup = json!({"setup": format!(
        "test -f '{m}' || {{ touch '{m}'; echo registry-down >&2; exit 4; }}",
        m = mark.display()
    )});
    commit_files(repo.path(), &[(".sushiai/setup.json", &setup.to_string())]);

    let task = create(&daemon, repo.path(), json!({}));
    let id = task["id"].as_str().unwrap().to_string();
    let settled = poll_until(&daemon, &id, |s| s == "done" || s == "failed");
    assert_eq!(settled["status"], "done", "{settled}");
    let decisions = settled["decisions"].to_string();
    assert!(decisions.contains("registry-down"), "{decisions}");
    assert!(decisions.contains("trying again (1/2)"), "{decisions}");
    daemon.shutdown_and_wait();
}

#[test]
fn the_base_check_worktree_runs_setup_before_the_check() {
    let (daemon, scripts) = spawn(PASS);
    let repo = init_git_repo();
    commit_files(
        repo.path(),
        &[(
            ".sushiai/setup.json",
            r#"{"setup": "touch SETUP_MADE.txt"}"#,
        )],
    );
    let log = scripts.path().join("check.log");
    // Always fails; records where it ran (the base worktree is detached) and
    // whether the file only setup creates was there.
    let check = format!(
        "echo \"$(git symbolic-ref -q HEAD || echo detached) $(test -f SETUP_MADE.txt && echo seen || echo missing)\" >> {}; exit 3",
        log.display()
    );
    let task = create(&daemon, repo.path(), json!({"finalVerify": [check]}));
    let id = task["id"].as_str().unwrap().to_string();
    poll_until(&daemon, &id, |s| s == "waiting");
    let text = std::fs::read_to_string(&log).unwrap();
    let base_runs: Vec<&str> = text.lines().filter(|l| l.starts_with("detached")).collect();
    assert!(!base_runs.is_empty(), "the check ran on the base: {text}");
    assert!(
        base_runs.iter().all(|l| l.ends_with("seen")),
        "setup ran in the base worktree first: {text}"
    );
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn a_nested_package_installed_by_setup_passes_its_verify() {
    let (daemon, _scripts) = spawn(PASS);
    let repo = init_git_repo();
    commit_files(
        repo.path(),
        &[
            (".gitignore", "node_modules/\n"),
            (
                "lib/package.json",
                r#"{"name":"demo-lib","version":"1.0.0","main":"index.js"}"#,
            ),
            ("lib/index.js", "module.exports = 'ok';\n"),
            (
                "hub/package.json",
                r#"{"name":"demo-hub","version":"1.0.0","private":true,"dependencies":{"demo-lib":"file:../lib"},"scripts":{"test":"node -e \"if (require('demo-lib') !== 'ok') process.exit(1)\""}}"#,
            ),
            (
                ".sushiai/setup.json",
                r#"{"setup": "npm install --prefix hub --no-audit --no-fund --no-package-lock"}"#,
            ),
        ],
    );
    let task = create(
        &daemon,
        repo.path(),
        json!({"verify": ["npm test --prefix hub"]}),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &id);
    assert_eq!(settled["status"], "done", "{settled}");
    daemon.shutdown_and_wait();
}

/// Writes an uncommitted file, then hangs until the task is stopped.
const HANG: &str = "#!/bin/sh\ncat > /dev/null\necho started > AGENT_STARTED.txt\nsleep 60\n";

/// Starts a hanging task whose setup appends a line to a log, and stops it
/// once the agent ran. Returns the task, its worktree and the log.
fn hanging_task_with_logged_setup(
    daemon: &Daemon,
    repo: &Path,
    log: &Path,
) -> (String, std::path::PathBuf) {
    let setup = json!({"setup": format!("echo ran >> {}", log.display())});
    commit_files(repo, &[(".sushiai/setup.json", &setup.to_string())]);
    let task = create(daemon, repo, json!({}));
    let id = task["id"].as_str().unwrap().to_string();
    let wt = std::path::PathBuf::from(task["worktree"].as_str().unwrap());
    wait_until("the agent started", || {
        wt.join("AGENT_STARTED.txt").exists().then_some(())
    });
    daemon.request("task.stop", json!({"id": id}));
    poll_until(daemon, &id, |s| s == "stopped");
    (id, wt)
}

fn setup_runs(log: &Path) -> usize {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .count()
}

#[test]
fn a_task_with_attempts_and_no_marker_gets_no_setup() {
    let (daemon, scripts) = spawn(HANG);
    let repo = init_git_repo();
    let log = scripts.path().join("setup.log");
    let (id, wt) = hanging_task_with_logged_setup(&daemon, repo.path(), &log);
    assert_eq!(setup_runs(&log), 1);
    let task = daemon.request("task.get", json!({"id": id}));
    assert!(!task["attempts"].as_array().unwrap().is_empty(), "{task}");

    // A task that was running before setup existed has attempts, no marker.
    std::fs::remove_file(
        daemon
            .data_dir()
            .join("tasks")
            .join(&id)
            .join("worktree-setup.done"),
    )
    .unwrap();
    std::fs::remove_file(wt.join("AGENT_STARTED.txt")).unwrap();
    daemon.request("task.start", json!({"id": id}));
    wait_until("the agent ran again", || {
        wt.join("AGENT_STARTED.txt").exists().then_some(())
    });
    assert_eq!(setup_runs(&log), 1, "no second setup");
    daemon.request("task.stop", json!({"id": id}));
    daemon.shutdown_and_wait();
}

#[test]
fn a_recreated_worktree_runs_setup_again() {
    let (daemon, scripts) = spawn(HANG);
    let repo = init_git_repo();
    let log = scripts.path().join("setup.log");
    let (id, wt) = hanging_task_with_logged_setup(&daemon, repo.path(), &log);
    assert_eq!(setup_runs(&log), 1);

    let removed = Command::new("git")
        .args(["worktree", "remove", "--force", wt.to_str().unwrap()])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(removed.success());
    daemon.request("task.start", json!({"id": id}));
    wait_until("the agent ran in the recreated worktree", || {
        wt.join("AGENT_STARTED.txt").exists().then_some(())
    });
    assert_eq!(setup_runs(&log), 2, "setup ran again");
    daemon.request("task.stop", json!({"id": id}));
    daemon.shutdown_and_wait();
}
