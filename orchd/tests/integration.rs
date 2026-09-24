//! Black-box integration tests: spawn the real `orchd` binary and drive it
//! purely over its NDJSON unix socket, the same way Electron (Lane B) and
//! the Claude Stop hook do. No access to crate internals -- `orchd` is a
//! binary-only crate, so this is a separate process from the start.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
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
        if native_sandbox_unavailable() {
            let mut settings =
                request_on(&socket, "settings.get", serde_json::json!({}), Some(&token));
            fit_sandbox(&mut settings);
            request_on(
                &socket,
                "settings.set",
                serde_json::json!({"settings": settings}),
                Some(&token),
            );
        }
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

/// Whether this process cannot apply a `sandbox-exec` profile. macOS refuses
/// to nest one inside another (`sandbox_apply: Operation not permitted`),
/// which is exactly where this suite runs when orchd's own Native-sandboxed
/// verify runs `npm run test:orchd`: every daemon here would then fail each
/// verify command it wraps in `sandbox-exec`, so no task could ever pass.
fn native_sandbox_unavailable() -> bool {
    static UNAVAILABLE: OnceLock<bool> = OnceLock::new();
    *UNAVAILABLE.get_or_init(|| {
        cfg!(target_os = "macos")
            && !Command::new("sandbox-exec")
                .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
    })
}

/// Switches a `settings.get` result to the `host` sandbox when the native
/// one can't be applied here; anywhere else the tests keep exercising the
/// real `sandbox-exec` path.
fn fit_sandbox(settings: &mut serde_json::Value) {
    if native_sandbox_unavailable() {
        settings["sandbox"] = serde_json::json!("host");
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
    raw_request_with_params(socket, method, serde_json::json!({}), auth)
}

fn raw_request_with_params(
    socket: &Path,
    method: &str,
    params: serde_json::Value,
    auth: Option<&str>,
) -> serde_json::Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    let mut req = serde_json::json!({"id": "1", "method": method, "params": params});
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

/// `kill(pid, 0)`: true iff a process with this pid exists and is
/// signalable by us -- the standard liveness probe, same one
/// `main.rs::acquire_singleton` uses for the daemon's own pidfile.
fn is_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Regression test for the bug where `App::shutdown()` and `main.rs`'s
/// drain loop only ever looked at `controls` (task attempts), never
/// `chat_turns`: a `shutdown` mid-conversation left the orchestrator chat's
/// `setsid`'d harness child running forever, orphaned, with the daemon gone
/// and nothing left to kill it.
#[test]
fn shutdown_kills_a_live_orchestrator_chat_turn_s_child() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("chat-pid");
    // Records its own pid (== the pid tokio spawned, since a shebang script
    // is exec'd in place, never forked again) then hangs without ever
    // printing a `result` line -- exactly what a slow chat turn looks like
    // from orchd's side. The rename keeps the test from reading the marker
    // between its creation and the pid landing in it.
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\necho $$ > {m}.tmp\nmv {m}.tmp {m}\nsleep 30\n",
        m = marker.display()
    );
    let script = fake_harness_script(scripts_dir.path(), "fake-claude-chat.sh", &body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let repo = tempfile::tempdir().unwrap();
    daemon.request(
        "chat.send",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "text": "hello"}),
    );

    let start = Instant::now();
    while !marker.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "chat harness never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let pid: i32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .expect("marker should contain the harness pid");
    assert!(
        is_alive(pid),
        "the fake harness should still be running before shutdown"
    );

    // Same call the acceptance criteria names: `shutdown` mid-conversation.
    daemon.shutdown_and_wait();

    assert!(
        !is_alive(pid),
        "chat turn's child (pid {pid}) leaked past the daemon's own shutdown"
    );
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
            panic!("task {task_id} did not settle in time, last status: {status}: {task}");
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
fn a_review_without_a_verdict_waits_for_the_owner_instead_of_passing() {
    // One fake plays both roles: the implementer edits a file and reports;
    // the reviewer (its brief opens with "## Review") answers in prose with
    // no sushi-review block, which used to count as PASS.
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\nbrief=$(cat)\ncase \"$brief\" in\n\"## Review\"*) echo '{\"type\":\"result\",\"result\":\"Looks fine to me.\"}' ;;\n*) echo changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}' ;;\nesac\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    // `auto` review of the standard route lands on the hard tier's route,
    // which is also Claude here, so the same fake answers it.
    let settings = daemon.request("settings.get", serde_json::json!({}));
    assert_eq!(settings["review"], "auto");

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Review case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting" || s == "done" || s == "failed"
    });
    assert_eq!(waiting["status"], "waiting", "task JSON: {waiting}");
    let question = waiting["question"]["text"].as_str().unwrap();
    assert!(question.contains("no verdict"), "{question}");

    daemon.request(
        "task.answer",
        serde_json::json!({"id": task_id, "answer": "approve"}),
    );
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert!(settled["question"].is_null(), "task JSON: {settled}");
    let decisions = settled["decisions"].to_string();
    assert!(decisions.contains("Owner: approve"), "{decisions}");
    assert!(
        decisions.contains("without a review verdict"),
        "{decisions}"
    );

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
    fit_sandbox(&mut settings);
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

