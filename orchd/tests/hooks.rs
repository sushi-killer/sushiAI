//! Black-box integration tests (hooks): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn stop_hook_blocks_on_failing_verify_then_task_passes_once_fixed() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude-hook.js",
        FAKE_CLAUDE_HOOK_SCRIPT,
    );

    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    // Skip review so this test only exercises the Stop hook + verify gate.
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    // Relative to the worktree (the verify command's cwd), and part of the
    // diff once the fake script writes it -- unlike a file outside the
    // repo, this actually changes the verify-cache key between the hook's
    // call and the post-session gate's.
    let verify_cmd = "test -f verified-marker.txt".to_string();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Stop hook case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": [verify_cmd],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0]["gateBlocks"], 1,
        "the one Stop-hook block should be recorded on the attempt"
    );

    let events_path = daemon
        .data_dir()
        .join("tasks")
        .join(&task_id)
        .join("runs")
        .join("1")
        .join("events.jsonl");
    let events_text = std::fs::read_to_string(&events_path).unwrap_or_default();
    assert!(
        events_text.contains("Verification failed"),
        "expected the hook's block reason to show up in events.jsonl: {events_text}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

// -- variant.leanOutput ---------------------------------------------------

/// Runs `orchd hook rtk` directly, with the child's own `PATH` set to
/// `path_dir` (a fake `rtk`, or, for the "absent" case, nothing) followed by
/// only `/usr/bin:/bin` -- fixed system dirs, not this machine's real `PATH`,
/// which may well have its own `rtk` on it (a fake or empty `path_dir` must
/// never fall through to that one) but always has `/bin/sh`/`sleep` for a
/// fake rtk script to use. The test process's own `PATH` is never read or
/// mutated. Returns the child's trimmed stdout.
fn rtk_hook(path_dir: &Path, stdin: &str) -> String {
    rtk_hook_with_timeout(path_dir, stdin, "10000")
}

/// `timeout_ms` is the hook's rtk timeout: generous by default so a loaded
/// machine never times a fast fake out.
fn rtk_hook_with_timeout(path_dir: &Path, stdin: &str, timeout_ms: &str) -> String {
    let path = format!("{}:/usr/bin:/bin", path_dir.display());
    let mut child = Command::new(env!("CARGO_BIN_EXE_orchd"))
        .arg("hook")
        .arg("rtk")
        .env("PATH", path)
        .env("ORCHD_RTK_REWRITE_TIMEOUT_MS", timeout_ms)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn rtk_payload(command: &str) -> String {
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command},
    })
    .to_string()
}

#[test]
fn lean_output_rtk_hook_rewrites_bash_and_keeps_the_rest_of_tool_input() {
    let dir = tempfile::tempdir().unwrap();
    fake_harness_script(dir.path(), "rtk", "#!/bin/sh\necho \"timeout 60 $2\"\n");
    let stdin = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": "npm test", "description": "run the tests", "timeout": 120_000},
    })
    .to_string();
    let out = rtk_hook(dir.path(), &stdin);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "allow");
    assert_eq!(
        v["hookSpecificOutput"]["updatedInput"],
        serde_json::json!({
            "command": "timeout 60 npm test",
            "description": "run the tests",
            "timeout": 120_000
        }),
        "{out}"
    );
}

