//! ci-crates: host
//! Real daemon and holder processes under a temporary `SUSHIAI_HOME`.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use common::*;
use serde_json::{json, Value};
use sushiai_protocol::{code, Frame, Message};

/// How long the daemon waits for a holder's answer (`ANSWER_TIMEOUT` in
/// `sushiai-daemon/src/session.rs`): a call that stalls on a stuck holder takes this long.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);
/// The grace between TERM and KILL on a graceful close (`TERM_GRACE` in the same file).
const TERM_GRACE: Duration = Duration::from_secs(5);

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
    wait_until("daemon to die", || !alive(first_daemon));
    holds_for(
        "the holder and shell outliving the daemon",
        ABSENCE_WINDOW,
        || alive(holder) && alive(shell),
    );
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
    wait_until("holder and shell to exit", || {
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
    wait_until("one daemon to give up", || {
        let exited = sandbox
            .daemons
            .iter_mut()
            .filter_map(|d| d.try_wait().expect("try_wait"))
            .count();
        exited >= 1
    });
    holds_for("exactly one daemon staying up", ABSENCE_WINDOW, || {
        sandbox
            .daemons
            .iter_mut()
            .filter_map(|d| d.try_wait().expect("try_wait").is_none().then_some(()))
            .count()
            == 1
    });
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
    wait_until("the survivor to answer", || {
        UnixStream::connect(sandbox.socket()).is_ok()
    });
}

#[test]
fn a_child_that_does_not_read_cannot_stall_the_daemon() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    // `ready` is printed after the tty is raw: only then does the child stop reading input.
    let id = sandbox.create(&mut client, "stty raw -echo; echo ready; sleep 30");
    let shell = child_of(sandbox.holder_pid(&id));
    wait_snapshot_contains(&mut client, &id, "ready");

    let chunk = "A".repeat(64 * 1024);
    let mut refused = 0;
    for _ in 0..32 {
        // 2 MiB in total
        let started = Instant::now();
        match client.try_call("session.input", json!({"id": id, "data": chunk})) {
            Ok(_) => {}
            Err(e) => {
                assert_eq!(e.code, code::INPUT_BACKPRESSURE, "{e:?}");
                refused += 1;
            }
        }
        // A call that waited for the stuck holder would take the whole answer timeout.
        assert!(started.elapsed() < ANSWER_TIMEOUT, "an input call hung");
    }
    assert!(refused > 0, "no input call was refused");

    // Another connection and the same one still get answers.
    let mut other = sandbox.client();
    let began = Instant::now();
    assert_eq!(
        other.call("session.list", Value::Null)[0]["id"],
        id.as_str()
    );
    assert!(began.elapsed() < ANSWER_TIMEOUT);

    let closing = Instant::now();
    client.call("session.close", json!({"id": id, "graceful": false}));
    wait_until("the stuck shell to die", || !alive(shell));
    // A forced close must not wait out the grace period of a graceful one.
    assert!(closing.elapsed() < TERM_GRACE);
}

#[test]
fn attach_response_comes_before_any_output() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    // Output flows constantly, so a chunk arrives almost the moment a stream starts.
    let id = sandbox.create(&mut client, "while :; do echo tick; sleep 0.001; done");
    wait_snapshot_contains(&mut client, &id, "tick");
    for _ in 0..30 {
        let mut fresh = sandbox.client();
        let request = fresh.send("session.attach", json!({"id": id}));
        let first = wait_value("a frame after the attach request", || {
            fresh.frames().into_iter().next()
        });
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
    wait_until("daemon to die", || !alive(daemon));

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
    wait_until("holders to be gone", || sandbox.holders().is_empty());
    assert_eq!(client.call("session.list", Value::Null), json!([]));
}

