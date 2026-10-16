//! Shared helpers for the end-to-end tests: real daemon and holder processes under a
//! temporary `SUSHIAI_HOME`.
#![allow(dead_code)]

pub mod rig;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sushiai_protocol::{encode, Decoder, Frame, Message, Request, RpcError};

pub const BIN: &str = env!("CARGO_BIN_EXE_sushiai");

/// The one wall-clock budget every positive wait in this suite shares: 180 s, or
/// `SUSHIAI_TEST_DEADLINE_SECS` when it is a positive whole number (unset, 0 or anything else
/// means 180 s; the same contract as `sushiai-orch/tests/common` and
/// `tests/helpers/deadline.cjs`). A short window belongs only to a check that something does
/// not happen.
pub fn deadline() -> Duration {
    static DEADLINE: OnceLock<Duration> = OnceLock::new();
    *DEADLINE.get_or_init(|| {
        let secs = std::env::var("SUSHIAI_TEST_DEADLINE_SECS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|secs| *secs > 0);
        Duration::from_secs(secs.unwrap_or(180))
    })
}

/// Polls `ok` until it is true; panics naming `what` after the shared deadline.
pub fn wait_until(what: &str, mut ok: impl FnMut() -> bool) {
    wait_value(what, || ok().then_some(()));
}

/// Polls `check` until it returns `Some`; panics naming `what` after the shared deadline.
pub fn wait_value<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + self::deadline();
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        sleep(Duration::from_millis(50));
    }
}

/// How long an absence check watches: long enough for the daemon to act on
/// something it should not do. A positive wait never uses it.
pub const ABSENCE_WINDOW: Duration = Duration::from_millis(1500);

/// An absence check: polls `still` for `window` and panics naming `what` as
/// soon as it stops holding. This, not a bare sleep, is how a test waits for
/// something not to happen.
pub fn holds_for(what: &str, window: Duration, mut still: impl FnMut() -> bool) {
    let end = Instant::now() + window;
    loop {
        assert!(still(), "{what} stopped holding");
        if Instant::now() >= end {
            return;
        }
        sleep(Duration::from_millis(50));
    }
}

pub fn processes() -> Vec<(u32, u32, String)> {
    sushiai_daemon::process::list_pids()
        .into_iter()
        .filter_map(|pid| {
            let info = sushiai_daemon::process::info(pid)?;
            Some((
                pid as u32,
                info.parent_pid as u32,
                sushiai_daemon::process::argv(pid)?.join(" "),
            ))
        })
        .collect()
}

pub fn alive(pid: u32) -> bool {
    sushiai_daemon::process::alive(pid as i32)
}

