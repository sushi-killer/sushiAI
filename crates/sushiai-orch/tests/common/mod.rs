//! Shared helpers for the black-box integration tests: spawning and
//! guarding real `sushiai daemon` processes with the orchestrator module enabled, fake
//! harness scripts, and request/poll helpers. Each test file compiles this as its own module, so any one
//! file uses only a subset.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub struct Daemon {
    pub child: DaemonChild,
    pub socket: PathBuf,
    /// The sushiAI home the daemon runs in; the orchestrator lives in `<home>/orchestrator`.
    pub home: tempfile::TempDir,
    data: PathBuf,
}

/// A `sushiai daemon` child that dies with its owner. Every daemon a test spawns -- through
/// `Daemon` or `spawn_daemon` -- is held in one, so a test that panics, or simply forgets to
/// shut its daemon down, cannot leave an orphan behind. Dropping reaps the process.
pub struct DaemonChild(Child);

impl std::ops::Deref for DaemonChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for DaemonChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for DaemonChild {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// The `sushiai` binary the daemon runs as: `SUSHIAI_BIN`, else the workspace's own build,
/// brought up to date once per test process.
pub fn sushiai_bin() -> &'static Path {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(bin) = std::env::var_os("SUSHIAI_BIN") {
            return PathBuf::from(bin);
        }
        let exe = std::env::current_exe().expect("test executable");
        let profile_dir = exe
            .parent()
            .and_then(Path::parent)
            .expect("target profile dir")
            .to_path_buf();
        let mut build = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        build
            .args(["build", "--quiet", "-p", "sushiai"])
            .current_dir(env!("CARGO_MANIFEST_DIR"));
        if profile_dir.file_name().is_some_and(|n| n == "release") {
            build.arg("--release");
        }
        assert!(
            build.status().is_ok_and(|s| s.success()),
            "cargo build -p sushiai failed"
        );
        profile_dir.join("sushiai")
    })
}

/// The daemon socket of a home.
pub fn socket_of(home: &Path) -> PathBuf {
    home.join("daemon.sock")
}

/// Starts `sushiai daemon` in `home` with the orchestrator module enabled, like the desktop
/// does. Its output goes to `<home>/daemon-test.log`.
pub fn spawn_daemon(home: &Path, extra_env: &[(&str, &str)]) -> DaemonChild {
    let user = home.join("user");
    std::fs::create_dir_all(user.join(".codex")).unwrap();
    std::fs::write(
        user.join(".gitconfig"),
        "[user]\n\tname = orch test\n\temail = orch-test@example.invalid\n",
    )
    .unwrap();
    std::fs::create_dir_all(home.join("modules")).unwrap();
    std::fs::write(home.join("modules/orch.enabled"), b"").unwrap();
    let log = std::fs::File::options()
        .create(true)
        .append(true)
        .open(home.join(DAEMON_LOG))
        .unwrap();
    let mut cmd = Command::new(sushiai_bin());
    cmd.arg("daemon")
        .env("SUSHIAI_HOME", home)
        .env("HOME", &user)
        .env("CODEX_HOME", user.join(".codex"))
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    DaemonChild(cmd.spawn().expect("failed to spawn sushiai daemon"))
}

impl Daemon {
    pub fn spawn(extra_env: &[(&str, &str)]) -> Daemon {
        let home = tempfile::tempdir().unwrap();
        let socket = socket_of(home.path());
        let child = spawn_daemon(home.path(), extra_env);
        wait_for_socket(&socket);
        // The brief check is one more harness run: off, so tests that count
        // runs, cost or argv see only the runs they set up. Its own tests
        // turn it back on.
        {
            let mut settings = request_on(&socket, "settings.get", serde_json::json!({}));
            settings["briefCheckRoute"] = serde_json::json!("");
            // The answer policy answers routine questions itself; the other
            // tests wait for them. Its own tests turn it back on.
            settings["answerPolicy"] = serde_json::json!(false);
            if native_sandbox_unavailable() {
                fit_sandbox(&mut settings);
            }
            request_on(
                &socket,
                "settings.set",
                serde_json::json!({"settings": settings}),
            );
        }
        let data = home.path().join("orchestrator");
        Daemon {
            child,
            socket,
            home,
            data,
        }
    }

    /// The orchestrator's own folder.
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    pub fn request(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        request_on(&self.socket, method, params)
    }

    /// The `error` text of a request that is expected to fail.
    pub fn request_error(&self, method: &str, params: serde_json::Value) -> String {
        let v = raw_request_with_params(&self.socket, method, params);
        v["error"].to_string()
    }

