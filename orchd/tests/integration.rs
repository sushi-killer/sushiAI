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

#[test]
fn settings_defaults_reports_the_built_in_defaults_not_the_saved_settings() {
    let daemon = Daemon::spawn(&[]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["planner"] = serde_json::json!("claude-sonnet");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let defaults = daemon.request("settings.defaults", serde_json::json!({}));
    assert_eq!(defaults["planner"], "claude-opus");
    let saved = daemon.request("settings.get", serde_json::json!({}));
    assert_eq!(saved["planner"], "claude-sonnet");

    daemon.shutdown_and_wait();
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
    // No `task.start`: the full form starts by default, like the request form.

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
      json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"tier\":\"hard\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"true\"],\"questions\":[]}\n```"}'
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
    assert!(
        !decisions
            .iter()
            .any(|d| d.as_str().unwrap_or("").contains("tier unavailable")),
        "a confident classifier answer must not fall back: {decisions:?}"
    );
    assert!(settled.get("tierFallback").is_none(), "{settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// A low-confidence classifier answer (p < 0.5) must not be routed on the
/// classified tier -- it falls back to standard and records why, without
/// ever writing the misleading `Jev: tier hard (p 0.45)` line (the
/// pre-fallback behaviour this test guards against, docs/orchd-acceptance-audit.md G8).
#[test]
fn jev_tier_falls_back_to_standard_on_a_low_confidence_answer() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho \"changed\" > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    // A p 0.45 "hard" answer falls back to standard, which routes to the
    // Claude harness faked above (the default `tiers` map) -- "hard" would
    // route to Claude Opus and still work, but standard is what the
    // fallback path must land on regardless of the classifier's choice.
    let base_url = spawn_fake_openai_classifier(
        r#"{"answers":{"tier":{"choice":"hard","probabilities":{"mechanical":0.05,"standard":0.5,"hard":0.45}}}}"#,
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
            "title": "Unsure case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": ["true"],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(settled["tier"], "standard", "{settled}");
    assert_eq!(
        settled["attempts"][0]["routeId"], "claude-sonnet",
        "{settled}"
    );
    let decisions = settled["decisions"].as_array().unwrap();
    assert!(
        decisions
            .iter()
            .any(|d| d
                == "Jev: tier unavailable (unsure: hard p 0.45) -> fallback standard, route claude-sonnet"),
        "expected a tier-unavailable fallback decision: {decisions:?}"
    );
    assert!(
        !decisions
            .iter()
            .any(|d| d.as_str().unwrap_or("").starts_with("Jev: tier hard")),
        "must not write the misleading pre-fallback line: {decisions:?}"
    );
    assert_eq!(settled["tierFallback"], "unsure: hard p 0.45", "{settled}");

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
        serde_json::json!({"repo": repo_path, "title": "Sender", "goal": "g", "start": false}),
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

#[test]
fn a_silent_harness_is_stopped_as_stalled_when_the_variant_sets_a_stall_timeout() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude-silent.sh",
        "#!/bin/sh\necho $$ > \"$PID_FILE\"\ncat > /dev/null\nexec sleep 90\n",
    );
    let pid_file = scripts_dir.path().join("harness.pid");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PID_FILE", pid_file.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(1);
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Hangs",
            "goal": "Never answers",
            "verify": ["true"],
            "variant": {"stallTimeoutSecs": 8},
            "start": true,
        }),
    );
    assert_eq!(task["variant"]["stallTimeoutSecs"], 8, "{task}");
    let task_id = task["id"].as_str().unwrap().to_string();

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(90));
    assert_eq!(settled["status"], "waiting", "task JSON: {settled}");
    assert_eq!(
        settled["attempts"][0]["failure"]["kind"], "stall",
        "{settled}"
    );
    let pid: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let start = Instant::now();
    while is_alive(pid) {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "stalled harness {pid} is still running"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn task_create_rejects_an_unknown_variant_flag() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let result = raw_request_with_params(
        &daemon.socket,
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "t",
            "goal": "g",
            "variant": {"retrymode": "fresh"},
        }),
        Some(&daemon.token),
    );
    assert!(
        result
            .to_string()
            .contains("unknown variant flag: retrymode"),
        "{result}"
    );
    daemon.shutdown_and_wait();
}

/// First attempt leaves FIRST (verify wants SECOND) and a handoff; the
/// retry creates SECOND. Every invocation's argv goes to $ARGS_LOG.
const FAKE_RETRY_SCRIPT: &str = r#"#!/bin/sh
cat > /dev/null
printf '%s\n' "$*" >> "$ARGS_LOG"
if [ -f FIRST ]; then echo x > SECOND; else echo x > FIRST; fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"handoff\":\"tried the quick fix\",\"decisions\":[],\"question\":\"\"}\n```"}'
printf '%s\n' "$json"
"#;

#[test]
fn a_fresh_retry_starts_a_new_session_that_reads_the_earlier_handoff() {
    for (mode, expect_resume) in [("fresh", false), ("resume", true)] {
        let scripts_dir = tempfile::tempdir().unwrap();
        let script = fake_harness_script(scripts_dir.path(), "fake-retry.sh", FAKE_RETRY_SCRIPT);
        let args_log = scripts_dir.path().join("args.log");
        let daemon = Daemon::spawn(&[
            ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
            ("ARGS_LOG", args_log.to_str().unwrap()),
        ]);
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["review"] = serde_json::json!("");
        daemon.request("settings.set", serde_json::json!({"settings": settings}));

        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "Two tries",
                "goal": "Needs a second attempt",
                "verify": ["test -f SECOND"],
                "variant": {"retryMode": mode},
                "start": true,
            }),
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
        assert_eq!(settled["status"], "done", "{mode}: {settled}");
        let attempts = settled["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 2, "{mode}: {settled}");
        assert_eq!(attempts[0]["handoff"], "tried the quick fix", "{mode}");
        assert_eq!(attempts[1]["resumed"], expect_resume, "{mode}: {settled}");

        let argv: Vec<String> = std::fs::read_to_string(&args_log)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(
            argv[1].contains("--resume sess-fake"),
            expect_resume,
            "{mode}: {argv:?}"
        );
        if !expect_resume {
            let brief = std::fs::read_to_string(
                daemon
                    .data_dir()
                    .join("tasks")
                    .join(&task_id)
                    .join("runs/2/brief.md"),
            )
            .unwrap();
            assert!(brief.contains("## Task"), "not the full brief: {brief}");
            assert!(brief.contains("handoff: tried the quick fix"), "{brief}");
            assert!(brief.contains("test -f SECOND exited"), "{brief}");
        }

        let worktree = task["worktree"].as_str().unwrap().to_string();
        daemon.shutdown_and_wait();
        let _ = std::fs::remove_dir_all(worktree);
    }
}

/// Like FAKE_RETRY_SCRIPT, but it also answers the advisor: a brief that
/// says an attempt failed gets $ADVISOR_MODE ("ok" answers, "fail" exits 1
/// with no output). Every run's brief is kept as $LOG_DIR/brief.<pid>.
const FAKE_ADVISOR_SCRIPT: &str = r#"#!/bin/sh
cat > "$LOG_DIR/brief.$$"
if grep -q 'An implement attempt at this task failed' "$LOG_DIR/brief.$$"; then
  echo advisor >> "$LOG_DIR/advisor.log"
  if [ "$ADVISOR_MODE" = fail ]; then exit 1; fi
  printf '%s\n' '{"type":"result","total_cost_usd":0.03,"usage":{"input_tokens":1,"output_tokens":1},"result":"Create SECOND, not FIRST."}'
  exit 0
fi
if [ -f FIRST ]; then echo x > SECOND; else echo x > FIRST; fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"handoff\":\"quick fix\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

