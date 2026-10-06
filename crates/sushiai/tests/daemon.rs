//! Real daemon and holder processes under a temporary `SUSHIAI_HOME`.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::thread::sleep;
use std::time::{Duration, Instant};

use common::*;
use serde_json::{json, Value};
use sushiai_protocol::{code, Frame, Message};

#[test]
fn sessions_survive_a_killed_daemon() {
    let mut sandbox = Sandbox::new();
    let first_daemon = sandbox.start_daemon();

    // a. create a shell, type into it, see the output in the snapshot
    let mut client = sandbox.client();
    let id = sandbox.create_shell(&mut client);
    print_marker(&mut client, &id, 1);
    wait_snapshot_contains(&mut client, &id, "marker-1");

    // b. SIGKILL the daemon; the holder and the shell stay
    let holder = sandbox.holder_pid(&id);
    let shell = child_of(holder);
    kill(first_daemon, "-KILL");
    wait_until("daemon to die", 5, || !alive(first_daemon));
    sleep(Duration::from_millis(300));
    assert!(alive(holder), "holder died with the daemon");
    assert!(alive(shell), "shell died with the daemon");

    // c. a new daemon finds the session again
    sandbox.start_daemon();
    let mut client = sandbox.client();
    wait_listed(&mut client, &id);
    let list = client.call("session.list", Value::Null);
    assert_eq!(list[0]["id"], id.as_str());
    assert_eq!(list[0]["status"], "running");
    assert!(client.attach(&id).contains("marker-1"));

    // d. input still works; the stream and a new snapshot show it
    print_marker(&mut client, &id, 2);
    client.wait_streamed("marker-2");
    wait_snapshot_contains(&mut client, &id, "marker-2");

    // resize reaches the screen only through the holder
    client.call("session.resize", json!({"id": id, "cols": 100, "rows": 30}));
    let attached = client.attach_result(&id);
    assert_eq!(
        (attached["cols"].as_u64(), attached["rows"].as_u64()),
        (Some(100), Some(30))
    );

    // e. close ends the holder and the shell
    client.call("session.close", json!({"id": id, "graceful": false}));
    wait_until("holder and shell to exit", 5, || {
        !alive(holder) && !alive(shell)
    });
}

#[test]
fn second_daemon_is_refused() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let (status, stderr) = sandbox.daemon_that_exits();
    assert!(!status.success(), "second daemon started");
    assert!(stderr.contains("already running"), "stderr: {stderr}");
    assert_eq!(
        sandbox.client().call("session.list", Value::Null),
        json!([])
    );
}

#[test]
fn two_daemons_started_together_leave_exactly_one() {
    let mut sandbox = Sandbox::new();
    for _ in 0..2 {
        let child = sandbox.command().spawn().expect("spawn daemon");
        sandbox.daemons.push(child);
    }
    wait_until("one daemon to give up", 10, || {
        let exited = sandbox
            .daemons
            .iter_mut()
            .filter_map(|d| d.try_wait().expect("try_wait"))
            .count();
        exited >= 1
    });
    sleep(Duration::from_millis(500));
    let statuses: Vec<_> = sandbox
        .daemons
        .iter_mut()
        .map(|d| d.try_wait().expect("try_wait"))
        .collect();
    assert_eq!(
        statuses.iter().filter(|s| s.is_none()).count(),
        1,
        "{statuses:?}"
    );
    assert!(statuses.iter().flatten().all(|s| !s.success()));
    wait_until("the survivor to answer", 10, || {
        UnixStream::connect(sandbox.socket()).is_ok()
    });
}

#[test]
fn a_child_that_does_not_read_cannot_stall_the_daemon() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create(&mut client, "stty raw -echo; sleep 30");
    let shell = child_of(sandbox.holder_pid(&id));
    sleep(Duration::from_millis(500));

    let chunk = "A".repeat(64 * 1024);
    let mut refused = 0;
    let started = Instant::now();
    for _ in 0..32 {
        // 2 MiB in total
        match client.try_call("session.input", json!({"id": id, "data": chunk})) {
            Ok(_) => {}
            Err(e) => {
                assert_eq!(e.code, code::INPUT_BACKPRESSURE, "{e:?}");
                refused += 1;
            }
        }
    }
    assert!(refused > 0, "no input call was refused");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "input calls hung"
    );

    // Another connection and the same one still get answers.
    let mut other = sandbox.client();
    let began = Instant::now();
    assert_eq!(
        other.call("session.list", Value::Null)[0]["id"],
        id.as_str()
    );
    assert!(began.elapsed() < Duration::from_secs(2));

    let closing = Instant::now();
    client.call("session.close", json!({"id": id, "graceful": false}));
    wait_until("the stuck shell to die", 2, || !alive(shell));
    assert!(closing.elapsed() < Duration::from_secs(2));
}