/// A fake planner/implement harness in one script: it can tell which stage
/// it was invoked for from the brief text on its own stdin (a plan brief is
/// the only one that ever mentions "sushi-plan"), and replies with a
/// ```sushi-plan fenced draft -- with one question when `PLAN_ASK_QUESTION`
/// is set, none otherwise -- or a normal ```sushi-report implement reply.
// `printf '%s\n' "$var"` rather than `echo '...'`: this machine's `/bin/sh`
// (bash running in sh-compatibility mode) has its builtin `echo` interpret
// backslash escapes like `\n` by default, which would silently split a
// single JSON line's embedded `\n` into a real newline -- breaking the line
// framing the harness reads on stdout. `printf`'s `%s` does no such
// interpretation on its argument, only on the (separate, literal) format
// string.
const FAKE_PLANNER_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    if [ -n "$PLAN_ASK_QUESTION" ]; then
      json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"true\"],\"questions\":[{\"text\":\"Which default theme?\",\"options\":[\"light\",\"dark\"]}]}\n```"}'
    else
      json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"true\"],\"questions\":[]}\n```"}'
    fi
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

fn poll_until(
    daemon: &Daemon,
    task_id: &str,
    timeout: Duration,
    pred: impl Fn(&str) -> bool,
) -> serde_json::Value {
    let start = Instant::now();
    loop {
        let task = daemon.request("task.get", serde_json::json!({"id": task_id}));
        let status = task["status"].as_str().unwrap_or("").to_string();
        if pred(&status) {
            return task;
        }
        assert!(
            start.elapsed() < timeout,
            "task {task_id} did not reach the expected status in time, stuck at {status}: {task}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn drafting_task_asks_its_planner_question_then_implements_and_passes() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PLAN_ASK_QUESTION", "1"),
    ]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": true,
        }),
    );
    assert_eq!(task["status"], "drafting");
    let task_id = task["id"].as_str().unwrap().to_string();

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert_eq!(waiting["question"]["text"], "Which default theme?");
    assert_eq!(
        waiting["question"]["options"],
        serde_json::json!(["light", "dark"])
    );
    // The plan attempt is recorded but must never count against maxAttempts
    // or show up as an implement attempt.
    assert_eq!(waiting["attempts"][0]["stage"], "plan");

    daemon.request(
        "task.answer",
        serde_json::json!({"id": task_id, "answer": "dark"}),
    );

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["title"], "Add dark mode");
    assert_eq!(
        settled["criteria"],
        serde_json::json!(["Toggle visible in settings"])
    );
    assert!(settled["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d.as_str().unwrap().contains("Which default theme? -> dark")));
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "one plan attempt, one implement attempt");
    assert_eq!(attempts[0]["stage"], "plan");
    assert_eq!(attempts[1]["stage"], "implement");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn drafting_task_with_no_questions_goes_straight_to_done() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["title"], "Add dark mode");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn drafting_task_created_with_start_false_stops_after_planning_for_review() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": false,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    // Planning runs regardless of `start: false` -- only whether it then
    // proceeds into implementing depends on it.
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "stopped" || s == "done" || s == "failed"
    });
    assert_eq!(settled["status"], "stopped", "task JSON: {settled}");
    assert_eq!(settled["title"], "Add dark mode");
    assert_eq!(
        settled["criteria"],
        serde_json::json!(["Toggle visible in settings"])
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn task_create_rejects_request_form_when_the_planner_is_disabled() {
    let daemon = Daemon::spawn(&[]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["planner"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let result = raw_request_with_params(
        &daemon.socket,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "request": "add a widget"}),
        Some(&daemon.token),
    );
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("planner is disabled"),
        "{result}"
    );

    daemon.shutdown_and_wait();
}

