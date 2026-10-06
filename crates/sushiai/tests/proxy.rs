//! `sushiai proxy` against real daemons under a temporary `SUSHIAI_HOME`.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sushiai_protocol::{encode, Decoder, Frame, Message, Request};

const BIN: &str = env!("CARGO_BIN_EXE_sushiai");

fn ps_rows() -> Vec<(u32, u32, String)> {
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

fn alive(pid: u32) -> bool {
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    !out.is_empty() && !out.starts_with('Z')
}

fn kill(pid: u32) {
    let _ = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .stderr(Stdio::null())
        .status();
}

fn wait_until(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        sleep(Duration::from_millis(50));
    }
}

/// A temp home. On drop it kills, by pid and without panicking, the proxies and daemons it
/// started and every holder (and its children) whose command line names this home.
struct Sandbox {
    dir: tempfile::TempDir,
    daemons: Vec<Child>,
    proxies: Vec<u32>,
}

impl Sandbox {
    fn new() -> Sandbox {
        let dir = tempfile::Builder::new()
            .prefix("sp")
            .tempdir_in("/tmp")
            .expect("tempdir");
        Sandbox {
            dir,
            daemons: Vec::new(),
            proxies: Vec::new(),
        }
    }

    fn home(&self) -> &Path {
        self.dir.path()
    }

    fn socket(&self) -> PathBuf {
        self.home().join("daemon.sock")
    }

    fn start_daemon(&mut self) {
        let child = Command::new(BIN)
            .arg("daemon")
            .env("SUSHIAI_HOME", self.home())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn daemon");
        self.daemons.push(child);
        let socket = self.socket();
        wait_until("daemon socket", 15, || UnixStream::connect(&socket).is_ok());
    }

    fn proxy(&mut self) -> Proxy {
        let proxy = Proxy::start(self.home());
        self.proxies.push(proxy.child.id());
        proxy
    }

    /// `(pid, ppid)` of every holder of this home.
    fn holders(&self) -> Vec<(u32, u32)> {
        let home = self.home().display().to_string();
        ps_rows()
            .into_iter()
            .filter(|(_, _, cmd)| cmd.contains("hold --id") && cmd.contains(&home))
            .map(|(pid, ppid, _)| (pid, ppid))
            .collect()
    }

    /// The daemon a proxy started is detached, so find it through the socket it listens on.
    fn daemon_pid(&self) -> Option<u32> {
        let out = Command::new("lsof")
            .arg("-t")
            .arg(self.socket())
            .output()
            .ok()?;
        let rows = ps_rows();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| l.trim().parse::<u32>().ok())
            .find(|pid| {
                rows.iter()
                    .any(|(p, _, cmd)| p == pid && cmd.ends_with("sushiai daemon"))
            })
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let daemon = self.daemon_pid();
        for pid in &self.proxies {
            kill(*pid);
        }
        for child in &mut self.daemons {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(pid) = daemon {
            kill(pid);
        }
        let all = ps_rows();
        for (pid, _) in self.holders() {
            for (child, ppid, _) in &all {
                if *ppid == pid {
                    kill(*child);
                }
            }
            kill(pid);
        }
    }
}

/// A `sushiai proxy` process spoken to through its stdio.
struct Proxy {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: Receiver<Frame>,
    notes: Vec<(String, Value)>,
    output: Vec<u8>,
    next_id: u64,
}