#[test]
fn the_advisor_runs_once_before_a_retry_and_its_advice_reaches_the_next_brief() {
    for (advisor, mode) in [(true, "ok"), (true, "fail"), (false, "ok")] {
        let scripts_dir = tempfile::tempdir().unwrap();
        let script =
            fake_harness_script(scripts_dir.path(), "fake-advisor.sh", FAKE_ADVISOR_SCRIPT);
        let daemon = Daemon::spawn(&[
            ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
            ("LOG_DIR", scripts_dir.path().to_str().unwrap()),
            ("ADVISOR_MODE", mode),
        ]);
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["review"] = serde_json::json!("");
        daemon.request("settings.set", serde_json::json!({"settings": settings}));

        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "Two tries",
                "goal": "Needs a second attempt",
                "verify": ["test -f SECOND"],
                "variant": {"advisor": advisor, "retryMode": "fresh"},
                "start": true,
            }),
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
        let label = format!("advisor={advisor} mode={mode}: {settled}");
        assert_eq!(settled["status"], "done", "{label}");
        let attempts = settled["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 2, "{label}");
        let runs = std::fs::read_to_string(scripts_dir.path().join("advisor.log"))
            .map(|t| t.lines().count())
            .unwrap_or(0);
        let brief = std::fs::read_to_string(
            daemon
                .data_dir()
                .join("tasks")
                .join(&task_id)
                .join("runs/2/brief.md"),
        )
        .unwrap();
        let cost = settled["costUsd"].as_f64().unwrap();
        if advisor && mode == "ok" {
            assert_eq!(runs, 1, "{label}");
            assert_eq!(
                attempts[0]["advice"], "Create SECOND, not FIRST.",
                "{label}"
            );
            assert!(attempts[1].get("advice").is_none(), "{label}");
            assert!(
                brief.contains("## Advisor") && brief.contains("Create SECOND, not FIRST."),
                "{brief}"
            );
            // Two implement runs plus the advisor; the advisor's cost is on
            // no attempt.
            assert!((cost - 0.05).abs() < 1e-9, "{label}");
            assert_eq!(attempts[0]["costUsd"], 0.01, "{label}");
        } else if advisor {
            assert_eq!(runs, 1, "{label}");
            assert!(attempts[0].get("advice").is_none(), "{label}");
            assert!(!brief.contains("## Advisor"), "{brief}");
        } else {
            assert_eq!(runs, 0, "{label}");
            assert!(!brief.contains("## Advisor"), "{brief}");
            assert!((cost - 0.02).abs() < 1e-9, "{label}");
        }

        let worktree = task["worktree"].as_str().unwrap().to_string();
        daemon.shutdown_and_wait();
        let _ = std::fs::remove_dir_all(worktree);
    }
}

#[test]
fn a_blind_review_brief_leaves_out_the_implementer_s_account() {
    for blind in [false, true] {
        let scripts_dir = tempfile::tempdir().unwrap();
        let claude = fake_harness_script(
            scripts_dir.path(),
            "fake-claude.sh",
            "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
        );
        let args_log = scripts_dir.path().join("codex-args");
        let codex = fake_harness_script(
            scripts_dir.path(),
            "fake-codex.sh",
            "#!/bin/sh\ncat > \"$CODEX_ARGS.brief\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"```sushi-review\\n{\\\"verdict\\\":\\\"PASS\\\",\\\"findings\\\":[]}\\n```\"}}'\n",
        );
        let daemon = Daemon::spawn(&[
            ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
            ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
            ("CODEX_ARGS", args_log.to_str().unwrap()),
        ]);
        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "Blind",
                "goal": "Write a marker",
                "verify": ["true"],
                "variant": {"reviewOtherFamily": true, "reviewBlind": blind},
                "start": true,
            }),
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
        assert_eq!(settled["status"], "done", "{settled}");
        let brief = std::fs::read_to_string(format!("{}.brief", args_log.display())).unwrap();
        assert!(brief.contains("## Diff"), "{brief}");
        assert_eq!(brief.contains("## Implementer"), !blind, "{brief}");
        assert_eq!(brief.contains("<untrusted-data>\ndone"), !blind, "{brief}");

        let worktree = task["worktree"].as_str().unwrap().to_string();
        daemon.shutdown_and_wait();
        let _ = std::fs::remove_dir_all(worktree);
    }
}

#[test]
fn the_planner_s_tier_routes_the_task_only_when_the_variant_asks_for_it() {
    for (planner_tier, route) in [(true, "claude-opus"), (false, "claude-sonnet")] {
        let scripts_dir = tempfile::tempdir().unwrap();
        let script =
            fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
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
                "variant": {"plannerTier": planner_tier},
                "start": true,
            }),
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
            s == "done" || s == "failed" || s == "stopped" || s == "waiting"
        });
        assert_eq!(settled["status"], "done", "{settled}");
        assert_eq!(settled["plannedTier"], "hard", "{settled}");
        // The plan run's cost counts toward the task, not only the implement run's.
        assert_eq!(settled["attempts"][0]["costUsd"], 0.01, "{settled}");
        assert!(
            (settled["costUsd"].as_f64().unwrap() - 0.02).abs() < 1e-9,
            "{settled}"
        );
        assert_eq!(settled["attempts"][1]["routeId"], route, "{settled}");
        let decisions = settled["decisions"].as_array().unwrap();
        let noted = decisions
            .iter()
            .any(|d| d == "Planner: tier hard -> route claude-opus");
        assert_eq!(noted, planner_tier, "{settled}");
        // With `plannerTier: false` and no classifier key configured (the
        // default), the tier falls back to standard instead of the
        // planner's "hard" -- `plannedTier` above still records the
        // planner's choice, just unused for routing.
        let fell_back = decisions.iter().any(|d| {
            d == "Jev: tier unavailable (no classifier key) -> fallback standard, route claude-sonnet"
        });
        assert_eq!(fell_back, !planner_tier, "{settled}");
        if planner_tier {
            assert!(settled.get("tierFallback").is_none(), "{settled}");
        } else {
            assert_eq!(settled["tierFallback"], "no classifier key", "{settled}");
        }

        let worktree = task["worktree"].as_str().unwrap().to_string();
        daemon.shutdown_and_wait();
        let _ = std::fs::remove_dir_all(worktree);
    }
}

#[test]
fn the_stall_clock_waits_while_the_stop_hook_runs_verify() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude-hook.js",
        FAKE_CLAUDE_HOOK_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    // The hook's verify is silent for 12s, well past the 5s stall timeout;
    // 5s leaves room for a loaded machine to start the fake harness.
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Slow hook",
            "goal": "Make a trivial change",
            "verify": ["sleep 12 && test -f verified-marker.txt"],
            "variant": {"stallTimeoutSecs": 5},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(60));
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    assert_eq!(
        settled["attempts"].as_array().unwrap().len(),
        1,
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Implements like the other fakes; as reviewer, answers PASS but marks a
/// criterion unmet when the brief asks for a ruling per criterion.
const FAKE_CONTRACT_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Review"*)
  case "$brief" in
  *"Rule on every acceptance criterion"*)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[],\"criteria\":[{\"criterion\":\"Marker exists\",\"met\":false,\"evidence\":\"wrong file\"}]}\n```"}' ;;
  *)
    json='{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
  esac
  printf '%s\n' "$json" ;;
*)
  echo changed > CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
  printf '%s\n' "$json" ;;
esac
"###;

