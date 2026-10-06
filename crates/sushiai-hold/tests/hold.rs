use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sushiai_hold::{start, Config};
use sushiai_protocol::{encode, Decoder, Frame, Message, Request};

struct Conn {
    stream: UnixStream,
    decoder: Decoder,
    next_id: u64,
}

impl Conn {
    fn connect(sock: &Path) -> Conn {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(stream) = UnixStream::connect(sock) {
                stream
                    .set_read_timeout(Some(Duration::from_millis(200)))
                    .expect("timeout");
                return Conn {
                    stream,
                    decoder: Decoder::new(),
                    next_id: 1,
                };
            }
            assert!(Instant::now() < deadline, "holder socket never appeared");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn send(&mut self, method: &str, params: Value) {
        let frame = Request::new(self.next_id, method, params).frame();
        self.next_id += 1;
        self.stream.write_all(&encode(&frame)).expect("write");
    }

    fn read(&mut self) -> Vec<Frame> {
        let mut buf = [0u8; 8192];
        match self.stream.read(&mut buf) {
            Ok(0) => Vec::new(),
            Ok(n) => self.decoder.push(&buf[..n]).expect("decode"),
            Err(_) => Vec::new(),
        }
    }

    /// Collects output bytes until they contain `needle`; returns them and any exit code seen.
    fn output_until(&mut self, needle: &str) -> (String, Option<i64>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let (mut text, mut exit) = (String::new(), None);
        while Instant::now() < deadline && !text.contains(needle) {
            for frame in self.read() {
                match frame {
                    Frame::Output { data, .. } => text.push_str(&String::from_utf8_lossy(&data)),
                    Frame::Json(json) => {
                        if let Ok(Message::Notification(n)) = Message::parse(&json) {
                            if n.method == "hold.exited" {
                                exit = n.params["code"].as_i64();
                            }
                        }
                    }
                }
            }
            if exit.is_some() {
                break;
            }
        }
        (text, exit)
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    sock: PathBuf,
    thread: Option<JoinHandle<()>>,
}

impl Fixture {
    fn start(cmd: &[&str]) -> Fixture {
        let dir = tempfile::Builder::new()
            .prefix("hd")
            .tempdir_in("/tmp")
            .expect("tempdir");
        let cfg = Config {
            id: "t1".into(),
            dir: dir.path().to_path_buf(),
            cols: 80,
            rows: 24,
            cwd: dir.path().to_path_buf(),
            cmd: cmd.iter().map(|s| s.to_string()).collect(),
        };
        let sock = dir.path().join("t1.sock");
        let running = start(cfg).expect("holder start");
        let thread = thread::spawn(move || running.serve().expect("holder serve"));
        Fixture {
            _dir: dir,
            sock,
            thread: Some(thread),
        }
    }

    fn finish(mut self) {
        self.thread
            .take()
            .expect("thread")
            .join()
            .expect("holder thread");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Best effort, never panics while unwinding: one connect attempt, every error ignored.
        // A KILL through the holder ends the shell, so the holder thread can stop.
        if self.thread.is_some() {
            if let Ok(mut stream) = UnixStream::connect(&self.sock) {
                let close = Request::new(1, "hold.close", json!({"signal": "KILL"})).frame();
                let _ = stream.write_all(&encode(&close));
            }
        }
    }
}

#[test]
fn input_output_replay_and_exit() {
    let fixture = Fixture::start(&["/bin/sh"]);
    let mut first = Conn::connect(&fixture.sock);
    first.send("hold.attach", json!({"fromSeq": 0}));
    first.send("hold.input", json!({"data": "ZWNobyBob2xkLTEK"})); // "echo hold-1\n"
    let (text, _) = first.output_until("hold-1");
    assert!(text.contains("hold-1"), "output: {text:?}");

    // A new connection replaces the old one and replays from the start.
    let mut second = Conn::connect(&fixture.sock);
    second.send("hold.attach", json!({"fromSeq": 0}));
    let (replayed, _) = second.output_until("hold-1");
    assert!(replayed.contains("hold-1"), "replay: {replayed:?}");

    second.send("hold.input", json!({"data": "ZXhpdCA3Cg=="})); // "exit 7\n"
    let (_, exit) = second.output_until("never-appears");
    assert_eq!(exit, Some(7));
    drop(second);
    fixture.finish();
}

fn config(dir: &Path, cmd: &[&str]) -> Config {
    Config {
        id: "t1".into(),
        dir: dir.to_path_buf(),
        cols: 80,
        rows: 24,
        cwd: dir.to_path_buf(),
        cmd: cmd.iter().map(|s| s.to_string()).collect(),
    }
}

fn response_error_code(frames: Vec<Frame>) -> Option<i64> {
    frames.into_iter().find_map(|frame| match frame {
        Frame::Json(json) => match Message::parse(&json) {
            Ok(Message::Response(r)) => r.error.map(|e| e.code),
            _ => None,
        },
        Frame::Output { .. } => None,
    })
}

#[test]
fn input_to_a_child_that_does_not_read_is_refused_and_close_still_works() {
    let fixture = Fixture::start(&["/bin/sh", "-c", "stty raw -echo; sleep 30"]);
    let mut conn = Conn::connect(&fixture.sock);
    conn.send("hold.attach", json!({"fromSeq": 0}));
    let chunk = "QUFB".repeat(16 * 1024); // 48 KiB of "AAA..." per request
    let mut refused = None;
    let started = Instant::now();
    for _ in 0..40 {
        conn.send("hold.input", json!({"data": chunk}));
        refused = refused.or(response_error_code(conn.read()));
    }
    for _ in 0..20 {
        refused = refused.or(response_error_code(conn.read()));
    }
    assert_eq!(refused, Some(sushiai_protocol::code::INPUT_BACKPRESSURE));
    conn.send("hold.close", json!({"signal": "KILL"}));
    let (_, exit) = conn.output_until("never-appears");
    assert!(exit.is_some(), "child did not exit after KILL");
    assert!(started.elapsed() < Duration::from_secs(10));
    drop(conn);
    fixture.finish();
}

#[test]
fn a_command_that_cannot_start_leaves_no_socket() {
    let dir = tempfile::Builder::new()
        .prefix("hd")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let err = match start(config(dir.path(), &["/no/such/command"])) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("started a missing command"),
    };
    assert!(err.contains("/no/such/command"), "{err}");
    assert!(!dir.path().join("t1.sock").exists());
}