#[test]
fn task_create_rejects_neither_request_nor_title_and_goal() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let result = raw_request_with_params(
        &daemon.socket,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap()}),
        Some(&daemon.token),
    );
    assert!(result.get("error").is_some(), "{result}");

    daemon.shutdown_and_wait();
}

/// The plan branch never produces a valid ```sushi-plan fence, no matter how
/// many times it's asked -- for exercising the "could not draft this task"
/// clarification path itself (as opposed to [`CLARIFY_THEN_SUCCEED_SCRIPT`],
/// which uses the clarification to recover).
const GARBAGE_PLANNER_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"no fenced block here, sorry"}'
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

/// The plan branch fails to parse until the brief it's given contains the
/// owner's clarification (which `run_plan_stage` folds into `task.request`
/// verbatim as `"(Owner clarification: ...)"`), then succeeds -- for
/// exercising "ask -> answer -> replan -> succeeds" end to end.
const CLARIFY_THEN_SUCCEED_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    case "$input" in
      *"Owner clarification"*)
        json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"true\"],\"questions\":[]}\n```"}'
        ;;
      *)
        json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"no fenced block here, sorry"}'
        ;;
    esac
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

/// Like [`FAKE_PLANNER_SCRIPT`] but the draft's own `verify` always fails,
/// for exercising the implement loop's own attempt budget on top of a
/// drafted task (spec review item P1-3: the plan attempt must never count
/// against `maxAttempts`).
const FAILING_VERIFY_PLANNER_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"false\"],\"questions\":[]}\n```"}'
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

