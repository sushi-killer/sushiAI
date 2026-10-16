//! Schedules and owner asks over the socket: the daemon runs in-process on a thread.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sushiai_daemon::Home;
use sushiai_protocol::{code, encode, Decoder, Frame, Message, Request};

struct Running {
    dir: tempfile::TempDir,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    fn start() -> Running {
        let dir = tempfile::Builder::new()
            .prefix("sm")
            .tempdir_in("/tmp")
            .expect("tempdir");
        let home = Home::new(dir.path().to_path_buf());
        let slots = Vec::new();
        let thread = std::thread::spawn(move || {
            sushiai_daemon::run_blocking(home, slots).expect("daemon");
        });
        let running = Running {
            dir,
            thread: Some(thread),
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        while UnixStream::connect(running.socket()).is_err() {
            assert!(Instant::now() < deadline, "daemon socket");
            std::thread::sleep(Duration::from_millis(50));
        }
        running
    }

    fn socket(&self) -> PathBuf {
        self.dir.path().join("daemon.sock")
    }

    fn client(&self, role: Option<&str>) -> (Client, Value) {
        let mut client = Client {
            stream: UnixStream::connect(self.socket()).expect("connect"),
            decoder: Decoder::new(),
            next: 1,
            notes: Vec::new(),
        };
        client
            .stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .expect("timeout");
        let hello = client
            .call("hello", json!({"protocol": 1, "client": "t", "role": role}))
            .expect("hello");
        (client, hello)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let (mut client, _) = self.client(None);
        let _ = client.call("daemon.shutdown", Value::Null);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Client {
    stream: UnixStream,
    decoder: Decoder,
    next: u64,
    notes: Vec<(String, Value)>,
}

impl Client {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, (i64, String)> {
        let id = self.next;
        self.next += 1;
        let frame = Request::new(id, method, params).frame();
        self.stream.write_all(&encode(&frame)).expect("write");
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let mut buf = [0u8; 8192];
            let Ok(n) = self.stream.read(&mut buf) else {
                continue;
            };
            assert!(n > 0, "daemon closed the connection");
            let mut answer = None;
            for frame in self.decoder.push(&buf[..n]).expect("decode") {
                let Frame::Json(text) = frame else { continue };
                match Message::parse(&text) {
                    Ok(Message::Response(r)) if r.id == id => answer = Some(r),
                    Ok(Message::Notification(n)) => self.notes.push((n.method, n.params)),
                    _ => {}
                }
            }
            if let Some(r) = answer {
                return match r.error {
                    Some(e) => Err((e.code, e.message)),
                    None => Ok(r.result.unwrap_or_default()),
                };
            }
        }
        panic!("no response to {method}");
    }
}

fn entry(id: &str, cron: &str) -> Value {
    json!({
        "id": id,
        "cron": cron,
        "tz": "Europe/Berlin",
        "overlap": "skip",
        "create": {
            "cmd": ["/bin/sh", "-c", "sleep 30"],
            "cwd": "/tmp",
            "cols": 80,
            "rows": 24,
            "background": true,
            "env": { "SYNTHETIC_TOKEN": "x" }
        }
    })
}

fn wait_for(client: &mut Client, method: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(at) = client.notes.iter().position(|(m, _)| m == method) {
            return client.notes.remove(at).1;
        }
        assert!(Instant::now() < deadline, "no {method}");
        let _ = client.call("ping", Value::Null);
    }
}

#[test]
fn set_list_fire_and_remove_over_the_socket() {
    let daemon = Running::start();
    let (mut client, hello) = daemon.client(None);
    let caps = hello["capabilities"].to_string();
    assert!(caps.contains("schedules") && caps.contains("ownerAsk"));

    client
        .call("schedule.set", entry("nightly", "0 3 * * *"))
        .expect("set");
    let listed = client.call("schedule.list", Value::Null).expect("list");
    assert_eq!(listed[0]["id"], "nightly");
    assert_eq!(listed[0]["tz"], "Europe/Berlin");
    assert!(listed[0]["nextAt"].as_u64().is_some());
    assert!(
        listed[0]["create"].get("env").is_none() || listed[0]["create"]["env"] == json!({}),
        "env is not listed"
    );

    // A test binary cannot be a session holder, so the start fails here: the entry
    // reports it as a skipped run (starting real sessions is covered by the desktop smoke).
    let fired = client
        .call("schedule.fire", json!({"id": "nightly"}))
        .expect("fire");
    assert!(fired["session"].is_null());
    assert!(fired["skipped"]
        .as_str()
        .is_some_and(|r| r.starts_with("could not start")));
    let event = wait_for(&mut client, "schedule.event");
    assert_eq!(event["kind"], "skipped");
    assert_eq!(event["id"], "nightly");
    assert!(event["scheduledFor"].as_u64().is_some());
    assert!(event["startedAt"].as_u64().is_some());

    client
        .call("schedule.remove", json!({"id": "nightly"}))
        .expect("remove");
    let err = client
        .call("schedule.remove", json!({"id": "nightly"}))
        .expect_err("gone");
    assert_eq!(err.0, code::SCHEDULE_NOT_FOUND);
    let err = client
        .call("schedule.fire", json!({"id": "nightly"}))
        .expect_err("gone");
    assert_eq!(err.0, code::SCHEDULE_NOT_FOUND);
}

#[test]
fn a_bad_entry_is_refused_and_not_stored() {
    let daemon = Running::start();
    let (mut client, _) = daemon.client(None);
    for (id, cron) in [("a", "*/2 * * * *"), ("b", "61 * * * *"), ("c", "0 0 *")] {
        let err = client
            .call("schedule.set", entry(id, cron))
            .expect_err("refused");
        assert_eq!(err.0, code::INVALID_PARAMS, "{cron}");
    }
    let mut zone = entry("z", "0 9 * * *");
    zone["tz"] = json!("Nowhere/Land");
    assert_eq!(
        client.call("schedule.set", zone).expect_err("zone").0,
        code::INVALID_PARAMS
    );
    assert_eq!(
        client.call("schedule.list", Value::Null).expect("list"),
        json!([])
    );
}

#[test]
fn an_owner_ask_needs_a_hook_connection_and_a_valid_token() {
    let daemon = Running::start();
    let (mut client, _) = daemon.client(None);
    let params = json!({"session": "s", "token": "t", "agent": "claude", "text": "ok?"});
    let err = client
        .call("hook.ask", params.clone())
        .expect_err("not a hook connection");
    assert_eq!(err.0, code::UNAUTHORIZED);
    let (mut hook, _) = daemon.client(Some("hook"));
    let err = hook.call("hook.ask", params).expect_err("bad token");
    assert_eq!(err.0, code::UNAUTHORIZED);
}