/// Close the sessions of this disposable home through their owning daemon.
pub fn shutdown_home(home: &Path) {
    if !home.join("daemon.lock").exists() {
        return;
    }
    let mut recovery = if UnixStream::connect(home.join("daemon.sock")).is_err() {
        Command::new(BIN)
            .arg("daemon")
            .env("SUSHIAI_HOME", home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    } else {
        None
    };
    // A recovery daemon that exits (a home it refuses, such as a newer state
    // schema) never opens the socket: stop waiting for it then.
    let deadline = Instant::now() + self::deadline();
    while UnixStream::connect(home.join("daemon.sock")).is_err() && Instant::now() < deadline {
        if let Some(child) = recovery.as_mut() {
            if !matches!(child.try_wait(), Ok(None)) {
                return;
            }
        }
        sleep(Duration::from_millis(25));
    }
    let _ = Command::new(BIN)
        .arg("shutdown")
        .env("SUSHIAI_HOME", home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Some(child) = recovery.as_mut() {
        let deadline = Instant::now() + self::deadline();
        while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
            sleep(Duration::from_millis(25));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

pub fn kill(pid: u32, signal: &str) {
    let _ = Command::new("kill")
        .args([signal, &pid.to_string()])
        .status();
}

/// The child of `parent` (a holder's shell).
pub fn child_of(parent: u32) -> u32 {
    let mut found = None;
    wait_until("the holder's child", || {
        found = processes()
            .into_iter()
            .find(|(_, ppid, _)| *ppid == parent)
            .map(|p| p.0);
        found.is_some()
    });
    found.expect("child")
}

/// A temp home. Drop closes its sessions through the daemon and reaps owned children.
pub struct Sandbox {
    pub dir: tempfile::TempDir,
    pub daemons: Vec<Child>,
    /// Extra variables for daemons started from here.
    pub env: Vec<(String, String)>,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let dir = tempfile::Builder::new()
            .prefix("sd")
            .tempdir_in("/tmp")
            .expect("tempdir");
        Sandbox {
            dir,
            daemons: Vec::new(),
            env: Vec::new(),
        }
    }

    pub fn home(&self) -> &Path {
        self.dir.path()
    }

    pub fn socket(&self) -> PathBuf {
        self.home().join("daemon.sock")
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(BIN);
        // An owner-set wrapper turns the compiler cache off: a test daemon must not start
        // or join the owner's sccache server (fixed port 4227).
        command
            .arg("daemon")
            .env("SUSHIAI_HOME", self.home())
            .env("RUSTC_WRAPPER", "");
        command.envs(self.env.iter().map(|(k, v)| (k, v)));
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    /// Starts a daemon and waits until it answers. Returns its pid.
    pub fn start_daemon(&mut self) -> u32 {
        let child = self.command().spawn().expect("spawn daemon");
        let pid = child.id();
        self.daemons.push(child);
        let socket = self.socket();
        let last = self.daemons.len() - 1;
        wait_until("daemon socket", || {
            let child = &mut self.daemons[last];
            assert!(
                child.try_wait().expect("try_wait").is_none(),
                "daemon exited early"
            );
            self.lock_pid() == Some(pid) && UnixStream::connect(&socket).is_ok()
        });
        pid
    }

    /// Runs a daemon that is expected to exit on its own; returns its status and stderr.
    pub fn daemon_that_exits(&mut self) -> (ExitStatus, String) {
        let mut command = self.command();
        command.stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn daemon");
        let deadline = Instant::now() + self::deadline();
        let status = loop {
            if let Some(status) = child.try_wait().expect("try_wait") {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("daemon did not exit");
            }
            sleep(Duration::from_millis(50));
        };
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        (status, stderr)
    }

    pub fn client(&self) -> Client {
        let mut client = Client::connect(&self.socket());
        let hello = client.call("hello", json!({"protocol": 1, "client": "test"}));
        assert_eq!(hello["protocol"], 1);
        client
    }

    pub fn create(&self, client: &mut Client, shell_command: &str) -> String {
        let created = client.call(
            "session.create",
            json!({"cmd": ["/bin/sh", "-c", shell_command], "cwd": "/tmp", "cols": 80, "rows": 24}),
        );
        created["id"].as_str().expect("id").to_string()
    }

    pub fn create_shell(&self, client: &mut Client) -> String {
        let created = client.call(
            "session.create",
            json!({"cmd": ["/bin/sh"], "cwd": "/tmp", "cols": 80, "rows": 24, "title": "demo"}),
        );
        created["id"].as_str().expect("id").to_string()
    }

    pub fn holder_pid(&self, id: &str) -> u32 {
        let mut found = None;
        wait_until("holder process", || {
            found = self
                .session_pids()
                .into_iter()
                .find(|(sid, _)| sid == id)
                .map(|(_, pid)| pid);
            found.is_some()
        });
        found.expect("holder")
    }

    pub fn holders(&self) -> Vec<u32> {
        self.session_pids()
            .into_iter()
            .map(|(_, pid)| pid)
            .filter(|pid| alive(*pid))
            .collect()
    }

    fn session_pids(&self) -> Vec<(String, u32)> {
        std::fs::read(self.home().join("state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|v| v["sessions"].as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|s| {
                Some((
                    s["id"].as_str()?.to_string(),
                    s["holderPid"].as_u64()? as u32,
                ))
            })
            .collect()
    }
}

impl Sandbox {
    /// The pid the running daemon wrote into `daemon.lock`.
    pub fn lock_pid(&self) -> Option<u32> {
        std::fs::read_to_string(self.home().join("daemon.lock"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        shutdown_home(self.home());
        for daemon in &mut self.daemons {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
    }
}

pub struct Client {
    pub stream: UnixStream,
    pub decoder: Decoder,
    pub next_id: u64,
    /// Output frames seen so far: `(seq, bytes)`.
    pub output: Vec<(u64, Vec<u8>)>,
    /// Notifications seen so far: `(method, params)`.
    pub notes: Vec<(String, Value)>,
    /// Responses read while waiting for another one (or while pumping): a later `response`
    /// call for their id takes them from here instead of waiting for a frame that was read.
    pub unclaimed: Vec<sushiai_protocol::Response>,
}

impl Client {
    pub fn connect(sock: &Path) -> Client {
        let stream = UnixStream::connect(sock).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .expect("timeout");
        Client {
            stream,
            decoder: Decoder::new(),
            next_id: 1,
            output: Vec::new(),
            notes: Vec::new(),
            unclaimed: Vec::new(),
        }
    }

    pub fn frames(&mut self) -> Vec<Frame> {
        let mut buf = [0u8; 8192];
        match self.stream.read(&mut buf) {
            Ok(0) | Err(_) => Vec::new(),
            Ok(n) => self.decoder.push(&buf[..n]).expect("decode"),
        }
    }

    pub fn send(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let frame = Request::new(id, method, params).frame();
        self.stream.write_all(&encode(&frame)).expect("write");
        id
    }

    /// Sorts frames into output, notifications and (returned) responses.
    pub fn file(&mut self, frame: Frame) -> Option<sushiai_protocol::Response> {
        match frame {
            Frame::Output { seq, data, .. } => self.output.push((seq, data)),
            Frame::Json(text) => match Message::parse(&text) {
                Ok(Message::Response(r)) => return Some(r),
                Ok(Message::Notification(n)) => self.notes.push((n.method, n.params)),
                _ => {}
            },
        }
        None
    }

    pub fn try_call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.send(method, params);
        self.response(method, id)
    }

    /// Waits for the response to request `id`.
    pub fn response(&mut self, method: &str, id: u64) -> Result<Value, RpcError> {
        let deadline = Instant::now() + self::deadline();
        while Instant::now() < deadline {
            // File every frame of a read, so notifications that follow the response are kept,
            // and keep the responses to other requests for their own `response` call.
            for frame in self.frames() {
                if let Some(r) = self.file(frame) {
                    self.unclaimed.push(r);
                }
            }
            if let Some(at) = self.unclaimed.iter().position(|r| r.id == id) {
                let r = self.unclaimed.remove(at);
                return match r.error {
                    Some(e) => Err(e),
                    None => Ok(r.result.unwrap_or_default()),
                };
            }
        }
        panic!("no response to {method}");
    }

    pub fn call(&mut self, method: &str, params: Value) -> Value {
        self.try_call(method, params)
            .unwrap_or_else(|e| panic!("{method} failed: {e:?}"))
    }

    /// Reads for up to `secs` until `done` is true.
    pub fn pump_until(&mut self, what: &str, done: impl Fn(&Client) -> bool) {
        let deadline = Instant::now() + self::deadline();
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            for frame in self.frames() {
                if let Some(r) = self.file(frame) {
                    self.unclaimed.push(r);
                }
            }
        }
    }

    pub fn streamed(&self) -> String {
        let bytes: Vec<u8> = self.output.iter().flat_map(|(_, d)| d.clone()).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn wait_streamed(&mut self, needle: &str) {
        self.pump_until(needle, |c| c.streamed().contains(needle));
    }

    pub fn wait_note(&mut self, method: &str) -> Value {
        self.pump_until(method, |c| c.notes.iter().any(|n| n.0 == method));
        self.notes
            .iter()
            .find(|n| n.0 == method)
            .map(|n| n.1.clone())
            .expect("note")
    }

    pub fn type_text(&mut self, id: &str, text: &str) {
        self.call("session.input", json!({"id": id, "data": text}));
    }

    /// Attaches and returns the whole result.
    pub fn attach_result(&mut self, id: &str) -> Value {
        self.call("session.attach", json!({"id": id}))
    }

    /// Attaches and returns the snapshot bytes as text.
    pub fn attach(&mut self, id: &str) -> String {
        let result = self.attach_result(id);
        let encoded = result["snapshot"].as_str().expect("snapshot");
        String::from_utf8_lossy(&base64_decode(encoded)).into_owned()
    }
}

pub fn base64_decode(text: &str) -> Vec<u8> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let (mut out, mut acc, mut bits) = (Vec::new(), 0u32, 0);
    for c in text.bytes().filter(|c| *c != b'=') {
        acc = (acc << 6) | ALPHABET.iter().position(|a| *a == c).expect("base64") as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Waits until `session.list` shows `id` as running: after a daemon restart a session is
/// listed `detached` until its holder answered.
pub fn wait_listed(client: &mut Client, id: &str) {
    wait_until("the session to be running", || {
        client
            .call("session.list", Value::Null)
            .as_array()
            .is_some_and(|list| {
                list.iter()
                    .any(|s| s["id"] == id && s["status"] == "running")
            })
    });
}

/// Waits until `id` is listed and no longer `detached` (running or exited).
pub fn wait_settled(client: &mut Client, id: &str) {
    wait_until("the session to settle", || {
        client
            .call("session.list", Value::Null)
            .as_array()
            .is_some_and(|list| {
                list.iter()
                    .any(|s| s["id"] == id && s["status"] != "detached")
            })
    });
}

pub fn wait_snapshot_contains(client: &mut Client, id: &str, needle: &str) {
    let deadline = Instant::now() + self::deadline();
    loop {
        let snapshot = client.attach(id);
        if snapshot.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "snapshot never contained {needle}: {snapshot:?}"
        );
        sleep(Duration::from_millis(100));
    }
}

/// Typed so the echoed command line cannot contain the needle: only the output can.
pub fn print_marker(client: &mut Client, id: &str, n: u32) {
    client.type_text(id, &format!("printf 'mark%s\\n' er-{n}\n"));
}

/// Lowercase hex SHA-256 of `text`, through the system `shasum`.
pub fn sha256_hex(text: &str) -> String {
    use std::io::Write;
    let mut child = Command::new("shasum")
        .args(["-a", "256"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("shasum");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(text.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string()
}