impl Proxy {
    fn start(home: &Path) -> Proxy {
        let mut child = Command::new(BIN)
            .arg("proxy")
            .env("SUSHIAI_HOME", home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn proxy");
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().expect("stdout");
        let (tx, frames) = channel();
        std::thread::spawn(move || {
            let mut decoder = Decoder::new();
            let mut buf = [0u8; 8192];
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                // A decode error means non-protocol bytes: drop the sender so the test fails.
                match decoder.push(&buf[..n]) {
                    Ok(frames) => frames.into_iter().for_each(|f| {
                        let _ = tx.send(f);
                    }),
                    Err(_) => break,
                }
            }
        });
        Proxy {
            child,
            stdin,
            frames,
            notes: Vec::new(),
            output: Vec::new(),
            next_id: 1,
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let frame = Request::new(id, method, params).frame();
        let bytes = encode(&frame);
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(&bytes).expect("write");
        stdin.flush().expect("flush");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let frame = match self.frames.recv_timeout(left) {
                Ok(frame) => frame,
                Err(RecvTimeoutError::Timeout) => panic!("no response to {method}"),
                Err(RecvTimeoutError::Disconnected) => panic!("proxy stdout ended in {method}"),
            };
            match frame {
                Frame::Output { data, .. } => self.output.extend(data),
                Frame::Json(text) => match Message::parse(&text) {
                    Ok(Message::Response(r)) if r.id == json!(id) => {
                        assert!(r.error.is_none(), "{method} failed: {:?}", r.error);
                        return r.result.unwrap_or_default();
                    }
                    Ok(Message::Notification(n)) => self.notes.push((n.method, n.params)),
                    _ => {}
                },
            }
        }
    }

    fn hello(&mut self) -> Value {
        self.call("hello", json!({"protocol": 1, "client": "proxy-test"}))
    }

    /// Reads frames until the streamed output contains `needle`.
    fn wait_streamed(&mut self, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !String::from_utf8_lossy(&self.output).contains(needle) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.frames.recv_timeout(left) {
                Ok(Frame::Output { data, .. }) => self.output.extend(data),
                Ok(_) => {}
                Err(_) => panic!("never streamed {needle}"),
            }
        }
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    fn wait_exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            assert!(Instant::now() < deadline, "proxy did not exit");
            sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn create_shell(proxy: &mut Proxy) -> String {
    let created = proxy.call(
        "session.create",
        json!({"cmd": ["/bin/sh"], "cwd": "/tmp", "cols": 80, "rows": 24}),
    );
    created["id"].as_str().expect("id").to_string()
}

#[test]
fn proxy_starts_the_daemon_and_relays_hello_and_list() {
    let sandbox = &mut Sandbox::new();
    assert!(UnixStream::connect(sandbox.socket()).is_err());
    let mut proxy = sandbox.proxy();
    let hello = proxy.hello();
    assert_eq!(hello["protocol"], 1);
    let list = proxy.call("session.list", json!({}));
    assert!(list.is_object() || list.is_array(), "{list}");
    assert!(UnixStream::connect(sandbox.socket()).is_ok());
}

#[test]
fn two_proxies_work_at_once_against_one_daemon() {
    let sandbox = &mut Sandbox::new();
    sandbox.start_daemon();
    let mut a = sandbox.proxy();
    let mut b = sandbox.proxy();
    assert_eq!(a.hello()["protocol"], 1);
    assert_eq!(b.hello()["protocol"], 1);
    let id = create_shell(&mut a);
    let listed = b.call("session.list", json!({}));
    assert!(listed.to_string().contains(&id), "{listed}");
    a.call("session.list", json!({}));
}

#[test]
fn create_input_and_attach_pass_through_the_proxy() {
    let sandbox = &mut Sandbox::new();
    sandbox.start_daemon();
    let mut proxy = sandbox.proxy();
    proxy.hello();
    let id = create_shell(&mut proxy);
    proxy.call("session.attach", json!({"id": id}));
    // Typed so the echoed command line cannot contain the needle: only the output can.
    proxy.call(
        "session.input",
        json!({"id": id, "data": "printf 'mark%s\\n' er-1\n"}),
    );
    proxy.wait_streamed("marker-1");
}

#[test]
fn closing_stdin_ends_the_proxy_and_leaves_daemon_and_sessions() {
    let sandbox = &mut Sandbox::new();
    sandbox.start_daemon();
    let daemon = sandbox.daemons[0].id();
    let mut proxy = sandbox.proxy();
    proxy.hello();
    let id = create_shell(&mut proxy);
    let holders: Vec<u32> = sandbox.holders().into_iter().map(|h| h.0).collect();
    assert_eq!(holders.len(), 1);
    proxy.close_stdin();
    assert!(proxy.wait_exit().success());
    assert!(alive(daemon));
    assert!(alive(holders[0]));
    let mut again = sandbox.proxy();
    again.hello();
    let listed = again.call("session.list", json!({}));
    assert!(listed.to_string().contains(&id), "{listed}");
}