#[test]
fn a_terminal_size_outside_the_bounds_is_refused_on_create_and_resize() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    for (cols, rows) in [(65535, 65535), (1001, 24), (80, 501), (0, 24), (80, 0)] {
        let refused = client
            .try_call(
                "session.create",
                json!({"cmd": ["/bin/sh"], "cwd": "/tmp", "cols": cols, "rows": rows}),
            )
            .expect_err("an oversized grid must be refused");
        assert_eq!(
            refused.code,
            code::INVALID_PARAMS,
            "{cols}x{rows}: {refused:?}"
        );
    }
    let id = sandbox.create_shell(&mut client);
    for (cols, rows) in [(65535, 65535), (0, 24), (80, 0)] {
        let refused = client
            .try_call(
                "session.resize",
                json!({"id": id, "cols": cols, "rows": rows}),
            )
            .expect_err("a bad size must be refused");
        assert_eq!(
            refused.code,
            code::INVALID_PARAMS,
            "{cols}x{rows}: {refused:?}"
        );
    }
    assert_eq!(
        client
            .call("session.list", Value::Null)
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

#[test]
fn graceful_close_escalates_to_kill_after_the_grace_period() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create(&mut client, "trap '' TERM; echo armed; sleep 60");
    let target = child_of(sandbox.holder_pid(&id));
    // TERM before the trap is set would end the shell: wait until it is armed.
    wait_snapshot_contains(&mut client, &id, "armed");
    let closing = Instant::now();
    client.call("session.close", json!({"id": id, "graceful": true}));
    // Half of the daemon's TERM grace: the process that ignores TERM must still be there.
    holds_for(
        "a process that ignores TERM surviving half the grace",
        TERM_GRACE / 2,
        || alive(target),
    );
    wait_until("KILL after the grace period", || !alive(target));
    let took = closing.elapsed();
    assert!(took >= TERM_GRACE, "KILL came after {took:?}");
}

#[test]
fn output_written_while_the_daemon_is_dead_appears_after_reattach() {
    let mut sandbox = Sandbox::new();
    let daemon = sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create(&mut client, "sleep 2; echo late-out; sleep 30");
    kill(daemon, "-KILL");
    wait_until("daemon to die", || !alive(daemon));
    // The script's `sleep 2` is over once its last `sleep 30` runs: "late-out" is written.
    let shell = child_of(sandbox.holder_pid(&id));
    wait_until("the shell to have printed late-out", || {
        processes()
            .iter()
            .any(|(pid, ppid, argv)| (*pid == shell || *ppid == shell) && argv == "sleep 30")
    });
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
    // The session waits for a line, so it cannot exit before the attach.
    let id = sandbox.create(&mut client, "read x; exit 3");
    client.attach(&id);
    client.type_text(&id, "\n");
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
    wait_until("the exited session to stop serving", || {
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
    // The flood only has to dwarf every buffer on the way (the holder's 2 MiB ring and the
    // daemon's 4 MiB history budget). The emulator parses it at well under 1 MiB/s in a
    // debug build, so a bigger flood only makes the test slower, not stronger.
    let id = sandbox.create(
        &mut client,
        // After the marker the command waits on stdin: a fixed sleep would end the
        // session mid-test whenever a loaded machine needs longer than the sleep.
        "yes | head -c 16000000; printf \"mark%s\\n\" er-end; exec cat",
    );
    // An attached client that reads slowly, while the flood runs.
    client.attach(&id);
    let shell = child_of(sandbox.holder_pid(&id));
    wait_until("the flood to end", || {
        client.frames();
        !processes()
            .iter()
            .any(|(_, ppid, argv)| *ppid == shell && argv == "yes")
    });
    // The flood ends; the session is still running and a fresh attach shows the last line.
    let mut fresh = sandbox.client();
    wait_snapshot_contains(&mut fresh, &id, "marker-end");
    assert_eq!(
        fresh.call("session.list", Value::Null)[0]["status"],
        "running"
    );
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
    let mut buf = [0u8; 64];
    wait_value("the oversize connection to close", || {
        match bad.stream.read(&mut buf) {
            Ok(0) => Some(()),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => Some(()),
            _ => None,
        }
    });
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
        client.pump_until("session.exited", |c| {
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