#[test]
fn with_a_contract_the_reviewer_s_unmet_criterion_fails_the_attempt() {
    for (contract, status) in [(true, "waiting"), (false, "done")] {
        let scripts_dir = tempfile::tempdir().unwrap();
        let script =
            fake_harness_script(scripts_dir.path(), "fake-contract.sh", FAKE_CONTRACT_SCRIPT);
        let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["maxAttempts"] = serde_json::json!(1);
        // Review by the Claude hard route, which is the same fake.
        settings["review"] = serde_json::json!("claude-opus");
        daemon.request("settings.set", serde_json::json!({"settings": settings}));

        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "Contract",
                "goal": "Write the marker",
                "criteria": ["Marker exists -- check: CHANGED_MARKER.txt"],
                "verify": ["true"],
                "variant": {"contract": contract},
                "start": true,
            }),
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
        assert_eq!(settled["status"], status, "contract={contract}: {settled}");
        if contract {
            let attempt = &settled["attempts"][0];
            assert_eq!(attempt["failure"]["kind"], "review", "{settled}");
            assert_eq!(
                attempt["review"]["findings"][0], "Unmet criterion: Marker exists (wrong file)",
                "{settled}"
            );
        }

        let worktree = task["worktree"].as_str().unwrap().to_string();
        daemon.shutdown_and_wait();
        let _ = std::fs::remove_dir_all(worktree);
    }
}

#[test]
fn the_contract_flag_reaches_the_plan_brief() {
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
            "variant": {"contract": true},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    let brief = std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(&task_id)
            .join("runs/1/plan/brief.md"),
    )
    .unwrap();
    assert!(brief.contains("-- check:"), "{brief}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn review_other_family_sends_claude_work_to_the_codex_reviewer() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let marker = scripts_dir.path().join("codex-reviewed");
    let codex = fake_harness_script(
        scripts_dir.path(),
        "fake-codex.sh",
        "#!/bin/sh\ncat > /dev/null\ntouch \"$CODEX_MARKER\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"```sushi-review\\n{\\\"verdict\\\":\\\"PASS\\\",\\\"findings\\\":[]}\\n```\"}}'\n",
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
        ("CODEX_MARKER", marker.to_str().unwrap()),
    ]);
    // review stays "auto": Sonnet's work would go to the Claude hard route.
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Other family",
            "goal": "Write the marker",
            "verify": ["true"],
            "variant": {"reviewOtherFamily": true},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(
        settled["attempts"][0]["routeId"], "claude-sonnet",
        "{settled}"
    );
    assert!(marker.exists(), "the review never ran on Codex: {settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn review_evidence_attaches_the_attempt_s_screenshots_to_the_codex_reviewer() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nmkdir -p artifacts\necho png > artifacts/after.png\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let args_log = scripts_dir.path().join("codex-args");
    let codex = fake_harness_script(
        scripts_dir.path(),
        "fake-codex.sh",
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$CODEX_ARGS\"\ncat > \"$CODEX_ARGS.brief\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"```sushi-review\\n{\\\"verdict\\\":\\\"PASS\\\",\\\"findings\\\":[]}\\n```\"}}'\n",
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
        ("CODEX_ARGS", args_log.to_str().unwrap()),
    ]);
    let repo = init_git_repo();
    std::fs::write(repo.path().join(".gitignore"), "artifacts/\n").unwrap();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Evidence",
            "goal": "Save a screenshot",
            "verify": ["true"],
            "variant": {"reviewOtherFamily": true, "reviewEvidence": true},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");
    let argv = std::fs::read_to_string(&args_log).unwrap();
    assert!(
        argv.starts_with("exec --image=") && argv.contains("artifacts/after.png"),
        "{argv}"
    );
    let brief = std::fs::read_to_string(format!("{}.brief", args_log.display())).unwrap();
    assert!(
        brief.contains("## Screenshots") && brief.contains("`artifacts/after.png`"),
        "{brief}"
    );
    assert!(
        brief.contains("The implementer's own account") && brief.contains("<untrusted-data>\ndone"),
        "the reviewer hears the implementer's summary: {brief}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn final_checks_run_once_after_review_passes_and_gate_the_commit() {
    for (final_check, status) in [("false", "waiting"), ("true", "done")] {
        let scripts_dir = tempfile::tempdir().unwrap();
        // Implements, and as the (Claude) reviewer answers a plain PASS.
        let script =
            fake_harness_script(scripts_dir.path(), "fake-contract.sh", FAKE_CONTRACT_SCRIPT);
        let ran_log = scripts_dir.path().join("final.log");
        let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["maxAttempts"] = serde_json::json!(1);
        settings["review"] = serde_json::json!("claude-opus");
        daemon.request("settings.set", serde_json::json!({"settings": settings}));

        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "Final checks",
                "goal": "Write the marker",
                "verify": ["true"],
                "finalVerify": [format!("echo ran >> {}; {final_check}", ran_log.display())],
                "start": true,
            }),
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
        assert_eq!(settled["status"], status, "{final_check}: {settled}");
        let attempt = &settled["attempts"][0];
        assert_eq!(attempt["review"]["verdict"], "PASS", "{settled}");
        if final_check == "false" {
            assert!(
                attempt["failure"]["detail"]
                    .as_str()
                    .unwrap()
                    .starts_with("Final check echo ran"),
                "{settled}"
            );
        }
        let worktree = task["worktree"].as_str().unwrap().to_string();
        let ran = std::fs::read_to_string(&ran_log).unwrap();
        assert_eq!(ran.lines().count(), 1, "the final check ran exactly once");
        let brief = std::fs::read_to_string(
            daemon
                .data_dir()
                .join("tasks")
                .join(&task_id)
                .join("runs/1/brief.md"),
        )
        .unwrap();
        assert!(
            brief.contains("## Final checks") && brief.contains("do not run them yourself"),
            "{brief}"
        );

        daemon.shutdown_and_wait();
        let _ = std::fs::remove_dir_all(&worktree);
    }
}

#[test]
fn a_failed_final_check_shows_the_failing_test_not_the_bundler_noise() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-contract.sh", FAKE_CONTRACT_SCRIPT);
    let check = fake_harness_script(
        scripts_dir.path(),
        "noisy-check.sh",
        "#!/bin/sh\ni=0\nwhile [ $i -lt 300 ]; do echo \"WARNING bundler chunk $i is large\" >&2; i=$((i+1)); done\necho 'test a_broken_thing ... FAILED'\necho \"thread 'a_broken_thing' panicked at t.rs:3:5:\"\necho 'assertion failed: boom_message'\nj=0\nwhile [ $j -lt 300 ]; do echo \"test fine_$j ... ok\"; j=$((j+1)); done\nexit 1\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["maxAttempts"] = serde_json::json!(1);
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Noisy final check",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [check.to_str().unwrap()],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(30));
    assert_eq!(settled["status"], "waiting", "{settled}");
    let detail = settled["attempts"][0]["failure"]["detail"]
        .as_str()
        .unwrap();
    assert!(detail.contains("a_broken_thing ... FAILED"), "{detail}");
    assert!(detail.contains("boom_message"), "{detail}");
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(&worktree);
}

/// Implements like the other fakes; as reviewer, answers PASS and reports a
/// cost of its own via `total_cost_usd`.
const FAKE_PRICED_REVIEWER_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Review"*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.05,"result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*)
  echo changed > CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