#[test]
fn attach_response_comes_before_any_output() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    // Output flows constantly, so a chunk arrives almost the moment a stream starts.
    let id = sandbox.create(&mut client, "while :; do echo tick; sleep 0.001; done");
    sleep(Duration::from_millis(300));
    for _ in 0..30 {
        let mut fresh = sandbox.client();
        let request = fresh.send("session.attach", json!({"id": id}));
        let deadline = Instant::now() + Duration::from_secs(10);
        let first = loop {
            let frames = fresh.frames();
            if let Some(first) = frames.into_iter().next() {
                break first;
            }
            assert!(
                Instant::now() < deadline,
                "no frame after the attach request"
            );
        };
        match first {
            Frame::Json(text) => match Message::parse(&text) {
                Ok(Message::Response(r)) => assert_eq!(r.id, request),
                other => panic!("first frame was {other:?}"),
            },
            Frame::Output { .. } => panic!("output came before the attach response"),
        }
    }
    client.call("session.close", json!({"id": id, "graceful": false}));
}

#[test]
fn a_corrupt_state_file_is_set_aside_and_sessions_are_found_again() {
    let mut sandbox = Sandbox::new();
    let daemon = sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create_shell(&mut client);
    print_marker(&mut client, &id, 1);
    wait_snapshot_contains(&mut client, &id, "marker-1");
    kill(daemon, "-KILL");
    wait_until("daemon to die", 5, || !alive(daemon));

    fs::write(sandbox.home().join("state.json"), b"{ not json").expect("corrupt");
    sandbox.start_daemon();
    let mut client = sandbox.client();
    wait_listed(&mut client, &id);
    let list = client.call("session.list", Value::Null);
    assert_eq!(list[0]["id"], id.as_str());
    assert_eq!(list[0]["status"], "running");
    assert!(client.attach(&id).contains("marker-1"));
    let aside = fs::read_dir(sandbox.home())
        .expect("read home")
        .flatten()
        .any(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("state.json.corrupt-")
        });
    assert!(aside, "corrupt file was not set aside");
    client.call("session.close", json!({"id": id, "graceful": false}));
}

#[test]
fn a_newer_state_schema_stops_the_daemon() {
    let mut sandbox = Sandbox::new();
    let state = sandbox.home().join("state.json");
    let original = br#"{"schemaVersion":99,"sessions":[]}"#;
    fs::write(&state, original).expect("write state");
    let (status, stderr) = sandbox.daemon_that_exits();
    assert!(!status.success());
    assert!(stderr.contains("schema"), "stderr: {stderr}");
    assert_eq!(fs::read(&state).expect("read state"), original);
}

#[test]
fn a_home_that_is_a_symlink_is_refused_and_left_alone() {
    let sandbox = Sandbox::new();
    let target = sandbox.home().join("real");
    fs::create_dir(&target).expect("mkdir");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).expect("chmod");
    let link = sandbox.home().join("link");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    let out = std::process::Command::new(BIN)
        .arg("daemon")
        .env("SUSHIAI_HOME", &link)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run daemon");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("not safe"), "stderr: {stderr}");
    let mode = fs::metadata(&target).expect("meta").permissions().mode();
    assert_eq!(mode & 0o777, 0o755, "the target was chmod-ed");
    assert!(!target.join("daemon.lock").exists());
}

#[test]
fn a_session_that_cannot_start_reports_why_and_leaves_nothing() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let missing = client
        .try_call(
            "session.create",
            json!({"cmd": ["/no/such/command"], "cwd": "/tmp", "cols": 80, "rows": 24}),
        )
        .expect_err("a missing command must fail");
    assert_eq!(missing.code, code::SPAWN_FAILED);
    assert!(missing.message.contains("/no/such/command"), "{missing:?}");
    let bad_cwd = client
        .try_call(
            "session.create",
            json!({"cmd": ["/bin/sh"], "cwd": "/no/such/dir", "cols": 80, "rows": 24}),
        )
        .expect_err("a missing cwd must fail");
    assert_eq!(bad_cwd.code, code::SPAWN_FAILED);
    assert!(bad_cwd.message.contains("/no/such/dir"), "{bad_cwd:?}");

    let sockets: Vec<_> = fs::read_dir(sandbox.home().join("sessions"))
        .expect("sessions dir")
        .flatten()
        .collect();
    assert!(sockets.is_empty(), "leftover files: {sockets:?}");
    wait_until("holders to be gone", 5, || sandbox.holders().is_empty());
    assert_eq!(client.call("session.list", Value::Null), json!([]));
}

#[test]
fn graceful_close_escalates_to_kill_after_the_grace_period() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create(&mut client, "trap '' TERM; sleep 60");
    let target = child_of(sandbox.holder_pid(&id));
    let closing = Instant::now();
    client.call("session.close", json!({"id": id, "graceful": true}));
    sleep(Duration::from_secs(3));
    assert!(alive(target), "TERM alone ended a process that ignores it");
    wait_until("KILL after the grace period", 7, || !alive(target));
    let took = closing.elapsed();
    assert!(
        took >= Duration::from_millis(4500) && took <= Duration::from_secs(8),
        "took {took:?}"
    );
}