#[test]
fn maxattempts_one_still_gives_the_implement_stage_its_own_one_attempt() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-planner-failing-verify.sh",
        FAILING_VERIFY_PLANNER_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["maxAttempts"] = serde_json::json!(1);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    // If the plan attempt wrongly counted against maxAttempts, the implement
    // stage would already be "exhausted" before it ever got to try -- this
    // proves it actually got its own single attempt instead.
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(
        attempts.len(),
        2,
        "one plan attempt, one implement attempt: {settled}"
    );
    assert_eq!(attempts[0]["stage"], "plan");
    assert_eq!(attempts[1]["stage"], "implement");
    assert_eq!(attempts[1]["failure"]["kind"], "verify");
    assert!(settled["question"]["text"]
        .as_str()
        .unwrap()
        .starts_with("Attempts keep failing with"));

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn a_task_waiting_on_its_plan_question_does_not_hold_a_parallel_slot() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let fake_bins = [
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PLAN_ASK_QUESTION", "1"),
    ];
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
    settings["parallel"] = serde_json::json!(1);
    settings["review"] = serde_json::json!("");
    fit_sandbox(&mut settings);
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
    let drafting = call(
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "request": "add dark mode", "start": true}),
    );
    let drafting_id = drafting["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    while call("task.get", serde_json::json!({"id": drafting_id}))["status"] != "waiting" {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "drafting task never reached waiting on its plan question"
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
            "second task starved while the first sat on a plan question: {status}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = call("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(child, Duration::from_secs(5));
    for t in [&drafting, &passing] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn stop_then_start_while_drafting_replans_from_scratch() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-planner-garbage.sh",
        GARBAGE_PLANNER_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    let first_wait = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert!(first_wait["question"]["text"]
        .as_str()
        .unwrap()
        .contains("could not draft"));
    let plan_attempts_before = first_wait["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "plan")
        .count();
    assert_eq!(plan_attempts_before, 1);

    daemon.request("task.stop", serde_json::json!({"id": task_id}));
    let stopped = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "stopped" || s == "failed"
    });
    assert_eq!(stopped["status"], "stopped", "task JSON: {stopped}");

    daemon.request("task.start", serde_json::json!({"id": task_id}));
    let second_wait = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    let plan_attempts_after = second_wait["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "plan")
        .count();
    assert_eq!(
        plan_attempts_after, 2,
        "task.start on a stopped, never-passed draft should re-run the plan stage from scratch: {second_wait}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn unparseable_plan_then_owner_clarification_leads_to_a_successful_replan() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-planner-clarify.sh",
        CLARIFY_THEN_SUCCEED_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert!(waiting["question"]["text"]
        .as_str()
        .unwrap()
        .contains("could not draft"));

    daemon.request(
        "task.answer",
        serde_json::json!({"id": task_id, "answer": "it's the settings screen's theme picker"}),
    );

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["title"], "Add dark mode");
    let plan_attempts = settled["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "plan")
        .count();
    assert_eq!(
        plan_attempts, 2,
        "the first (unparseable) round and the successful replan: {settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn drafted_task_still_gets_classified_despite_the_plan_attempt() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");

    // Before the P1-3 fix, `task.attempts.is_empty()` was already false once
    // the plan attempt existed, so `classify_tier` (and its "tier" journal
    // entry) was silently skipped for the task's very first implement try.
    let decisions_path = daemon.data_dir().join("decisions.jsonl");
    let decisions_text = std::fs::read_to_string(&decisions_path).unwrap_or_default();
    assert!(
        decisions_text
            .lines()
            .any(|l| l.contains("\"point\":\"tier\"")),
        "expected a tier classification journal entry: {decisions_text}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// A one-shot-per-connection fake HTTP server that always answers with
/// `answers_json` wrapped as an OpenAI chat-completion envelope -- a local,
/// deterministic stand-in for `classify::decide`'s `Openai` backend (the
/// only backend whose base URL is a runtime setting rather than hardcoded),
/// so a test can drive a real classifier success without a network call.
/// Runs on a detached thread for the test process's lifetime -- ponytail:
/// nothing ever joins it, since the process exit is what reclaims it.
fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn spawn_fake_openai_classifier(answers_json: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let body = serde_json::json!({
        "choices": [{"message": {"content": answers_json}}]
    })
    .to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            // Drain the full request (headers + declared Content-Length)
            // before writing anything back: closing on a socket that still
            // has unread bytes queued can RST the connection and truncate
            // our own response, which showed up as an intermittent ureq
            // "invalid header" parse failure on the client side.
            let mut received = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        received.extend_from_slice(&chunk[..n]);
                        let Some(header_end) = find_double_crlf(&received) else {
                            continue;
                        };
                        let headers = String::from_utf8_lossy(&received[..header_end]);
                        let content_length: usize = headers
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().to_string())
                            })
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        if received.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    format!("http://{addr}")
}

#[test]
fn jev_tier_decision_lands_in_task_decisions_on_classifier_success() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"changed\" > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    // "standard" routes to the Claude harness (default `tiers` map), which
    // is the one faked above via `ORCHD_CLAUDE_BIN` -- "mechanical" would
    // route to Codex and hang waiting on a real `codex` binary.
    let base_url = spawn_fake_openai_classifier(
        r#"{"answers":{"tier":{"choice":"standard","probabilities":{"mechanical":0.08,"standard":0.82,"hard":0.1}}}}"#,
    );
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["classifier"] =
        serde_json::json!({"backend": "openai", "model": "fake", "providerId": "fake"});
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    daemon.request(
        "secrets.set",
        serde_json::json!({"classifier": {"key": "test-key", "baseUrl": base_url}}),
    );

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
    let decisions = settled["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(|d| d
            .as_str()
            .unwrap_or("")
            .starts_with("Jev: tier standard (p 0.82) -> route ")),
        "expected a Jev tier decision: {decisions:?}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Reports `blocked` the first time (with classifier off by default, that