#[test]
fn review_cost_lands_on_the_attempt_s_review_cost_usd_and_the_task_total() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-priced-reviewer.sh",
        FAKE_PRICED_REVIEWER_SCRIPT,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Priced review",
            "goal": "Write the marker",
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");
    let attempt = &settled["attempts"][0];
    assert_eq!(attempt["review"]["verdict"], "PASS", "{settled}");
    // The review's cost sits on the implement attempt's own reviewCostUsd,
    // never folded into that attempt's costUsd (attempt_cost() subtracts
    // costUsd from earlier attempts on a resumed session).
    assert_eq!(attempt["reviewCostUsd"], 0.05, "{settled}");
    assert_eq!(attempt["costUsd"], 0.01, "{settled}");
    assert!(
        (settled["costUsd"].as_f64().unwrap() - 0.06).abs() < 1e-9,
        "{settled}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

const FAKE_CLAUDE_PASS: &str = "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CLAUDE_ARGS\"\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n";

fn run_file(daemon: &Daemon, task_id: &str, name: &str) -> String {
    std::fs::read_to_string(
        daemon
            .data_dir()
            .join("tasks")
            .join(task_id)
            .join("runs/1")
            .join(name),
    )
    .unwrap()
}

/// The prompt-prefix trim (`--disallowedTools`) used to be gated behind
/// `variant.leanContext`; it is now unconditional for every Claude implement
/// run, and mcp.json carries exactly the configured servers -- no Jev
/// filtering, no skills hook wired into `settings.json`.
#[test]
fn a_claude_implement_run_trims_delegation_tools_and_keeps_the_configured_mcp_servers() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", FAKE_CLAUDE_PASS);
    let args = scripts_dir.path().join("args");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("CLAUDE_ARGS", args.to_str().unwrap()),
    ]);
    // Skip review: FAKE_CLAUDE_PASS's report has no PASS/FAIL verdict, and
    // this test only cares about the implement attempt's argv/settings/mcp.
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Trimmed prefix",
            "goal": "Change the widget",
            "verify": ["true"],
            "mcp": {"mcpServers": {"relevant": {"command": "r"}, "irrelevant": {"command": "i"}}},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");

    let argv = std::fs::read_to_string(&args).unwrap();
    assert!(argv.contains("--disallowedTools"), "{argv}");
    let settings: serde_json::Value =
        serde_json::from_str(&run_file(&daemon, &task_id, "settings.json")).unwrap();
    let hooks: Vec<&String> = settings["hooks"].as_object().unwrap().keys().collect();
    assert_eq!(hooks, ["Stop"], "{settings}");
    let mcp: serde_json::Value =
        serde_json::from_str(&run_file(&daemon, &task_id, "mcp.json")).unwrap();
    assert_eq!(
        mcp,
        serde_json::json!({"mcpServers": {
            "relevant": {"command": "r"},
            "irrelevant": {"command": "i"},
            "sushiai-messages": mcp["mcpServers"]["sushiai-messages"].clone(),
        }})
    );
    assert!(mcp["mcpServers"]["sushiai-messages"].is_object());

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

/// Runs `orchd eval run` against `daemon` from `repo`; `(exit ok, stdout, stderr)`.
fn eval_run(daemon: &Daemon, repo: &Path, extra: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_orchd"))
        .args(["eval", "run", "--data"])
        .arg(daemon.data_dir())
        .arg("--socket")
        .arg(&daemon.socket)
        .arg("--repo")
        .arg(repo)
        .args(extra)
        .output()
        .expect("run orchd eval");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn git_out(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn eval_run_creates_one_task_at_the_resolved_base_with_its_eval_fields() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    std::fs::write(repo.path().join("second.txt"), "2\n").unwrap();
    git_out(repo.path(), &["add", "."]);
    git_out(repo.path(), &["commit", "-q", "-m", "second"]);
    let parent = git_out(repo.path(), &["rev-parse", "HEAD^"]);

    let sets = tempfile::tempdir().unwrap();
    let set = sets.path().join("set-x.json");
    std::fs::write(
        &set,
        serde_json::json!({"tasks": [
            {"name": "one", "base": "HEAD^", "request": "add dark mode"},
            {"name": "two", "base": "HEAD", "request": "something else"},
        ]})
        .to_string(),
    )
    .unwrap();

    let (ok, stdout, stderr) = eval_run(
        &daemon,
        repo.path(),
        &[
            "--set",
            set.to_str().unwrap(),
            "--only",
            "one",
            "--variant",
            r#"{"retryMode":"fresh"}"#,
        ],
    );
    assert!(ok, "stderr: {stderr}");
    let created: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(created["tasks"].as_array().unwrap().len(), 1, "{stdout}");
    let id = created["tasks"][0]["id"].as_str().unwrap();

    let list = daemon.request("task.list", serde_json::json!({}));
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    let task = daemon.request("task.get", serde_json::json!({"id": id}));
    assert_eq!(task["baseSha"], parent.as_str(), "{task}");
    assert_eq!(task["evalSet"], "set-x");
    assert_eq!(task["evalName"], "one");
    assert_eq!(task["variant"]["retryMode"], "fresh");

    // Ad-hoc pair: one task per arm, same request and base, one shared name.
    let (ok, stdout, stderr) = eval_run(
        &daemon,
        repo.path(),
        &[
            "--request",
            "try the thing",
            "--arms",
            r#"[{"retryMode":"fresh"},{"contract":true}]"#,
        ],
    );
    assert!(ok, "stderr: {stderr}");
    let created: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let tasks = created["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2, "{stdout}");
    assert_eq!(tasks[0]["evalSet"], "adhoc");
    assert_eq!(tasks[0]["evalName"], tasks[1]["evalName"]);
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);
    for t in tasks {
        let full = daemon.request("task.get", serde_json::json!({"id": t["id"]}));
        assert_eq!(full["baseSha"], head.as_str());
    }

    let worktrees: Vec<String> = daemon
        .request("task.list", serde_json::json!({}))
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["worktree"].as_str().unwrap().to_string())
        .collect();
    daemon.shutdown_and_wait();
    for w in worktrees {
        let _ = std::fs::remove_dir_all(w);
    }
}

#[test]
fn variant_route_overrides_pick_the_planner_and_the_tier_s_implement_route() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    // Settings plan and implement `hard` on claude-opus; this task moves both
    // to claude-sonnet. The planner's tier (hard) routes it.
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "variant": {
                "plannerTier": true,
                "plannerRoute": "claude-sonnet",
                "tierRoutes": {"hard": "claude-sonnet"},
            },
            "start": true,
        }),
    );
    assert_eq!(task["variant"]["plannerRoute"], "claude-sonnet", "{task}");
    assert_eq!(
        task["variant"]["tierRoutes"],
        serde_json::json!({"hard": "claude-sonnet"}),
        "{task}"
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(settled["tier"], "hard", "{settled}");
    assert_eq!(settled["attempts"][0]["stage"], "plan", "{settled}");
    assert_eq!(
        settled["attempts"][0]["routeId"], "claude-sonnet",
        "{settled}"
    );
    assert_eq!(settled["attempts"][1]["stage"], "implement", "{settled}");
    assert_eq!(
        settled["attempts"][1]["routeId"], "claude-sonnet",
        "{settled}"
    );
    let decisions = settled["decisions"].as_array().unwrap();
    for line in [
        "Variant: planner -> route claude-sonnet (override)",
        "Variant: tier hard -> route claude-sonnet (override)",
        "Planner: tier hard -> route claude-sonnet",
    ] {
        assert!(decisions.iter().any(|d| d == line), "{line}: {settled}");
    }

    // The override was this task's only: the next one plans on the settings'
    // planner and records no override.
    let other = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": false,
        }),
    );
    assert!(other["variant"].get("plannerRoute").is_none(), "{other}");
    assert!(other["variant"].get("tierRoutes").is_none(), "{other}");
    let other_id = other["id"].as_str().unwrap().to_string();
    let other_settled = poll_until(&daemon, &other_id, Duration::from_secs(15), |s| {
        s == "stopped" || s == "done" || s == "failed"
    });
    assert_eq!(
        other_settled["attempts"][0]["routeId"], "claude-opus",
        "{other_settled}"
    );
    assert!(
        !other_settled["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d.as_str().unwrap().starts_with("Variant:")),
        "{other_settled}"
    );

    daemon.shutdown_and_wait();
    for t in [&task, &other] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn eval_run_with_an_unknown_name_or_an_unresolvable_base_creates_nothing() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let sets = tempfile::tempdir().unwrap();
    let set = sets.path().join("set-y.json");
    std::fs::write(
        &set,
        serde_json::json!({"tasks": [
            {"name": "good", "base": "HEAD", "request": "r"},
            {"name": "bad-base", "base": "no-such-ref^", "request": "r"},
        ]})
        .to_string(),
    )
    .unwrap();

    let (ok, _, stderr) = eval_run(
        &daemon,
        repo.path(),
        &["--set", set.to_str().unwrap(), "--only", "good,nope"],
    );
    assert!(!ok);
    assert!(
        stderr.contains("unknown task name in --only: nope"),
        "{stderr}"
    );

    let (ok, _, stderr) = eval_run(&daemon, repo.path(), &["--set", set.to_str().unwrap()]);
    assert!(!ok);
    assert!(
        stderr.contains("bad-base") && stderr.contains("does not resolve"),
        "{stderr}"
    );

    let list = daemon.request("task.list", serde_json::json!({}));
    assert!(list.as_array().unwrap().is_empty(), "{list}");
    daemon.shutdown_and_wait();
}