#[test]
fn lean_output_rtk_hook_answers_empty_when_it_has_nothing_useful_to_say() {
    let stdin = rtk_payload("npm test");

    // rtk echoes the same command back, or declines with a non-zero exit
    // (its common answer -- most commands have no rewrite).
    for (name, script) in [
        ("same command back", "#!/bin/sh\necho \"$2\"\n"),
        ("non-zero exit", "#!/bin/sh\nexit 1\n"),
        ("empty stdout", "#!/bin/sh\nexit 0\n"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fake_harness_script(dir.path(), "rtk", script);
        assert_eq!(rtk_hook(dir.path(), &stdin), "{}", "{name}");
    }

    // No `rtk` anywhere on PATH.
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(rtk_hook(empty.path(), &stdin), "{}", "absent from PATH");

    // Bad or missing input never even reaches rtk -- a rewriting fake would
    // prove that if it were called.
    let dir = tempfile::tempdir().unwrap();
    fake_harness_script(dir.path(), "rtk", "#!/bin/sh\necho \"rewritten $2\"\n");
    for bad in [
        "not json",
        "{}",
        r#"{"tool_input": {}}"#,
        r#"{"tool_input": {"command": 1}}"#,
    ] {
        assert_eq!(rtk_hook(dir.path(), bad), "{}", "{bad}");
    }
}

#[test]
fn lean_output_rtk_hook_kills_a_slow_rtk_on_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("rtk.pid");
    fake_harness_script(
        dir.path(),
        "rtk",
        &format!(
            "#!/bin/sh\necho $$ > {}\nsleep 60\necho \"$2 --slow\"\n",
            pid_file.display()
        ),
    );
    let stdin = rtk_payload("npm test");
    let start = Instant::now();
    assert_eq!(rtk_hook_with_timeout(dir.path(), &stdin, "8000"), "{}");
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(30),
        "the hook must return well before the fake's 60s sleep, took {elapsed:?}"
    );
    // Give the kill a brief moment to land before checking.
    std::thread::sleep(Duration::from_millis(200));
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(!is_alive(pid), "the slow rtk should be killed on timeout");
}

#[test]
fn lean_output_rtk_hook_never_rewrites_git_commit_or_push() {
    let dir = tempfile::tempdir().unwrap();
    // Would rewrite anything it's asked about.
    fake_harness_script(dir.path(), "rtk", "#!/bin/sh\necho \"rtk $2\"\n");
    for command in ["git commit -m x", "git push"] {
        assert_eq!(
            rtk_hook(dir.path(), &rtk_payload(command)),
            "{}",
            "{command}"
        );
    }
}

#[test]
fn lean_output_gives_a_claude_implement_run_the_rtk_hook_and_output_cap() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", FAKE_CLAUDE_PASS);
    let args = scripts_dir.path().join("args");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("CLAUDE_ARGS", args.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Lean output",
            "goal": "Change the widget",
            "verify": ["true"],
            "variant": {"leanOutput": true},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(settled["variant"]["leanOutput"], true);

    let settings: serde_json::Value =
        serde_json::from_str(&run_file(&daemon, &task_id, "settings.json")).unwrap();
    let hooks: Vec<&String> = settings["hooks"].as_object().unwrap().keys().collect();
    assert!(hooks.contains(&&"Stop".to_string()), "{settings}");
    assert!(hooks.contains(&&"PreToolUse".to_string()), "{settings}");
    let pre = &settings["hooks"]["PreToolUse"][0];
    assert_eq!(pre["matcher"], "Bash");
    assert!(
        pre["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("hook rtk"),
        "{settings}"
    );
    assert!(pre["hooks"][0]["timeout"].as_u64().unwrap() > 0);
    assert!(
        settings["bashOutputMaxChars"].as_u64() == Some(10_000),
        "{settings}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn without_lean_output_a_claude_implement_run_gets_no_rtk_hook_or_output_cap() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", FAKE_CLAUDE_PASS);
    let args = scripts_dir.path().join("args");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("CLAUDE_ARGS", args.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Not lean output",
            "goal": "Change the widget",
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(settled["variant"]["leanOutput"], false);

    let settings: serde_json::Value =
        serde_json::from_str(&run_file(&daemon, &task_id, "settings.json")).unwrap();
    let hooks: Vec<&String> = settings["hooks"].as_object().unwrap().keys().collect();
    assert_eq!(hooks, ["Stop"], "{settings}");
    assert!(settings.get("env").is_none(), "{settings}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}