/// takes the "Jev says not answerable / Jev is off" branch straight into
/// triage), then `complete` on the retry -- so a triage "answer" lets the
/// task finish without ever reaching the owner. A brief containing
/// "sushi-triage" (only the triage report format mentions it) picks the
/// triage branch regardless of which route/attempt is asking.
const TRIAGE_ANSWERS_THEN_SUCCEEDS_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-triage*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-triage"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-triage\n{\"action\":\"answer\",\"answer\":\"use approach A\",\"reason\":\"README already says so\"}\n```"}'
    printf '%s\n' "$json"
    ;;
  *)
    if [ -f ALREADY_BLOCKED_ONCE.txt ]; then
      echo "changed" > CHANGED_MARKER.txt
      printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
      json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
      printf '%s\n' "$json"
    else
      touch ALREADY_BLOCKED_ONCE.txt
      echo "changed" > CHANGED_MARKER.txt
      printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
      json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"blocked\",\"summary\":\"need input\",\"decisions\":[],\"question\":\"Which approach, A or B?\"}\n```"}'
      printf '%s\n' "$json"
    fi
    ;;
esac
"#;

#[test]
fn triage_answer_lets_the_task_continue_without_the_owner() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        TRIAGE_ANSWERS_THEN_SUCCEEDS_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    // `classifier.backend: "openrouter"` with no key configured routes the
    // first blocked report into "Jev is off" rather than a classifier call.
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["orchestrator"] = serde_json::json!("claude-sonnet");
    settings["autoAnswer"] = serde_json::json!(true);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Pick an approach",
            "goal": "Add the feature",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(
        settled["status"], "done",
        "triage should have answered the blocked question itself: {settled}"
    );
    let decisions = settled["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(|d| {
            let d = d.as_str().unwrap_or("");
            d.starts_with("Orchestrator: Which approach, A or B?")
                && d.contains("use approach A")
                && d.contains("README already says so")
        }),
        "expected an Orchestrator answer decision: {decisions:?}"
    );
    // The blocked attempt keeps its own state: triage neither overwrites its
    // session nor drops its cost (two implement runs plus one triage run).
    let first = &settled["attempts"][0];
    assert_eq!(first["failure"]["kind"], "blocked", "attempt: {first}");
    assert_eq!(first["sessionId"], "sess-fake", "attempt: {first}");
    let cost = settled["costUsd"].as_f64().unwrap();
    assert!((cost - 0.03).abs() < 1e-9, "cost {cost}: {settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn a_task_is_carried_onto_its_base_when_the_base_moves_mid_attempt() {
    // The fake agent commits to the base branch of the main checkout while
    // it works, as another merged task would.
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\nroot=\"$(git rev-parse --git-common-dir)/..\"\ngit -C \"$root\" -c user.email=t@example.com -c user.name=t commit -q --allow-empty -m 'base moves'\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Carry case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["test -f CHANGED_MARKER.txt"],
        }),
    );
    assert!(task["baseRef"].is_string(), "task JSON: {task}");
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
    assert_eq!(settled["baseSha"], head.as_str(), "task JSON: {settled}");
    assert!(
        settled["decisions"]
            .to_string()
            .contains("carried the work onto"),
        "task JSON: {settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Fails verify until its brief carries the orchestrator's instruction;
/// as the orchestrator it answers the exhausted question with that fix.
const EXHAUSTED_TRIAGE_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-triage*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-triage"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-triage\n{\"action\":\"answer\",\"answer\":\"continue - create fixed.txt\",\"reason\":\"the verify only checks that file\"}\n```"}'
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > CHANGED_MARKER.txt
    case "$input" in *"continue - create fixed.txt"*) echo ok > fixed.txt ;; esac
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

