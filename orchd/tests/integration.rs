//! Black-box integration tests: spawn the real `orchd` binary and drive it
//! purely over its NDJSON unix socket, the same way Electron (Lane B) and
//! the Claude Stop hook do. No access to crate internals -- `orchd` is a
//! binary-only crate, so this is a separate process from the start.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

struct Daemon {
    child: Child,
    socket: PathBuf,
    data_dir: tempfile::TempDir,
    token: String,
}

/// Reads `<data>/control.token` (spec item A), trimmed -- the same file the
/// real Electron client reads before it can call anything but `ping`.
fn read_control_token(data_dir: &Path) -> String {
    let start = Instant::now();
    let path = data_dir.join("control.token");
    loop {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
        if start.elapsed() > Duration::from_secs(10) {
            panic!("orchd did not write control.token in time");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A test that panics before `shutdown_and_wait` must not leave its daemon
/// running (they used to pile up across runs).
impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Daemon {
    fn spawn(extra_env: &[(&str, &str)]) -> Daemon {
        let data_dir = tempfile::tempdir().unwrap();
        let socket = data_dir.path().join("orchd.sock");
        let child = spawn_orchd_raw(data_dir.path(), &socket, extra_env);
        wait_for_socket(&socket);
        let token = read_control_token(data_dir.path());
        Daemon {
            child,
            socket,
            data_dir,
            token,
        }
    }

    fn data_dir(&self) -> &Path {
        self.data_dir.path()
    }

    fn request(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        request_on(&self.socket, method, params, Some(&self.token))
    }

    fn shutdown_and_wait(mut self) {
        // Drop still runs afterwards and finds the child already reaped.
        let _ = self.request("shutdown", serde_json::json!({}));
        let start = Instant::now();
        loop {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            if start.elapsed() > Duration::from_secs(5) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn spawn_orchd_raw(data_dir: &Path, socket: &Path, extra_env: &[(&str, &str)]) -> Child {
    let bin = env!("CARGO_BIN_EXE_orchd");
    let mut cmd = Command::new(bin);
    cmd.args([
        "serve",
        "--data",
        data_dir.to_str().unwrap(),
        "--socket",
        socket.to_str().unwrap(),
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.spawn().expect("failed to spawn orchd")
}

fn wait_for_socket(socket: &Path) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if socket.exists() && UnixStream::connect(socket).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("orchd did not open its socket in time");
}

/// Wait for a raw (non-`Daemon`) child to exit, returning its status and
/// everything it wrote to stderr. Used for the singleton/bind-failure tests
/// where the process under test is expected to exit quickly on its own.
fn wait_for_exit(mut child: Child, timeout: Duration) -> (ExitStatus, String) {
    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let mut stderr = String::new();
            if let Some(mut s) = child.stderr.take() {
                let _ = s.read_to_string(&mut stderr);
            }
            return (status, stderr);
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            panic!("orchd did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn request_on(
    socket: &Path,
    method: &str,
    params: serde_json::Value,
    auth: Option<&str>,
) -> serde_json::Value {
    let mut stream = UnixStream::connect(socket).expect("connect to orchd socket");
    let id = "1";
    let mut req = serde_json::json!({"id": id, "method": method, "params": params});
    if let Some(token) = auth {
        req["auth"] = serde_json::json!(token);
    }
    let mut line = req.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes()).unwrap();
    stream.flush().unwrap();

    let mut reader = BufReader::new(stream);
    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .expect("read response line from orchd");
    let v: serde_json::Value =
        serde_json::from_str(response_line.trim()).expect("valid JSON response");
    if let Some(err) = v.get("error") {
        panic!("{method} failed: {err}");
    }
    v.get("result").cloned().unwrap_or(serde_json::Value::Null)
}

/// `git init` + one commit in a fresh temp dir, for `task.create` to branch
/// a worktree from.
fn init_git_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "orchd-test@example.com"]);
    run(&["config", "user.name", "orchd test"]);
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init"]);
    dir
}

/// Sends a raw request without going through `request_on` (which panics on
/// an error response) so the "unauthorized" error itself can be asserted on.
fn raw_request(socket: &Path, method: &str, auth: Option<&str>) -> serde_json::Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    let mut req = serde_json::json!({"id": "1", "method": method, "params": {}});
    if let Some(token) = auth {
        req["auth"] = serde_json::json!(token);
    }
    stream.write_all(format!("{req}\n").as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(line.trim()).unwrap()
}

#[test]
fn requests_without_the_control_token_are_rejected() {
    let daemon = Daemon::spawn(&[]);

    // ping needs no token.
    let ping = request_on(&daemon.socket, "ping", serde_json::json!({}), None);
    assert!(ping["pid"].as_u64().unwrap() > 0);

    // Everything else does: no auth field at all, or the wrong token, both
    // rejected before the method ever runs.
    let no_auth = raw_request(&daemon.socket, "settings.get", None);
    assert_eq!(no_auth["error"]["message"], "unauthorized");
    let wrong_auth = raw_request(&daemon.socket, "settings.get", Some("not-the-real-token"));
    assert_eq!(wrong_auth["error"]["message"], "unauthorized");

    // The real token works.
    let settings = daemon.request("settings.get", serde_json::json!({}));
    assert_eq!(settings["maxAttempts"], 4);

    daemon.shutdown_and_wait();
}

#[test]
fn ping_settings_task_create_and_list_round_trip() {
    let daemon = Daemon::spawn(&[]);

    let ping = daemon.request("ping", serde_json::json!({}));
    assert!(ping["pid"].as_u64().unwrap() > 0);
    assert!(!ping["dataDir"].as_str().unwrap().is_empty());
    assert!(ping["binaryMtimeMs"].as_u64().unwrap() > 0);
    assert_eq!(ping["running"], 0);

    let settings = daemon.request("settings.get", serde_json::json!({}));
    assert_eq!(settings["maxAttempts"], 4);
    assert_eq!(settings["tiers"]["standard"], "claude-sonnet");

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Add a widget",
            "goal": "Add a widget to the page",
            "criteria": ["widget visible"],
            "verify": ["true"],
        }),
    );
    assert_eq!(task["status"], "queued");
    assert_eq!(task["branch"], "task/add-a-widget");
    let worktree = task["worktree"].as_str().unwrap();
    assert!(
        Path::new(worktree).is_dir(),
        "worktree directory should exist"
    );

    let list = daemon.request("task.list", serde_json::json!({}));
    let tasks = list.as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], task["id"]);

    let get = daemon.request("task.get", serde_json::json!({"id": task["id"]}));
    assert_eq!(get["title"], "Add a widget");

    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