#[test]
fn output_written_while_the_daemon_is_dead_appears_after_reattach() {
    let mut sandbox = Sandbox::new();
    let daemon = sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create(&mut client, "sleep 2; echo late-out; sleep 30");
    kill(daemon, "-KILL");
    wait_until("daemon to die", 5, || !alive(daemon));
    sleep(Duration::from_secs(3));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    wait_listed(&mut client, &id);
    wait_snapshot_contains(&mut client, &id, "late-out");
    client.call("session.close", json!({"id": id, "graceful": false}));
}

#[test]
fn exit_code_arrives_as_session_exited_and_the_session_is_then_not_running() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create(&mut client, "sleep 1; exit 3");
    client.attach(&id);
    let note = client.wait_note("session.exited");
    assert_eq!(note["id"], id.as_str());
    assert_eq!(note["code"], 3);
    let list = client.call("session.list", Value::Null);
    assert_eq!(list[0]["status"], "exited");
    assert_eq!(list[0]["exitCode"], 3);
    let refused = client
        .try_call("session.input", json!({"id": id, "data": "x"}))
        .expect_err("input to an exited session must fail");
    assert_eq!(refused.code, code::SESSION_NOT_RUNNING);

    // With nobody attached the exited session's actor ends; the record stays.
    drop(client);
    // Each probe uses a fresh connection: an attach that succeeds keeps the actor alive only
    // until that connection closes.
    wait_until("the exited session to stop serving", 8, || {
        sandbox
            .client()
            .try_call("session.attach", json!({"id": id}))
            .is_err_and(|e| e.code == code::SESSION_NOT_RUNNING)
    });
    let mut other = sandbox.client();
    assert_eq!(
        other.call("session.list", Value::Null)[0]["status"],
        "exited"
    );
}

#[test]
fn a_flooding_session_stays_running_and_its_screen_keeps_up() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create(
        &mut client,
        "yes | head -c 50000000; printf \"mark%s\\n\" er-end; sleep 30",
    );
    // An attached client that reads slowly, while the flood runs.
    client.attach(&id);
    let reading = Instant::now();
    while reading.elapsed() < Duration::from_secs(8) {
        sleep(Duration::from_millis(100));
        client.frames();
    }
    // The flood ends; the session is still running and a fresh attach shows the last line.
    let mut fresh = sandbox.client();
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let status = fresh.call("session.list", Value::Null)[0]["status"].clone();
        if status == "running" && fresh.attach(&id).contains("marker-end") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "never reached marker-end with status running (last status {status})"
        );
        sleep(Duration::from_millis(500));
    }
    client.call("session.close", json!({"id": id, "graceful": false}));
}

#[test]
fn state_and_lock_files_are_owner_only() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create_shell(&mut client);
    for name in ["state.json", "daemon.lock"] {
        let mode = fs::metadata(sandbox.home().join(name))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "{name} mode {mode:o}");
    }
    client.call("session.close", json!({"id": id, "graceful": false}));
}

#[test]
fn an_oversize_frame_closes_only_that_connection() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut bad = Client::connect(&sandbox.socket());
    let header = ((16 * 1024 * 1024 + 1) as u32).to_be_bytes();
    bad.stream.write_all(&header).expect("write header");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut buf = [0u8; 64];
    loop {
        match bad.stream.read(&mut buf) {
            Ok(0) => break,
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            _ => {}
        }
        assert!(Instant::now() < deadline, "the connection stayed open");
    }
    let mut good = sandbox.client();
    assert_eq!(good.call("session.list", Value::Null), json!([]));
}

#[test]
fn a_killed_holder_ends_its_session_and_removes_its_socket() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create(&mut client, "sleep 300");
    let holder = sandbox.holder_pid(&id);
    let socket = sandbox.home().join("sessions").join(format!("{id}.sock"));
    assert!(socket.exists());
    kill(holder, "-KILL");
    let note = {
        client.pump_until("session.exited", 5, |c| {
            c.notes.iter().any(|n| n.0 == "session.exited")
        });
        client.wait_note("session.exited")
    };
    assert_eq!(note["id"], id.as_str());
    assert!(!socket.exists(), "the stale socket was left behind");
    assert_eq!(
        client.call("session.list", Value::Null)[0]["status"],
        "exited"
    );
}

#[test]
fn hello_reports_the_sha256_of_the_daemon_binary_as_build() {
    use sha2::{Digest, Sha256};
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let hello = client.call("hello", json!({"protocol": 1, "client": "test"}));
    let expected: String = Sha256::digest(std::fs::read(common::BIN).expect("binary"))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hello["build"], expected.as_str());
}