    /// Stops the daemon and waits for it to exit, then starts a new one in the same home.
    pub fn restart(&mut self, extra_env: &[(&str, &str)]) {
        stop_daemon(&self.socket);
        let begin = Instant::now();
        while !matches!(self.child.try_wait(), Ok(Some(_))) {
            assert!(begin.elapsed() < Duration::from_secs(15), "no exit");
            std::thread::sleep(Duration::from_millis(50));
        }
        self.child = spawn_daemon(self.home.path(), extra_env);
        wait_for_socket(&self.socket);
    }

    pub fn shutdown_and_wait(mut self) {
        // Drop still runs afterwards and finds the child already reaped.
        stop_daemon(&self.socket);
        let start = Instant::now();
        loop {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            if start.elapsed() > Duration::from_secs(15) {
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
/// which is exactly where this suite runs when the orchestrator's own Native-sandboxed
/// verify runs `cargo test`: every daemon here would then fail each
/// verify command it wraps in `sandbox-exec`, so no task could ever pass.
pub fn native_sandbox_unavailable() -> bool {
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
pub fn fit_sandbox(settings: &mut serde_json::Value) {
    if native_sandbox_unavailable() {
        settings["sandbox"] = serde_json::json!("host");
    }
}

/// The file a daemon's output goes to, inside its home, so a request that gets no answer can
/// say what the daemon printed.
const DAEMON_LOG: &str = "daemon-test.log";

/// Why a request got no answer: the tail of the daemon's log, when there is one.
fn no_answer(socket: &Path, method: &str) -> String {
    let log = std::fs::read_to_string(socket.with_file_name(DAEMON_LOG)).unwrap_or_default();
    let tail: Vec<&str> = log.lines().rev().take(40).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    format!(
        "the daemon closed the connection without answering {method}; log tail:\n{}",
        tail.join("\n")
    )
}

pub fn wait_for_socket(socket: &Path) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        if socket.exists() && UnixStream::connect(socket).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the daemon did not open its socket in time");
}

/// Wait for a raw (non-`Daemon`) child to exit, returning its status and
/// everything it wrote to stderr. Used for the singleton/bind-failure tests
/// where the process under test is expected to exit quickly on its own.
pub fn wait_for_exit(mut child: DaemonChild, timeout: Duration) -> (ExitStatus, String) {
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
            panic!("the daemon did not exit within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// One JSON frame: a 4-byte big-endian length (kind byte included), the kind `J`, the text.
fn frame(text: &str) -> Vec<u8> {
    let mut out = ((text.len() + 1) as u32).to_be_bytes().to_vec();
    out.push(b'J');
    out.extend_from_slice(text.as_bytes());
    out
}

/// Opens a connection, says `hello`, sends `orch.<method>` and returns the response envelope
/// (`result` or `error`). `None` when the daemon closed the connection first.
fn exchange(socket: &Path, method: &str, params: serde_json::Value) -> Option<serde_json::Value> {
    let mut stream = UnixStream::connect(socket).expect("connect to the daemon socket");
    let hello = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "hello",
        "params": {"protocol": 1, "client": "orch-test"}});
    let call = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": format!("orch.{method}"),
        "params": params});
    stream.write_all(&frame(&hello.to_string())).unwrap();
    stream.write_all(&frame(&call.to_string())).unwrap();
    stream.flush().unwrap();
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        while pending.len() >= 4 {
            let len = u32::from_be_bytes(pending[..4].try_into().unwrap()) as usize;
            if pending.len() < 4 + len {
                break;
            }
            let body: Vec<u8> = pending.drain(..4 + len).skip(5).collect();
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
                if v["id"] == 2 {
                    return Some(v);
                }
            }
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(n) => pending.extend_from_slice(&buf[..n]),
        }
    }
}

/// Asks the daemon to stop. It may exit before it answers.
pub fn stop_daemon(socket: &Path) {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return;
    };
    let hello = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "hello",
        "params": {"protocol": 1, "client": "orch-test"}});
    let stop = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "daemon.shutdown",
        "params": {}});
    let _ = stream.write_all(&frame(&hello.to_string()));
    let _ = stream.write_all(&frame(&stop.to_string()));
    let _ = stream.flush();
    let mut sink = [0u8; 4096];
    while matches!(stream.read(&mut sink), Ok(n) if n > 0) {}
}

/// The `result` of `orch.<method>`; panics on an error answer.
pub fn request_on(socket: &Path, method: &str, params: serde_json::Value) -> serde_json::Value {
    let Some(v) = exchange(socket, method, params) else {
        panic!("{}", no_answer(socket, method));
    };
    if let Some(err) = v.get("error") {
        panic!("{method} failed: {err}");
    }
    v.get("result").cloned().unwrap_or(serde_json::Value::Null)
}