#[test]
fn task_create_rejects_a_route_override_naming_an_unknown_route_and_creates_nothing() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    for (variant, expected) in [
        (
            serde_json::json!({"tierRoutes": {"hard": "claude-nope"}}),
            "variant tierRoutes.hard \"claude-nope\" is not a configured route",
        ),
        (
            serde_json::json!({"plannerRoute": "claude-nope"}),
            "variant plannerRoute \"claude-nope\" is not a configured route",
        ),
    ] {
        let result = raw_request_with_params(
            &daemon.socket,
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "t",
                "goal": "g",
                "variant": variant,
            }),
            Some(&daemon.token),
        );
        assert_eq!(result["error"]["message"], expected, "{result}");
    }
    let tasks = daemon.request("task.list", serde_json::json!({}));
    assert_eq!(tasks, serde_json::json!([]), "{tasks}");
    let worktrees = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&worktrees.stdout);
    assert_eq!(listed.matches("worktree ").count(), 1, "{listed}");
    daemon.shutdown_and_wait();
}

/// A fake harness for `repo.audit`: records its argv and brief next to the
/// script (never in the audited repo), then replies with `reply` as its
/// final result.
fn fake_audit_harness(dir: &Path, reply: &str) -> PathBuf {
    let result = serde_json::json!({
        "type": "result",
        "total_cost_usd": 0.42,
        "usage": {"input_tokens": 10, "output_tokens": 5},
        "result": reply,
    });
    let body = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > {d}/argv.txt\ncat > {d}/brief.md\ncat <<'JSON'\n{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-audit\"}}\n{result}\nJSON\n",
        d = dir.display()
    );
    fake_harness_script(dir, "fake-claude-audit.sh", &body)
}

fn poll_audit(daemon: &Daemon, id: &str) -> serde_json::Value {
    let start = Instant::now();
    loop {
        let audit = daemon.request("repo.audit.get", serde_json::json!({"id": id}));
        if audit["status"] != "running" {
            return audit;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "audit did not settle: {audit}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ---------------------------------------------------------------------------
// Task graph: dependsOn, parents, landing
// ---------------------------------------------------------------------------

fn settle(daemon: &Daemon, task_id: &str) -> serde_json::Value {
    poll_until(daemon, task_id, Duration::from_secs(30), |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    })
}

fn review_off(daemon: &Daemon) {
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
}

fn implement_attempts(task: &serde_json::Value) -> Vec<serde_json::Value> {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .cloned()
        .collect()
}

/// Plans a top-level request as two parts (B after A), plans each part as
/// one task, and implements each part by writing its file.
const GRAPH_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"'
case "$input" in
  *sushi-plan*)
    case "$input" in
      *PART_A*) out '"```sushi-plan\n{\"title\":\"Part A\",\"goal\":\"PART_A: create a.txt\",\"verify\":[\"test -f a.txt\"]}\n```"' ;;
      *PART_B*) out '"```sushi-plan\n{\"title\":\"Part B\",\"goal\":\"PART_B: create b.txt next to a.txt\",\"verify\":[\"test -f a.txt && test -f b.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"Both parts\",\"goal\":\"Create a.txt, then b.txt\",\"verify\":[\"test -f a.txt && test -f b.txt\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"request\":\"PART_A create a.txt\"},{\"key\":\"b\",\"title\":\"Part B\",\"request\":\"PART_B create b.txt\",\"dependsOn\":[\"a\"]}]}\n```"' ;;
    esac
    ;;
  *PART_A*) echo a > a.txt; out "$report" ;;
  *PART_B*) test -f a.txt && echo b > b.txt; out "$report" ;;
esac
"#;

#[test]
fn a_task_with_depends_on_is_not_started_until_its_dependency_is_done() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let create = |title: &str, depends_on: serde_json::Value| {
        daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": title,
                "goal": "Make a trivial change",
                "verify": ["test -f CHANGED_MARKER.txt"],
                "dependsOn": depends_on,
                "start": false,
            }),
        )
    };
    let first = create("First", serde_json::json!([]));
    let first_id = first["id"].as_str().unwrap().to_string();
    let second = create("Second", serde_json::json!([first_id]));
    let second_id = second["id"].as_str().unwrap().to_string();
    assert_eq!(second["dependsOn"], serde_json::json!([first_id]));

    // Created with a dependency, the second task starts on its own, but its
    // loop must not run an attempt while the first is not done.
    std::thread::sleep(Duration::from_millis(1500));
    let waiting = daemon.request("task.get", serde_json::json!({"id": second_id}));
    assert_eq!(waiting["status"], "queued", "task JSON: {waiting}");
    assert!(waiting["attempts"].as_array().unwrap().is_empty());

    daemon.request("task.start", serde_json::json!({"id": first_id}));
    let first = settle(&daemon, &first_id);
    assert_eq!(first["status"], "done", "task JSON: {first}");
    let second = settle(&daemon, &second_id);
    assert_eq!(second["status"], "done", "task JSON: {second}");
    assert!(
        second["attempts"][0]["startedAt"].as_i64() >= first["attempts"][0]["endedAt"].as_i64(),
        "second started before first ended: {second}"
    );
    let listed = daemon.request("task.list", serde_json::json!({"repo": first["repo"]}));
    assert!(listed
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["dependsOn"] == serde_json::json!([first_id])));

    let worktrees = [first["worktree"].clone(), second["worktree"].clone()];
    daemon.shutdown_and_wait();
    for wt in worktrees {
        let _ = std::fs::remove_dir_all(wt.as_str().unwrap());
    }
}

#[test]
fn repo_audit_stores_the_parsed_report_and_returns_it() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let reply = "Audit done.\n\n```sushi-audit\n{\"summary\":\"Decent, slow tests.\",\"items\":[{\"area\":\"2. Build and test commands\",\"grade\":\"weak\",\"evidence\":[\"README.md:1\",\"unmeasured: no test command documented\"],\"recommendation\":\"Document one test command in README.md.\",\"effort\":\"small\"}],\"topFixes\":[\"Document one test command in README.md.\"]}\n```";
    let script = fake_audit_harness(scripts_dir.path(), reply);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();

    let started = daemon.request("repo.audit", serde_json::json!({"repo": repo_path}));
    let id = started["id"].as_str().unwrap().to_string();
    assert_eq!(started["status"], "running");
    // The planner's route by default.
    assert_eq!(started["routeId"], "claude-opus");

    let audit = poll_audit(&daemon, &id);
    assert_eq!(audit["status"], "done", "{audit}");
    assert!(audit.get("error").is_none(), "{audit}");
    assert_eq!(audit["costUsd"], 0.42);
    let report = &audit["report"];
    assert_eq!(report["summary"], "Decent, slow tests.");
    assert_eq!(report["items"][0]["area"], "2. Build and test commands");
    assert_eq!(report["items"][0]["grade"], "weak");
    assert_eq!(report["items"][0]["effort"], "small");
    assert_eq!(report["items"][0]["evidence"][0], "README.md:1");
    assert_eq!(
        report["topFixes"][0],
        "Document one test command in README.md."
    );

    let dir = daemon.data_dir().join("audits").join(&id);
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(&stored, report);
    let brief = std::fs::read_to_string(dir.join("brief.md")).unwrap();
    assert!(brief.contains("```sushi-audit"));
    assert!(std::fs::read_to_string(dir.join("events.jsonl"))
        .unwrap()
        .contains("sess-audit"));
    // The harness got that same brief, and the read-only review flags.
    assert_eq!(
        std::fs::read_to_string(scripts_dir.path().join("brief.md")).unwrap(),
        brief
    );
    let argv = std::fs::read_to_string(scripts_dir.path().join("argv.txt")).unwrap();
    assert!(argv.contains("--permission-mode\nplan\n"), "{argv}");
    assert!(argv.contains("--tools\nRead,Grep,Glob\n"), "{argv}");
    assert!(!argv.contains("acceptEdits"), "{argv}");

    let list = daemon.request("repo.audit.list", serde_json::json!({"repo": repo_path}));
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], id.as_str());
    assert_eq!(list[0]["report"], *report);

    daemon.shutdown_and_wait();
}