#[test]
fn the_orchestrator_answers_attempts_keep_failing_and_the_task_passes() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        EXHAUSTED_TRIAGE_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["orchestrator"] = serde_json::json!("claude-sonnet");
    settings["autoAnswer"] = serde_json::json!(true);
    settings["maxAttempts"] = serde_json::json!(1);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Exhausted case",
            "goal": "Make verify pass",
            "criteria": [],
            "verify": ["test -f fixed.txt"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_until(&daemon, &task_id, Duration::from_secs(20), |s| {
        s == "done" || s == "waiting" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let decisions = settled["decisions"].to_string();
    assert!(
        decisions.contains("Orchestrator: Attempts keep failing")
            && decisions.contains("continue - create fixed.txt"),
        "{decisions}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Always reports `blocked`; triage always escalates with a sharpened
/// question and its own options.
const TRIAGE_ESCALATES_WITH_SHARPENED_QUESTION_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-triage*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-triage"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-triage\n{\"action\":\"escalate\",\"question\":\"Pick the auth approach: OAuth or API key?\",\"options\":[\"OAuth\",\"API key\"],\"reason\":\"repo supports both, no default\"}\n```"}'
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"blocked\",\"summary\":\"need input\",\"decisions\":[],\"question\":\"Which auth approach?\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

#[test]
fn triage_escalate_waits_for_the_owner_with_the_sharpened_question() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        TRIAGE_ESCALATES_WITH_SHARPENED_QUESTION_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["orchestrator"] = serde_json::json!("claude-sonnet");
    settings["autoAnswer"] = serde_json::json!(true);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Add auth",
            "goal": "Add authentication",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert_eq!(
        waiting["question"]["text"], "Pick the auth approach: OAuth or API key?",
        "the owner should see triage's sharpened question, not the original: {waiting}"
    );
    assert_eq!(
        waiting["question"]["options"],
        serde_json::json!(["OAuth", "API key"])
    );
    let decisions = waiting["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(|d| d
            .as_str()
            .unwrap_or("")
            .starts_with("Orchestrator: escalated (")),
        "expected an Orchestrator escalation decision: {decisions:?}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Reports `complete` on a change under a protected path -- the resulting
/// approval question must reach the owner completely untouched, even with
/// the orchestrator on and (per its triage branch) willing to answer.
const TRIAGE_POISON_IF_EVER_ASKED_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-triage*)
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-triage"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-triage\n{\"action\":\"answer\",\"answer\":\"approve\",\"reason\":\"should never be asked\"}\n```"}'
    printf '%s\n' "$json"
    ;;
  *)
    echo "changed" > PROTECTED.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
    printf '%s\n' "$json"
    ;;
esac
"#;

#[test]
fn protected_path_approval_is_never_triaged() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        TRIAGE_POISON_IF_EVER_ASKED_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["orchestrator"] = serde_json::json!("claude-sonnet");
    settings["autoAnswer"] = serde_json::json!(true);
    settings["protectedPaths"] = serde_json::json!(["PROTECTED.txt"]);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Touch a protected file",
            "goal": "Change something protected",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert!(
        waiting["question"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Change touches protected path"),
        "the protected-path question must reach the owner unmodified: {waiting}"
    );
    let decisions = waiting["decisions"].as_array().unwrap();
    assert!(
        !decisions
            .iter()
            .any(|d| d.as_str().unwrap_or("").starts_with("Orchestrator:")),
        "the orchestrator must never be asked to approve a protected path: {decisions:?}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn orchestrator_off_goes_straight_to_the_owner() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        TRIAGE_ESCALATES_WITH_SHARPENED_QUESTION_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    settings["autoAnswer"] = serde_json::json!(false);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Add auth",
            "goal": "Add authentication",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    // The script would escalate with a *different*, sharpened question if
    // triage ever ran -- the owner seeing the original proves it didn't.
    let waiting = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert_eq!(
        waiting["question"]["text"], "Which auth approach?",
        "orchestrator = \"\" must skip triage entirely: {waiting}"
    );
    let decisions = waiting["decisions"].as_array().unwrap();
    assert!(
        !decisions
            .iter()
            .any(|d| d.as_str().unwrap_or("").starts_with("Orchestrator:")),
        "no triage should have run at all: {decisions:?}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn a_task_interrupted_by_a_daemon_crash_resumes_on_restart() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("first-run");
    // The first run hangs (the daemon is killed under it); any later run
    // finishes the task.
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\nif [ ! -f {m} ]; then touch {m}; sleep 30; fi\necho changed > CHANGED_MARKER.txt\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}}'\necho '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"done\"}}'\n",
        m = marker.display()
    );
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", &body);
    let fake_bins = [("ORCHD_CLAUDE_BIN", script.to_str().unwrap())];
    let data_holder = tempfile::tempdir().unwrap();
    let data = data_holder.path().to_path_buf();

    let socket1 = data.join("orchd1.sock");
    let mut first = spawn_orchd_raw(&data, &socket1, &fake_bins);
    wait_for_socket(&socket1);
    let token = read_control_token(&data);
    let mut settings = request_on(
        &socket1,
        "settings.get",
        serde_json::json!({}),
        Some(&token),
    );
    settings["review"] = serde_json::json!("");
    fit_sandbox(&mut settings);
    request_on(
        &socket1,
        "settings.set",
        serde_json::json!({"settings": settings}),
        Some(&token),
    );
    let repo = init_git_repo();
    let task = request_on(
        &socket1,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(),
        "title": "Survives a crash", "goal": "g", "criteria": [], "verify": ["true"], "start": true}),
        Some(&token),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    while !marker.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "harness never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    first.kill().unwrap();
    let _ = first.wait();

    let socket2 = data.join("orchd2.sock");
    let second = spawn_orchd_raw(&data, &socket2, &fake_bins);
    wait_for_socket(&socket2);
    let token = read_control_token(&data);
    let call = |m: &str, p: serde_json::Value| request_on(&socket2, m, p, Some(&token));
    let start = Instant::now();
    let settled = loop {
        let t = call("task.get", serde_json::json!({"id": id}));
        if t["status"] == "done" {
            break t;
        }
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "task did not resume after restart: {t}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(settled["attempts"][0]["status"], "interrupted");
    let _ = call("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(second, Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
}

/// Records each attempt's brief under `$MSG_DIR` and holds the first attempt
/// open until the test creates `$MSG_DIR/release`; only the second attempt
/// writes the file verify looks for.
const HOLD_FIRST_ATTEMPT_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
if [ ! -f "$MSG_DIR/first.md" ]; then
  printf '%s' "$input" > "$MSG_DIR/first.md"
  while [ ! -f "$MSG_DIR/release" ]; do sleep 0.1; done
else
  printf '%s' "$input" > "$MSG_DIR/second.md"
  echo pass > PASS.txt
fi
echo changed > CHANGED_MARKER.txt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"done"}'
"#;