fn fake_harness_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn poll_task_status(daemon: &Daemon, task_id: &str, timeout: Duration) -> serde_json::Value {
    let start = Instant::now();
    loop {
        let task = daemon.request("task.get", serde_json::json!({"id": task_id}));
        let status = task["status"].as_str().unwrap_or("");
        if status != "queued" && status != "running" {
            return task;
        }
        if start.elapsed() > timeout {
            panic!("task {task_id} did not settle in time, last status: {status}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn engine_loop_passes_when_verify_succeeds() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"changed\" > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );

    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    // This test only exercises the verify gate, not review; turn review off
    // so it never shells out to a real `codex`/`claude` binary that might
    // happen to be on this machine's PATH.
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Pass case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["attempts"].as_array().unwrap().len(), 1);
    assert_eq!(settled["attempts"][0]["status"], "passed");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn engine_loop_waits_after_verify_keeps_failing_and_attempts_are_exhausted() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"changed\" > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );

    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(2);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Fail case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["false"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(settled["status"], "waiting", "task JSON: {settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "should stop after maxAttempts=2");
    for a in attempts {
        assert_eq!(a["status"], "failed");
        assert_eq!(a["failure"]["kind"], "verify");
    }
    assert!(settled["question"]["text"]
        .as_str()
        .unwrap()
        .starts_with("Attempts keep failing with"));

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn second_daemon_with_same_data_dir_but_different_socket_refuses_to_start() {
    // Electron can legitimately pick a different `--socket` across launches
    // for the same `--data` dir (it falls back to a `$TMPDIR` path when the
    // natural one is too long), so the singleton lock must not depend on
    // both invocations agreeing on the socket path.
    let data_dir = tempfile::tempdir().unwrap();
    let socket1 = data_dir.path().join("first.sock");
    let first = spawn_orchd_raw(data_dir.path(), &socket1, &[]);
    wait_for_socket(&socket1);

    let socket2 = data_dir.path().join("second.sock");
    let second = spawn_orchd_raw(data_dir.path(), &socket2, &[]);
    let (status, stderr) = wait_for_exit(second, Duration::from_secs(5));
    assert!(!status.success(), "second daemon should refuse to start");
    assert!(
        stderr.contains("already running"),
        "stderr should explain why: {stderr}"
    );
    assert!(
        !socket2.exists(),
        "the second daemon must not have bound a socket of its own"
    );

    // The first daemon must be completely unaffected.
    let ping = request_on(&socket1, "ping", serde_json::json!({}), None);
    assert!(ping["pid"].as_u64().unwrap() > 0);
    let token = read_control_token(data_dir.path());
    let _ = request_on(&socket1, "shutdown", serde_json::json!({}), Some(&token));
    let _ = wait_for_exit(first, Duration::from_secs(5));
}

#[test]
fn daemon_exits_nonzero_when_the_socket_path_is_too_long() {
    let data_dir = tempfile::tempdir().unwrap();
    // Unix socket paths are capped well under 200 bytes on every platform
    // orchd targets; this is comfortably past that so `UnixListener::bind`
    // fails with ENAMETOOLONG.
    let long_name = "x".repeat(200);
    let socket = data_dir.path().join(format!("{long_name}.sock"));
    let child = spawn_orchd_raw(data_dir.path(), &socket, &[]);
    let (status, stderr) = wait_for_exit(child, Duration::from_secs(5));
    assert!(
        !status.success(),
        "daemon should exit non-zero when it can't bind its socket"
    );
    assert!(
        !stderr.trim().is_empty(),
        "daemon should print a clear message when bind fails"
    );
}

/// A fake Claude harness that exercises the *real* Stop hook path: it reads
/// the `--settings` file it was given, extracts the hook command orchd
/// wired up, and actually runs `orchd hook stop` against the live daemon
/// with a synthetic payload on stdin -- the same way the real Claude CLI
/// would, just without a real model in the loop.
const FAKE_CLAUDE_HOOK_SCRIPT: &str = r#"#!/usr/bin/env node
const fs = require('fs');
const { execSync } = require('child_process');

const args = process.argv.slice(2);
const settingsPath = args[args.indexOf('--settings') + 1];
const settings = JSON.parse(fs.readFileSync(settingsPath, 'utf8'));
const hookCmd = settings.hooks.Stop[0].hooks[0].command;

try { fs.readFileSync(0); } catch (e) {}

fs.writeFileSync('CHANGED_MARKER.txt', 'changed\n');

console.log(JSON.stringify({ type: 'system', subtype: 'init', session_id: 'sess-fake' }));

const payload = JSON.stringify({
  session_id: 's',
  stop_hook_active: false,
  last_assistant_message: 'done',
});
let hookOutput = '';
try {
  hookOutput = execSync(hookCmd, { input: payload }).toString();
} catch (e) {
  hookOutput = (e.stdout || '').toString();
}

// Simulate the agent reading the hook's block reason and fixing it before
// actually finishing the session -- inside the worktree, so the fix is
// itself part of the diff the verify-result cache keys on (spec item 14):
// the hook's failing verify and the post-session gate's re-check must see
// two different diffs, not reuse a stale cached failure.
fs.writeFileSync('verified-marker.txt', 'fixed\n');

console.log(JSON.stringify({
  type: 'result',
  total_cost_usd: 0.01,
  usage: { input_tokens: 1, output_tokens: 1 },
  result: 'stop hook said: ' + hookOutput.trim(),
}));
"#;

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

#[test]
fn a_task_waiting_for_its_owner_does_not_hold_a_parallel_slot() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"changed\" > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let data = data_holder.path().to_path_buf();
    // `parallel` is read at startup: save it with a first daemon, then
    // restart onto the saved settings in the same data dir.
    let socket1 = data.join("orchd1.sock");
    let first = spawn_orchd_raw(&data, &socket1, &fake_bins);
    wait_for_socket(&socket1);
    let token1 = read_control_token(&data);
    let mut settings = request_on(
        &socket1,
        "settings.get",
        serde_json::json!({}),
        Some(&token1),
    );
    settings["maxAttempts"] = serde_json::json!(1);
    settings["parallel"] = serde_json::json!(1);
    settings["review"] = serde_json::json!("");
    request_on(
        &socket1,
        "settings.set",
        serde_json::json!({"settings": settings}),
        Some(&token1),
    );
    let _ = request_on(&socket1, "shutdown", serde_json::json!({}), Some(&token1));
    let _ = wait_for_exit(first, Duration::from_secs(5));

    let socket = data.join("orchd2.sock");
    let child = spawn_orchd_raw(&data, &socket, &fake_bins);
    wait_for_socket(&socket);
    let token = read_control_token(&data);
    let call = |m: &str, p: serde_json::Value| request_on(&socket, m, p, Some(&token));

    let repo = init_git_repo();
    let waiting = call(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Always fails", "goal": "g", "criteria": [], "verify": ["false"], "start": true}),
    );
    let waiting_id = waiting["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    while call("task.get", serde_json::json!({"id": waiting_id}))["status"] != "waiting" {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "first task never reached waiting"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(call("ping", serde_json::json!({}))["running"], 0);

    let passing = call(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Passes", "goal": "g", "criteria": [], "verify": ["true"], "start": true}),
    );
    let passing_id = passing["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    loop {
        let status = call("task.get", serde_json::json!({"id": passing_id}))["status"].clone();
        if status == "done" {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "second task starved: {status}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = call("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(child, Duration::from_secs(5));
    for t in [&waiting, &passing] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}