#[test]
fn repo_audit_without_a_parsable_report_stores_an_error_and_no_report() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_audit_harness(scripts_dir.path(), "The repository looks fine to me.");
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();

    let started = daemon.request(
        "repo.audit",
        serde_json::json!({"repo": repo.path().to_str().unwrap()}),
    );
    let id = started["id"].as_str().unwrap().to_string();
    let audit = poll_audit(&daemon, &id);
    assert_eq!(audit["status"], "failed", "{audit}");
    assert!(
        audit["error"]
            .as_str()
            .unwrap()
            .contains("no sushi-audit report"),
        "{audit}"
    );
    assert!(audit["report"].is_null(), "{audit}");
    assert_eq!(audit["costUsd"], 0.42);
    assert!(!daemon
        .data_dir()
        .join("audits")
        .join(&id)
        .join("report.json")
        .exists());
    daemon.shutdown_and_wait();
}

#[test]
fn repo_audit_refuses_a_directory_that_is_not_a_git_repository() {
    let daemon = Daemon::spawn(&[]);
    let plain = tempfile::tempdir().unwrap();
    let result = raw_request_with_params(
        &daemon.socket,
        "repo.audit",
        serde_json::json!({"repo": plain.path().to_str().unwrap()}),
        Some(&daemon.token),
    );
    assert_eq!(
        result["error"]["message"], "repo is not a git repository",
        "{result}"
    );
    let audits = daemon.data_dir().join("audits");
    let created = std::fs::read_dir(&audits).map(|d| d.count()).unwrap_or(0);
    assert_eq!(created, 0, "no audit may be recorded");
    daemon.shutdown_and_wait();
}

#[test]
fn a_plan_with_two_dependent_subtasks_lands_both_on_the_parent_in_order() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", GRAPH_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build both parts",
            "start": true,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();

    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    assert!(
        implement_attempts(&parent).is_empty(),
        "a parent implements nothing itself: {parent}"
    );
    let tasks = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    let children: Vec<&serde_json::Value> = tasks
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["parent"] == parent_id.as_str())
        .collect();
    assert_eq!(children.len(), 2, "tasks: {tasks}");
    let a = children.iter().find(|t| t["title"] == "Part A").unwrap();
    let b = children.iter().find(|t| t["title"] == "Part B").unwrap();
    assert_eq!(a["status"], "done", "task JSON: {a}");
    assert_eq!(b["status"], "done", "task JSON: {b}");
    assert_eq!(b["dependsOn"], serde_json::json!([a["id"]]));
    assert!(
        b["attempts"].as_array().unwrap().last().unwrap()["startedAt"].as_i64()
            >= a["attempts"].as_array().unwrap().last().unwrap()["endedAt"].as_i64(),
        "B started before A landed: {b}"
    );

    // Both commits are on the parent's branch, B's on top of A's.
    let branch = parent["branch"].as_str().unwrap();
    let subjects = git_out(repo.path(), &["log", "--format=%s", "-3", branch]);
    assert_eq!(subjects, "Part B\nPart A\ninit", "log of {branch}");
    let parent_wt = PathBuf::from(parent["worktree"].as_str().unwrap());
    assert!(parent_wt.join("a.txt").is_file() && parent_wt.join("b.txt").is_file());
    assert!(b["decisions"].to_string().contains("started from"), "{b}");
    let total = a["costUsd"].as_f64().unwrap() + b["costUsd"].as_f64().unwrap();
    assert!(
        parent["costUsd"].as_f64().unwrap() >= total - 1e-9,
        "parent cost includes its children: {parent}"
    );

    daemon.shutdown_and_wait();
    for t in [&parent, *a, *b] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn a_split_that_cannot_create_every_part_creates_none_and_runs_as_one_task() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", GRAPH_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    // Part B's worktree path is already taken, so creating Part B fails
    // after Part A was created.
    let basename = repo.path().file_name().unwrap().to_str().unwrap();
    let parent_dir = repo.path().parent().unwrap();
    let blocked = parent_dir.join(format!("{basename}-task-part-b"));
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::write(blocked.join("occupied.txt"), "x\n").unwrap();
    let part_a_wt = parent_dir.join(format!("{basename}-task-part-a"));

    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "build both parts",
            "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "stopped", "task JSON: {parent}");
    assert!(
        parent["decisions"]
            .to_string()
            .contains("could not create the subtasks"),
        "task JSON: {parent}"
    );
    let tasks = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    let tasks = tasks.as_array().unwrap();
    assert_eq!(tasks.len(), 1, "only the parent is left: {tasks:?}");
    assert!(tasks[0]["parent"].is_null());
    let branches = git_out(repo.path(), &["branch", "--list", "task/part-*"]);
    assert!(branches.is_empty(), "left-over branches: {branches}");
    assert!(!part_a_wt.exists(), "Part A's worktree was left behind");

    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(&blocked);
    let _ = std::fs::remove_dir_all(parent["worktree"].as_str().unwrap());
}

/// Two parts of a hand-built graph. Each writes its own file, or with
/// `SHARED` both write the same one; a retry told about a conflict keeps
/// both lines. Every run notes how many runs were live when it started.
const PARTS_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
mkdir -p "$PARTS_LOG/live"
touch "$PARTS_LOG/live/$$"
ls "$PARTS_LOG/live" | wc -l | tr -d ' ' >> "$PARTS_LOG/counts"
case "$input" in
  *"These files conflict"*) printf 'one\ntwo\n' > shared.txt ;;
  *PART_ONE*) sleep 1; if [ -n "$SHARED" ]; then echo one > shared.txt; else echo one > one.txt; fi ;;
  *PART_TWO*) sleep 1; if [ -n "$SHARED" ]; then echo two > shared.txt; else echo two > two.txt; fi ;;
esac
rm -f "$PARTS_LOG/live/$$"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

/// A parent created by hand and its two children; returns the ids.
fn hand_built_graph(
    daemon: &Daemon,
    repo: &Path,
    parent_verify: &str,
    child_verify: [&str; 2],
) -> (String, [String; 2]) {
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "title": "Parent",
            "goal": "Both parts",
            "verify": [parent_verify],
            // Left unstarted until its children are attached, or it would
            // implement the whole request itself.
            "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let child = |title: &str, goal: &str, verify: &str| {
        let t = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.to_str().unwrap(),
                "title": title,
                "goal": goal,
                "verify": [verify],
                "parent": parent_id,
            }),
        );
        assert_eq!(t["parent"], parent_id.as_str());
        assert_eq!(
            t["baseRef"], parent["branch"],
            "a child branches from its parent"
        );
        t["id"].as_str().unwrap().to_string()
    };
    let one = child("One", "PART_ONE", child_verify[0]);
    let two = child("Two", "PART_TWO", child_verify[1]);
    (parent_id, [one, two])
}

