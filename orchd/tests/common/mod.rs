//! Shared helpers for the black-box integration tests: spawning and
//! guarding `orchd serve` daemons, fake harness scripts, and request/poll
//! helpers. Each test file compiles this as its own module, so any one
//! file uses only a subset.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub struct Daemon {
    pub child: OrchdChild,
    pub socket: PathBuf,
    pub data_dir: tempfile::TempDir,
    pub token: String,
}

/// Reads `<data>/control.token` (spec item A), trimmed -- the same file the
/// real Electron client reads before it can call anything but `ping`.
pub fn read_control_token(data_dir: &Path) -> String {
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

/// An `orchd serve` child that dies with its owner. Every daemon a test
/// spawns -- through `Daemon` or `spawn_orchd_raw` -- is held in one, so a
/// test that panics, or simply forgets to shut its daemon down, cannot leave
/// an orphan behind (they used to pile up across runs, each with a temp
/// `--data` dir). Dropping reaps the process and asserts it is really gone.
pub struct OrchdChild(Child);

impl std::ops::Deref for OrchdChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for OrchdChild {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for OrchdChild {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
        if !std::thread::panicking() {
            assert!(
                matches!(self.0.try_wait(), Ok(Some(_))),
                "orchd serve (pid {}) survived its guard",
                self.0.id()
            );
        }
    }
}

impl Daemon {
    pub fn spawn(extra_env: &[(&str, &str)]) -> Daemon {
        let data_dir = tempfile::tempdir().unwrap();
        let socket = data_dir.path().join("orchd.sock");
        let child = spawn_orchd_raw(data_dir.path(), &socket, extra_env);
        wait_for_socket(&socket);
        let token = read_control_token(data_dir.path());
        // The brief check is one more harness run: off, so tests that count
        // runs, cost or argv see only the runs they set up. Its own tests
        // turn it back on.
        {
            let mut settings =
                request_on(&socket, "settings.get", serde_json::json!({}), Some(&token));
            settings["briefCheckRoute"] = serde_json::json!("");
            if native_sandbox_unavailable() {
                fit_sandbox(&mut settings);
            }
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

    pub fn data_dir(&self) -> &Path {
        self.data_dir.path()
    }

    pub fn request(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        request_on(&self.socket, method, params, Some(&self.token))
    }

    pub fn shutdown_and_wait(mut self) {
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

pub fn spawn_orchd_raw(data_dir: &Path, socket: &Path, extra_env: &[(&str, &str)]) -> OrchdChild {
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
    OrchdChild(cmd.spawn().expect("failed to spawn orchd"))
}

pub fn wait_for_socket(socket: &Path) {
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
pub fn wait_for_exit(mut child: OrchdChild, timeout: Duration) -> (ExitStatus, String) {
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

pub fn request_on(
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
    run(&["config", "user.email", "orchd-test@example.com"]);
    run(&["config", "user.name", "orchd test"]);
    std::fs::write(dir.path().join("README.md"), "hello\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-q", "-m", "init"]);
    dir
}

pub fn raw_request_with_params(
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

/// `kill(pid, 0)`: true iff a process with this pid exists and is
/// signalable by us -- the standard liveness probe, same one
/// `main.rs::acquire_singleton` uses for the daemon's own pidfile.
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
/// the `--settings` file it was given, extracts the hook command orchd
/// wired up, and actually runs `orchd hook stop` against the live daemon
/// with a synthetic payload on stdin -- the same way the real Claude CLI
/// would, just without a real model in the loop.
pub const FAKE_CLAUDE_HOOK_SCRIPT: &str = r#"#!/usr/bin/env node
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