/// `git init` + one commit in a fresh temp dir, for `task.create` to branch
/// a worktree from.
pub fn init_git_repo() -> tempfile::TempDir {
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
    run(&["config", "user.email", "orch-test@example.invalid"]);
    run(&["config", "user.name", "orch test"]);
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init"]);
    dir
}

/// The whole response envelope of `orch.<method>`, error or not.
pub fn raw_request_with_params(
    socket: &Path,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    exchange(socket, method, params).unwrap_or_else(|| panic!("{}", no_answer(socket, method)))
}

/// `kill(pid, 0)`: true iff a process with this pid exists and is signalable by us.
pub fn is_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

pub fn fake_harness_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

pub fn poll_task_status(daemon: &Daemon, task_id: &str, timeout: Duration) -> serde_json::Value {
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

/// A fake Claude harness that exercises the *real* Stop hook path: it reads
/// the `--settings` file it was given, extracts the hook command the orchestrator
/// wired up, and actually runs `sushiai orch hook stop` against the live daemon
/// with a synthetic payload on stdin -- the same way the real Claude CLI
/// would, just without a real model in the loop.
pub const FAKE_CLAUDE_HOOK_SCRIPT: &str = r#"#!/usr/bin/env node
// First output before anything else runs: the stall clock must not depend on
// how long node takes to load modules on a busy machine.
console.log(JSON.stringify({ type: 'system', subtype: 'init', session_id: 'sess-fake' }));

const fs = require('fs');
const { execSync } = require('child_process');

const args = process.argv.slice(2);
const settingsPath = args[args.indexOf('--settings') + 1];
const settings = JSON.parse(fs.readFileSync(settingsPath, 'utf8'));
const hookCmd = settings.hooks.Stop[0].hooks[0].command;

try { fs.readFileSync(0); } catch (e) {}

fs.writeFileSync('CHANGED_MARKER.txt', 'changed\n');

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
pub const FAKE_PLANNER_SCRIPT: &str = r#"#!/bin/sh
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

pub fn poll_until(
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

/// First attempt leaves FIRST (verify wants SECOND) and a handoff; the
/// retry creates SECOND. Every invocation's argv goes to $ARGS_LOG.
pub const FAKE_RETRY_SCRIPT: &str = r#"#!/bin/sh
cat > /dev/null
printf '%s\n' "$*" >> "$ARGS_LOG"
if [ -f FIRST ]; then echo x > SECOND; else echo x > FIRST; fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
json='{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"handoff\":\"tried the quick fix\",\"decisions\":[],\"question\":\"\"}\n```"}'
printf '%s\n' "$json"
"#;

pub const FAKE_CLAUDE_PASS: &str = "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CLAUDE_ARGS\"\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n";

pub fn run_file(daemon: &Daemon, task_id: &str, name: &str) -> String {
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

pub fn git_out(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

// ---------------------------------------------------------------------------
// Task graph: dependsOn, parents, landing
// ---------------------------------------------------------------------------

pub fn settle(daemon: &Daemon, task_id: &str) -> serde_json::Value {
    poll_until(daemon, task_id, Duration::from_secs(30), |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    })
}

/// Points the cheap (`mechanical`) tier at a Claude route, so a conflict-only
/// attempt runs on the fake Claude harness the test set up.
pub fn cheap_route_on_claude(daemon: &Daemon) {
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["routes"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": "claude-cheap", "label": "Claude cheap", "harness": "claude", "model": "sonnet",
        }));
    settings["tiers"]["mechanical"] = serde_json::json!("claude-cheap");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
}

/// One fake playing every role of a task whose review always FAILs: the
/// implementer edits a file and reports, the reviewer (brief opens with
/// "## Review") fails it, and the orchestrator's accept judge (its brief
/// asks for `{"accept": true}`) records the call in $LOG_DIR/judge.called
/// and answers `{"accept": $ACCEPT_REPLY}`.
pub const FAKE_FAILING_REVIEW_SCRIPT: &str = r###"#!/bin/sh
input="$(cat)"
case "$input" in
  "## Review"*)
    printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"needs work\"]}\n```"}' ;;
  *'"accept": true'*)
    touch "$LOG_DIR/judge.called"
    printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.02,\"result\":\"{\\\"accept\\\": $ACCEPT_REPLY}\"}" ;;
  *)
    echo changed > CHANGED_MARKER.txt
    printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
    printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;