#[test]
fn two_independent_children_run_concurrently_and_both_land() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", PARTS_SCRIPT);
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
    ]);
    review_off(&daemon);
    let repo = init_git_repo();
    let (parent_id, [one, two]) = hand_built_graph(
        &daemon,
        repo.path(),
        "test -f one.txt && test -f two.txt",
        ["test -f one.txt", "test -f two.txt"],
    );

    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    let children: Vec<serde_json::Value> = [&one, &two]
        .iter()
        .map(|id| daemon.request("task.get", serde_json::json!({"id": id})))
        .collect();
    for t in &children {
        assert_eq!(t["status"], "done", "task JSON: {t}");
        assert!(
            t["decisions"].to_string().contains("Land: landed on"),
            "{t}"
        );
    }
    let counts = std::fs::read_to_string(log.path().join("counts")).unwrap();
    assert!(
        counts.lines().any(|c| c == "2"),
        "the two children never ran at the same time: {counts}"
    );
    let branch = parent["branch"].as_str().unwrap();
    let subjects = git_out(repo.path(), &["log", "--format=%s", branch]);
    assert!(
        subjects.contains("One") && subjects.contains("Two"),
        "{subjects}"
    );

    daemon.shutdown_and_wait();
    for t in children.iter().chain([&parent]) {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn a_child_whose_rebase_conflicts_is_retried_by_the_agent_not_dropped() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", PARTS_SCRIPT);
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
        ("SHARED", "1"),
    ]);
    review_off(&daemon);
    let repo = init_git_repo();
    let clean = "! grep -q '<<<<' shared.txt";
    let (parent_id, ids) = hand_built_graph(
        &daemon,
        repo.path(),
        "grep -q one shared.txt && grep -q two shared.txt",
        [
            &format!("grep -q one shared.txt && {clean}"),
            &format!("grep -q two shared.txt && {clean}"),
        ],
    );

    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "task JSON: {parent}");
    let children: Vec<serde_json::Value> = ids
        .iter()
        .map(|id| daemon.request("task.get", serde_json::json!({"id": id})))
        .collect();
    for t in &children {
        assert_eq!(t["status"], "done", "task JSON: {t}");
    }
    let retried: Vec<&serde_json::Value> = children
        .iter()
        .filter(|t| implement_attempts(t).len() >= 2)
        .collect();
    assert_eq!(
        retried.len(),
        1,
        "exactly the second to land retries: {children:?}"
    );
    let first = &implement_attempts(retried[0])[0];
    assert_eq!(first["status"], "failed", "{first}");
    assert!(
        first["failure"]["detail"]
            .as_str()
            .unwrap()
            .contains("These files conflict: shared.txt"),
        "{first}"
    );
    let parent_wt = PathBuf::from(parent["worktree"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(parent_wt.join("shared.txt")).unwrap(),
        "one\ntwo\n"
    );

    daemon.shutdown_and_wait();
    for t in children.iter().chain([&parent]) {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn task_create_rejects_a_dependency_cycle_and_an_unknown_id_and_creates_nothing() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();
    let create = |extra: serde_json::Value| {
        let mut params = serde_json::json!({
            "repo": repo_path,
            "title": "T",
            "goal": "G",
            "start": false,
        });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        raw_request_with_params(&daemon.socket, "task.create", params, Some(&daemon.token))
    };
    let parent = create(serde_json::json!({}))["result"].clone();
    let parent_id = parent["id"].as_str().unwrap().to_string();
    // Waits for the parent, which in turn would wait for its new child.
    let waiter = create(serde_json::json!({"dependsOn": [parent_id]}))["result"].clone();
    let waiter_id = waiter["id"].as_str().unwrap().to_string();
    let worktrees_before = git_out(repo.path(), &["worktree", "list"]);

    let rejected = [
        (
            serde_json::json!({"parent": parent_id, "dependsOn": [waiter_id]}),
            "cycle",
        ),
        (
            serde_json::json!({"parent": parent_id, "dependsOn": [parent_id]}),
            "cycle",
        ),
        (
            serde_json::json!({"dependsOn": [uuid_like()]}),
            "unknown dependency",
        ),
        (
            serde_json::json!({"dependsOn": ["nope"]}),
            "unknown dependency",
        ),
        (serde_json::json!({"parent": uuid_like()}), "unknown parent"),
    ];
    for (extra, why) in rejected {
        let v = create(extra.clone());
        let message = v["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains(why), "{extra}: {v}");
    }
    let listed = daemon.request("task.list", serde_json::json!({"repo": parent["repo"]}));
    assert_eq!(listed.as_array().unwrap().len(), 2, "{listed}");
    assert_eq!(
        git_out(repo.path(), &["worktree", "list"]),
        worktrees_before
    );

    daemon.shutdown_and_wait();
    for t in [&parent, &waiter] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

fn uuid_like() -> String {
    "4f1c7a52-2d3b-4c9e-9a1e-0b7f6d5e4c3a".to_string()
}

#[test]
fn a_stopped_dependency_asks_its_dependent_and_dropping_it_lets_the_dependent_run() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ninput=\"$(cat)\"\ncase \"$input\" in *SLOW*) sleep 30 ;; esac\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    let repo = init_git_repo();
    let slow = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "SLOW dependency",
            "goal": "Takes a while",
            "verify": ["true"],
            "start": true,
        }),
    );
    let slow_id = slow["id"].as_str().unwrap().to_string();
    let dependent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Dependent",
            "goal": "Runs after",
            "verify": ["true"],
            "dependsOn": [slow_id],
        }),
    );
    let dependent_id = dependent["id"].as_str().unwrap().to_string();
    poll_until(&daemon, &slow_id, Duration::from_secs(10), |s| {
        s == "running"
    });
    daemon.request("task.stop", serde_json::json!({"id": slow_id}));

    let asked = poll_until(&daemon, &dependent_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    let question = asked["question"]["text"].as_str().unwrap();
    assert!(
        question.contains("\"SLOW dependency\" (stopped)"),
        "{asked}"
    );
    assert_eq!(
        asked["question"]["options"],
        serde_json::json!(["retry the dependency", "drop the dependency", "stop"])
    );
    daemon.request(
        "task.answer",
        serde_json::json!({"id": dependent_id, "answer": "drop the dependency"}),
    );
    let done = settle(&daemon, &dependent_id);
    assert_eq!(done["status"], "done", "task JSON: {done}");
    assert!(done.get("dependsOn").is_none(), "{done}");

    daemon.shutdown_and_wait();
    for t in [&slow, &dependent] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn stopping_a_parent_stops_its_children_and_running_it_again_restarts_them() {
    let scripts_dir = tempfile::tempdir().unwrap();
    // The first run hangs until it is stopped; every later run finishes.
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        "#!/bin/sh\ncat > /dev/null\nif [ ! -f \"$PARTS_LOG/slept\" ]; then touch \"$PARTS_LOG/slept\"; sleep 30; fi\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n",
    );
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
    ]);
    review_off(&daemon);
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Parent",
            "goal": "One part",
            "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let child = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Only part",
            "goal": "Make a change",
            "verify": ["test -f CHANGED_MARKER.txt"],
            "parent": parent_id,
        }),
    );
    let child_id = child["id"].as_str().unwrap().to_string();
    let start = Instant::now();
    while !log.path().join("slept").exists() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the child never ran"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let stopped = daemon.request("task.stop", serde_json::json!({"id": parent_id}));
    assert_eq!(
        stopped["status"], "stopped",
        "an idle parent stops at once: {stopped}"
    );
    let child_stopped = settle(&daemon, &child_id);
    assert_eq!(child_stopped["status"], "stopped", "{child_stopped}");
    // A stop is not a dependency failure: nobody is asked anything.
    let parent_now = daemon.request("task.get", serde_json::json!({"id": parent_id}));
    assert_eq!(parent_now["status"], "stopped", "{parent_now}");

    daemon.request("task.start", serde_json::json!({"id": parent_id}));
    let parent_done = poll_until(&daemon, &parent_id, Duration::from_secs(30), |s| {
        matches!(s, "done" | "failed" | "waiting")
    });
    assert_eq!(parent_done["status"], "done", "{parent_done}");
    let child_done = daemon.request("task.get", serde_json::json!({"id": child_id}));
    assert_eq!(child_done["status"], "done", "{child_done}");

    daemon.shutdown_and_wait();
    for t in [&parent_done, &child_done] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