#[test]
fn a_message_reaches_a_running_task_with_its_next_attempt_only() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        HOLD_FIRST_ATTEMPT_SCRIPT,
    );
    let msg_dir = scripts_dir.path().to_str().unwrap().to_string();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("MSG_DIR", msg_dir.as_str()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();
    let sender = daemon.request(
        "task.create",
        serde_json::json!({"repo": repo_path, "title": "Sender", "goal": "g"}),
    );
    let receiver = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo_path,
            "title": "Receiver",
            "goal": "g",
            "verify": ["test -f PASS.txt"],
            "start": true,
        }),
    );
    let (from, to) = (&sender["id"], &receiver["id"]);
    let first = scripts_dir.path().join("first.md");
    let start = Instant::now();
    while !first.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "the first attempt never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // Sent while attempt 1 runs: kept, not delivered, attempt untouched.
    let note = serde_json::json!({"from": from, "to": to, "text": "PEER-NOTE-42"});
    let sent = daemon.request("message.send", note);
    assert_eq!(sent["delivered"], false);
    let inbox = daemon.request("message.inbox", serde_json::json!({"id": to}));
    assert_eq!(inbox[0]["delivered"], false);
    let running = daemon.request("task.get", serde_json::json!({"id": to}));
    assert_eq!(running["status"], "running", "{running}");
    assert_eq!(running["attempts"].as_array().unwrap().len(), 1);
    let first_brief = std::fs::read_to_string(&first).unwrap();
    assert!(first_brief.contains("Your task id is"));
    assert!(!first_brief.contains("PEER-NOTE-42"));

    std::fs::write(scripts_dir.path().join("release"), "").unwrap();
    let settled = poll_task_status(&daemon, to.as_str().unwrap(), Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{settled}");
    // Attempt 1 ran to its own end (verify failed): never stopped or rerun.
    assert_eq!(attempts[0]["failure"]["kind"], "verify");
    let second_brief = std::fs::read_to_string(scripts_dir.path().join("second.md")).unwrap();
    assert!(second_brief.contains("PEER-NOTE-42"), "{second_brief}");
    let inbox = daemon.request("message.inbox", serde_json::json!({"id": to}));
    assert_eq!(inbox[0]["delivered"], true);

    daemon.shutdown_and_wait();
    for t in [&sender, &receiver] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}
