//! The module seam: routing `<ns>.*`, the hook-role refusal, panics and the capability list.
//! The daemon runs in-process on a thread with a synthetic module.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sushiai_daemon::{BoxFuture, Home, Module, ModuleNotify, ModuleSlot, Reply};
use sushiai_protocol::{code, encode, Decoder, Frame, Message, Request};

struct Probe {
    notify: ModuleNotify,
}

impl Module for Probe {
    fn namespace(&self) -> &'static str {
        "probe"
    }
    fn capability(&self) -> &'static str {
        "probing"
    }
    fn call(&self, method: &str, params: Value) -> BoxFuture<Reply> {
        let method = method.to_string();
        let notify = self.notify.clone();
        Box::pin(async move {
            match method.as_str() {
                "echo" => {
                    notify.send("event", json!({"saw": params}));
                    Ok(params)
                }
                "boom" => panic!("synthetic failure"),
                _ => Err((code::METHOD_NOT_FOUND, "no such method".into())),
            }
        })
    }
    fn shutdown(&self) -> BoxFuture<()> {
        Box::pin(async {})
    }
}

struct Running {
    dir: tempfile::TempDir,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    fn start(with_module: bool) -> Running {
        let dir = tempfile::Builder::new()
            .prefix("sm")
            .tempdir_in("/tmp")
            .expect("tempdir");
        let home = Home::new(dir.path().to_path_buf());
        let slots = if with_module {
            vec![ModuleSlot::new("probe", |notify| {
                Arc::new(Probe { notify })
            })]
        } else {
            Vec::new()
        };
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

fn capabilities(hello: &Value) -> Vec<&str> {
    hello["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

#[test]
fn a_module_call_is_routed_and_its_notification_arrives() {
    let daemon = Running::start(true);
    let (mut client, hello) = daemon.client(None);
    assert!(capabilities(&hello).contains(&"probing"));
    let reply = client.call("probe.echo", json!({"n": 1})).expect("echo");
    assert_eq!(reply, json!({"n": 1}));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !client.notes.iter().any(|(m, _)| m == "probe.event") {
        assert!(Instant::now() < deadline, "no probe.event");
        let _ = client.call("ping", Value::Null);
    }
    let err = client.call("probe.nope", Value::Null).expect_err("unknown");
    assert_eq!(err.0, code::METHOD_NOT_FOUND);
}

#[test]
fn a_hook_connection_is_refused_for_module_methods() {
    let daemon = Running::start(true);
    let (mut hook, _) = daemon.client(Some("hook"));
    let err = hook
        .call("probe.echo", json!({}))
        .expect_err("hook role refused");
    assert_eq!(err.0, code::UNAUTHORIZED);
}

#[test]
fn a_panicking_call_answers_internal_and_the_daemon_keeps_serving() {
    let daemon = Running::start(true);
    let (mut client, _) = daemon.client(None);
    let err = client.call("probe.boom", Value::Null).expect_err("panic");
    assert_eq!(err.0, code::INTERNAL);
    assert_eq!(
        client.call("probe.echo", json!(2)).expect("still serving"),
        json!(2)
    );
}

#[test]
fn a_daemon_without_a_module_has_no_module_capability_and_no_route() {
    let daemon = Running::start(false);
    let (mut client, hello) = daemon.client(None);
    assert!(!capabilities(&hello).contains(&"probing"));
    let err = client.call("probe.echo", json!({})).expect_err("no route");
    assert_eq!(err.0, code::METHOD_NOT_FOUND);
}