/// The first run hangs until it is stopped; every later run finishes.
const SLEEP_ONCE_SCRIPT: &str = "#!/bin/sh\ncat > /dev/null\nif [ ! -f \"$PARTS_LOG/slept\" ]; then touch \"$PARTS_LOG/slept\"; sleep 30; fi\necho changed > CHANGED_MARKER.txt\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\necho '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n";

fn sleep_once_daemon() -> (Daemon, tempfile::TempDir, tempfile::TempDir) {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", SLEEP_ONCE_SCRIPT);
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
    ]);
    review_off(&daemon);
    (daemon, scripts_dir, log)
}

/// Creates a task that hangs on its first run, waits until it does, stops it.
fn create_and_stop_first_run(
    daemon: &Daemon,
    repo: &Path,
    log: &Path,
    extra: serde_json::Value,
) -> serde_json::Value {
    let mut params = serde_json::json!({
        "repo": repo.to_str().unwrap(),
        "title": "Hangs once",
        "goal": "Takes a while",
        "verify": ["true"],
        "start": true,
    });
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let task = daemon.request("task.create", params);
    let start = Instant::now();
    while !log.join("slept").exists() {
        assert!(start.elapsed() < Duration::from_secs(10), "never ran");
        std::thread::sleep(Duration::from_millis(50));
    }
    let id = task["id"].as_str().unwrap();
    daemon.request("task.stop", serde_json::json!({"id": id}));
    poll_until(daemon, id, Duration::from_secs(10), |s| s == "stopped");
    task
}

#[test]
fn a_parent_waiting_for_a_stopped_dependency_starts_no_child() {
    let (daemon, _scripts, log) = sleep_once_daemon();
    let repo = init_git_repo();
    let dep = create_and_stop_first_run(&daemon, repo.path(), log.path(), serde_json::json!({}));
    let dep_id = dep["id"].as_str().unwrap().to_string();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Parent",
            "goal": "Both parts",
            "verify": ["true"],
            "dependsOn": [dep_id],
            "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let child = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Only part",
            "goal": "Make a change",
            "verify": ["true"],
            "parent": parent_id,
        }),
    );
    let child_id = child["id"].as_str().unwrap().to_string();

    let asked = poll_until(&daemon, &parent_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert!(
        asked["question"]["text"]
            .as_str()
            .unwrap()
            .contains("(stopped)"),
        "{asked}"
    );
    std::thread::sleep(Duration::from_millis(1500));
    let child_now = daemon.request("task.get", serde_json::json!({"id": child_id}));
    assert_eq!(child_now["status"], "queued", "{child_now}");
    assert_eq!(child_now["attempts"], serde_json::json!([]), "{child_now}");
    let parent_now = daemon.request("task.get", serde_json::json!({"id": parent_id}));
    assert_eq!(
        parent_now["attempts"],
        serde_json::json!([]),
        "{parent_now}"
    );

    daemon.shutdown_and_wait();
    for t in [&dep, &parent, &child] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn a_task_queued_for_a_slot_cannot_become_a_parent_and_implements_alone() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", SLEEP_ONCE_SCRIPT);
    let log = tempfile::tempdir().unwrap();
    let fake_bins = [
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("PARTS_LOG", log.path().to_str().unwrap()),
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
    let repo_path = repo.path().to_str().unwrap();

    // The blocker holds the only slot; the parent queues behind it.
    let blocker = call(
        "task.create",
        serde_json::json!({"repo": repo_path, "title": "Blocker", "goal": "g",
        "verify": ["true"], "start": true}),
    );
    let start = Instant::now();
    while !log.path().join("slept").exists() {
        assert!(start.elapsed() < Duration::from_secs(10), "never ran");
        std::thread::sleep(Duration::from_millis(50));
    }
    let parent = call(
        "task.create",
        serde_json::json!({"repo": repo_path, "title": "Parent", "goal": "g",
        "verify": ["true"], "start": true}),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let rejected = raw_request_with_params(
        &socket,
        "task.create",
        serde_json::json!({"repo": repo_path, "title": "Late child", "goal": "g",
        "parent": parent_id}),
        Some(&token),
    );
    let message = rejected["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("already running"), "{rejected}");
    let listed = call("task.list", serde_json::json!({"repo": parent["repo"]}));
    assert_eq!(listed.as_array().unwrap().len(), 2, "{listed}");

    // Freed slot: the parent is an ordinary task and implements itself.
    call(
        "task.stop",
        serde_json::json!({"id": blocker["id"].as_str().unwrap()}),
    );
    let start = Instant::now();
    let done = loop {
        let t = call("task.get", serde_json::json!({"id": parent_id}));
        if t["status"] == "done" {
            break t;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "{t}");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");

    let _ = call("shutdown", serde_json::json!({}));
    let _ = wait_for_exit(child, Duration::from_secs(5));
    for t in [&blocker, &parent] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn retrying_a_stopped_dependency_finishes_the_dependency_and_its_dependent() {
    let (daemon, _scripts, log) = sleep_once_daemon();
    let repo = init_git_repo();
    let dep = create_and_stop_first_run(&daemon, repo.path(), log.path(), serde_json::json!({}));
    let dep_id = dep["id"].as_str().unwrap().to_string();
    let dependent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Dependent",
            "goal": "Runs after",
            "verify": ["true"],
            "dependsOn": [dep_id],
        }),
    );
    let dependent_id = dependent["id"].as_str().unwrap().to_string();
    poll_until(&daemon, &dependent_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    daemon.request(
        "task.answer",
        serde_json::json!({"id": dependent_id, "answer": "retry the dependency"}),
    );
    let dep_done = poll_until(&daemon, &dep_id, Duration::from_secs(20), |s| {
        matches!(s, "done" | "failed" | "waiting")
    });
    assert_eq!(dep_done["status"], "done", "{dep_done}");
    let dependent_done = poll_until(&daemon, &dependent_id, Duration::from_secs(20), |s| {
        matches!(s, "done" | "failed")
    });
    assert_eq!(dependent_done["status"], "done", "{dependent_done}");
    assert!(dependent_done["question"].is_null(), "{dependent_done}");

    daemon.shutdown_and_wait();
    for t in [&dep, &dependent] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn dropping_the_only_subtask_stops_the_parent_instead_of_implementing_it() {
    let (daemon, _scripts, log) = sleep_once_daemon();
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Parent",
            "goal": "One part",
            "verify": ["true"],
            "start": false,
        }),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let child = create_and_stop_first_run(
        &daemon,
        repo.path(),
        log.path(),
        serde_json::json!({"title": "Only part", "parent": parent_id}),
    );
    let asked = poll_until(&daemon, &parent_id, Duration::from_secs(15), |s| {
        s == "waiting"
    });
    assert!(asked["question"]["text"].is_string(), "{asked}");
    daemon.request(
        "task.answer",
        serde_json::json!({"id": parent_id, "answer": "drop the dependency"}),
    );
    std::thread::sleep(Duration::from_millis(1500));
    let after = daemon.request("task.get", serde_json::json!({"id": parent_id}));
    assert_eq!(after["status"], "stopped", "{after}");
    assert_eq!(after["attempts"], serde_json::json!([]), "{after}");
    assert!(
        after["decisions"].to_string().contains("no subtasks"),
        "{after}"
    );

    daemon.shutdown_and_wait();
    for t in [&parent, &child] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}
