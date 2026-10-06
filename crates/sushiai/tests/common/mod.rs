//! Shared helpers for the end-to-end tests: real daemon and holder processes under a
//! temporary `SUSHIAI_HOME`.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sushiai_protocol::{encode, Decoder, Frame, Message, Request, RpcError};

pub const BIN: &str = env!("CARGO_BIN_EXE_sushiai");

pub fn wait_until(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        sleep(Duration::from_millis(50));
    }
}

/// `(pid, ppid, command line)` of every process, from `ps`. Never panics: it also runs in Drop.
pub fn processes() -> Vec<(u32, u32, String)> {
    let Ok(out) = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,command="])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let ppid = parts.next()?.parse().ok()?;
            Some((pid, ppid, parts.collect::<Vec<_>>().join(" ")))
        })
        .collect()
}

pub fn alive(pid: u32) -> bool {
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    !out.is_empty() && !out.starts_with('Z')
}

pub fn kill(pid: u32, signal: &str) {
    let _ = Command::new("kill")
        .args([signal, &pid.to_string()])
        .status();
}

/// The child of `parent` (a holder's shell).
pub fn child_of(parent: u32) -> u32 {
    let mut found = None;
    wait_until("the holder's child", 5, || {
        found = processes()
            .into_iter()
            .find(|(_, ppid, _)| *ppid == parent)
            .map(|p| p.0);
        found.is_some()
    });
    found.expect("child")
}

/// A temp home. On drop it kills, by pid and without panicking, the daemons it started and
/// every holder (and its children) whose command line names this home.
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
        command.arg("daemon").env("SUSHIAI_HOME", self.home());
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
        wait_until("daemon socket", 15, || {
            let child = &mut self.daemons[last];
            assert!(
                child.try_wait().expect("try_wait").is_none(),
                "daemon exited early"
            );
            UnixStream::connect(&socket).is_ok()
        });
        pid
    }

    /// Runs a daemon that is expected to exit on its own; returns its status and stderr.
    pub fn daemon_that_exits(&mut self) -> (ExitStatus, String) {
        let mut command = self.command();
        command.stderr(Stdio::piped());
        let mut child = command.spawn().expect("spawn daemon");
        let deadline = Instant::now() + Duration::from_secs(10);
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
        let needle = format!("hold --id {id}");
        let mut found = None;
        wait_until("holder process", 5, || {
            found = processes()
                .into_iter()
                .find(|(_, _, cmd)| cmd.contains(&needle))
                .map(|p| p.0);
            found.is_some()
        });
        found.expect("holder")
    }

    pub fn holders(&self) -> Vec<u32> {
        let home = self.home().display().to_string();
        processes()
            .into_iter()
            .filter(|(_, _, cmd)| cmd.contains("hold --id") && cmd.contains(&home))
            .map(|p| p.0)
            .collect()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        for daemon in &mut self.daemons {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        let all = processes();
        for pid in self.holders() {
            for (child, ppid, _) in &all {
                if *ppid == pid {
                    kill(*child, "-KILL");
                }
            }
            kill(pid, "-KILL");
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
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            // File every frame of a read, so notifications that follow the response are kept.
            let mut answer = None;
            for frame in self.frames() {
                if let Some(r) = self.file(frame) {
                    if r.id == id {
                        answer = Some(r);
                    }
                }
            }
            if let Some(r) = answer {
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
    pub fn pump_until(&mut self, what: &str, secs: u64, done: impl Fn(&Client) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            for frame in self.frames() {
                self.file(frame);
            }
        }
    }

    pub fn streamed(&self) -> String {
        let bytes: Vec<u8> = self.output.iter().flat_map(|(_, d)| d.clone()).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn wait_streamed(&mut self, needle: &str) {
        self.pump_until(needle, 15, |c| c.streamed().contains(needle));
    }

    pub fn wait_note(&mut self, method: &str) -> Value {
        self.pump_until(method, 15, |c| c.notes.iter().any(|n| n.0 == method));
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

pub fn wait_snapshot_contains(client: &mut Client, id: &str, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);
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
