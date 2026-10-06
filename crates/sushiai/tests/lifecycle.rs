//! Catalog wiring, lifecycle notifications, daemon control and recovery, against real
//! daemon and holder processes under a temporary `SUSHIAI_HOME`.

mod common;

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use common::*;
use serde_json::{json, Value};
use sushiai_protocol::{code, encode, Decoder, Frame, Message, Request};

/// A sandbox whose daemon is the replica of host `devbox`.
fn devbox() -> Sandbox {
    let mut sandbox = Sandbox::new();
    sandbox.env.push(("SUSHIAI_HOST".into(), "devbox".into()));
    sandbox
}

fn project(id: &str, path: &str) -> Value {
    json!({"id": id, "name": id, "folders": [{"host": "devbox", "path": path}],
           "rev": 1, "updatedAt": 1_700_000_000_000u64, "deleted": false})
}

fn group(id: &str, project: &str) -> Value {
    json!({"id": id, "projectId": project, "name": id, "order": 0,
           "rev": 1, "updatedAt": 1_700_000_000_000u64, "deleted": false})
}

fn sync_catalog(client: &mut Client, folder: &str) {
    let result = client.call(
        "projects.sync",
        json!({"host": "devbox", "full": true, "projects": [project("p1", folder), project("p2", "/elsewhere")]}),
    );
    assert_eq!(result["applied"], 2, "{result}");
    let result = client.call(
        "groups.sync",
        json!({"full": true, "groups": [group("g1", "p1"), group("g2", "p2")]}),
    );
    assert_eq!(result["applied"], 2, "{result}");
}

fn create_in(
    client: &mut Client,
    cwd: &str,
    extra: Value,
) -> Result<String, sushiai_protocol::RpcError> {
    let mut params =
        json!({"cmd": ["/bin/sh", "-c", "sleep 300"], "cwd": cwd, "cols": 80, "rows": 24});
    for (k, v) in extra.as_object().expect("object") {
        params[k] = v.clone();
    }
    client
        .try_call("session.create", params)
        .map(|r| r["id"].as_str().expect("id").to_string())
}

fn info_of(client: &mut Client, id: &str) -> Value {
    client
        .call("session.list", Value::Null)
        .as_array()
        .expect("list")
        .iter()
        .find(|s| s["id"] == id)
        .cloned()
        .expect("session in list")
}

#[test]
fn hello_reports_the_host_and_the_catalog_capability() {
    let mut sandbox = devbox();
    sandbox.start_daemon();
    let mut client = Client::connect(&sandbox.socket());
    let hello = client.call("hello", json!({"protocol": 1, "client": "test"}));
    assert_eq!(hello["host"], "devbox");
    assert!(hello["capabilities"].to_string().contains("catalog"));
}

#[test]
fn ping_answers_even_before_hello() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = Client::connect(&sandbox.socket());
    assert_eq!(client.call("$/ping", Value::Null), json!({}));
}

#[test]
fn a_session_is_bound_by_cwd_or_by_request_and_bindings_are_stored_as_given() {
    let mut sandbox = devbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let work = sandbox.home().join("work");
    fs::create_dir_all(work.join("sub")).expect("mkdir");
    let work = work.to_str().expect("utf8").to_string();
    sync_catalog(&mut client, &work);

    // No project given: the one whose folder holds the cwd.
    let sub = format!("{work}/sub");
    let auto = create_in(&mut client, &sub, json!({})).expect("create");
    assert_eq!(info_of(&mut client, &auto)["project"], "p1");
    // Outside every folder: unbound.
    let free = create_in(&mut client, "/tmp", json!({})).expect("create");
    assert!(info_of(&mut client, &free).get("project").is_none());
    // Explicit binding.
    let bound =
        create_in(&mut client, "/tmp", json!({"project": "p2", "group": "g2"})).expect("create");
    let info = info_of(&mut client, &bound);
    assert_eq!(
        (info["project"].as_str(), info["group"].as_str()),
        (Some("p2"), Some("g2"))
    );

    // Bindings are opaque: ids the catalog does not know are stored as given, so a launch
    // never waits for a sync. This sandbox's catalog knows neither "later" nor "g-later".
    let opaque = create_in(
        &mut client,
        &sub,
        json!({"project": "later", "group": "g-later"}),
    )
    .expect("an unsynced binding must not block a launch");
    let info = info_of(&mut client, &opaque);
    assert_eq!(
        (info["project"].as_str(), info["group"].as_str()),
        (Some("later"), Some("g-later"))
    );
    // A mismatched pair is stored too: the desktop reconciles.
    let odd =
        create_in(&mut client, "/tmp", json!({"project": "p1", "group": "g2"})).expect("create");
    assert_eq!(info_of(&mut client, &odd)["group"], "g2");

    // session.update: set, clear, title; the result is the new SessionInfo.
    let updated = client.call(
        "session.update",
        json!({"id": free, "project": "p1", "group": "g1", "title": "renamed"}),
    );
    assert_eq!(updated["group"], "g1");
    assert_eq!(updated["title"], "renamed");
    let moved = client.call("session.update", json!({"id": free, "project": "p2"}));
    assert_eq!(moved["project"], "p2");
    client.call("session.update", json!({"id": free, "project": "p1"}));
    // A title change alone leaves the binding alone.
    client.call("session.update", json!({"id": free, "title": "again"}));
    assert_eq!(info_of(&mut client, &free)["group"], "g1");
    client.call(
        "session.update",
        json!({"id": free, "project": null, "group": null}),
    );
    let cleared = info_of(&mut client, &free);
    assert!(cleared.get("project").is_none() && cleared.get("group").is_none());
    let err = client
        .try_call("session.update", json!({"id": "missing", "title": "x"}))
        .expect_err("unknown session");
    assert_eq!(err.code, code::SESSION_NOT_FOUND);

    // The binding survives the actor's own updates (status, resize).
    client.call(
        "session.resize",
        json!({"id": auto, "cols": 90, "rows": 30}),
    );
    assert_eq!(info_of(&mut client, &auto)["project"], "p1");
    for id in [&auto, &free, &bound, &opaque, &odd] {
        client.call("session.close", json!({"id": id, "graceful": false}));
    }
}

#[test]
fn the_catalog_and_bindings_survive_a_daemon_restart() {
    let mut sandbox = devbox();
    let daemon = sandbox.start_daemon();
    let mut client = sandbox.client();
    let work = sandbox.home().join("work");
    fs::create_dir_all(&work).expect("mkdir");
    let work = work.to_str().expect("utf8").to_string();
    sync_catalog(&mut client, &work);
    let id = create_in(&mut client, &work, json!({"group": "g1"})).expect("create");
    client.call("session.update", json!({"id": id, "title": "kept"}));
    let mode = fs::metadata(sandbox.home().join("catalog.json"))
        .expect("catalog file")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    // The state file is written by a background thread: wait for it before the kill.
    wait_until("the state file to hold the update", 5, || {
        fs::read_to_string(sandbox.home().join("state.json")).is_ok_and(|t| t.contains("kept"))
    });
    kill(daemon, "-KILL");
    wait_until("daemon to die", 5, || !alive(daemon));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    wait_until("the session to come back", 10, || {
        info_of(&mut sandbox.client(), &id)["status"] == "running"
    });
    let info = info_of(&mut client, &id);
    assert_eq!(
        (info["project"].as_str(), info["group"].as_str()),
        (Some("p1"), Some("g1"))
    );
    assert_eq!(info["title"], "kept");
    // The replica was loaded: cwd matching still works.
    let again = create_in(&mut client, &work, json!({})).expect("create");
    assert_eq!(info_of(&mut client, &again)["project"], "p1");
    // A sync addressed to another host is an error, not a silent no-op.
    let err = client
        .try_call(
            "projects.sync",
            json!({"host": "other", "projects": [project("p3", "/x")]}),
        )
        .expect_err("wrong host");
    assert_eq!(err.code, code::INVALID_PARAMS);
    assert_eq!(err.message, "daemon is devbox");
    client.call("session.close", json!({"id": id, "graceful": false}));
    client.call("session.close", json!({"id": again, "graceful": false}));
}

#[test]
fn sessions_announce_created_updated_and_removed() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut watcher = sandbox.client();
    let mut actor = sandbox.client();
    let id = sandbox.create(&mut actor, "exit 0");
    let created = watcher.wait_note("session.created");
    assert_eq!(created["id"], id.as_str());
    assert_eq!(created["status"], "running");
    assert!(created.get("tokenHash").is_none());

    actor.call("session.update", json!({"id": id, "title": "named"}));
    let updated = watcher.wait_note("session.updated");
    assert_eq!(
        (updated["id"].as_str(), updated["title"].as_str()),
        (Some(id.as_str()), Some("named"))
    );

    watcher.wait_note("session.exited");
    // `session.created` came first, even for a session that exits at once.
    let position = |name: &str| watcher.notes.iter().position(|n| n.0 == name);
    assert!(position("session.created") < position("session.exited"));
    actor.call("session.remove", json!({"id": id}));
    let removed = watcher.wait_note("session.removed");
    assert_eq!(removed, json!({"id": id}));
    assert_eq!(actor.call("session.list", Value::Null), json!([]));
    let err = actor
        .try_call("session.remove", json!({"id": id}))
        .expect_err("already gone");
    assert_eq!(err.code, code::SESSION_NOT_FOUND);
}

#[test]
fn only_an_exited_session_can_be_removed() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create(&mut client, "sleep 300");
    let err = client
        .try_call("session.remove", json!({"id": id}))
        .expect_err("running");
    assert_eq!(err.code, code::SESSION_STILL_RUNNING);
    assert_eq!(info_of(&mut client, &id)["status"], "running");
    client.call("session.close", json!({"id": id, "graceful": false}));
}

#[test]
fn the_lock_file_holds_the_daemon_pid() {
    let mut sandbox = Sandbox::new();
    let pid = sandbox.start_daemon();
    assert_eq!(sandbox.lock_pid(), Some(pid));
    // A refused second daemon must not overwrite it.
    let (status, _) = sandbox.daemon_that_exits();
    assert!(!status.success());
    assert_eq!(sandbox.lock_pid(), Some(pid));
}

#[test]
fn shutdown_stops_the_daemon_and_keeps_holders_and_sessions() {
    let mut sandbox = Sandbox::new();
    let daemon = sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create_shell(&mut client);
    let holder = sandbox.holder_pid(&id);
    client.call("daemon.shutdown", json!({}));
    wait_until("the daemon to stop", 10, || !alive(daemon));
    assert!(alive(holder), "the holder died with the daemon");
    assert!(!sandbox.socket().exists(), "the socket was left behind");
    // Nobody restarts it: the next client starts a daemon, which finds the session again.
    sleep(Duration::from_secs(1));
    assert!(!sandbox.socket().exists(), "the daemon restarted itself");
    sandbox.start_daemon();
    let mut client = sandbox.client();
    wait_until("the session to come back", 10, || {
        info_of(&mut client, &id)["status"] == "running"
    });
    client.call("session.close", json!({"id": id, "graceful": false}));
}

#[test]
fn a_daemon_that_is_stopping_refuses_changes() {
    let mut sandbox = Sandbox::new();
    let daemon = sandbox.start_daemon();
    let mut stopper = sandbox.client();
    let mut other = sandbox.client();
    let create =
        json!({"cmd": ["/bin/sh", "-c", "sleep 300"], "cwd": "/tmp", "cols": 80, "rows": 24});
    stopper.call("daemon.shutdown", Value::Null);
    // A request after the response always sees the stopping flag: refused.
    let refused = stopper
        .try_call("session.create", create.clone())
        .expect_err("a stopping daemon takes no new session");
    assert_eq!(refused.code, code::SHUTTING_DOWN);
    // From another connection, in the grace period: refused, or kept in the state file.
    let raced = other.try_call("session.create", create);
    wait_until("the daemon to stop", 10, || !alive(daemon));
    match raced {
        Err(e) => assert_eq!(e.code, code::SHUTTING_DOWN, "{e:?}"),
        Ok(created) => {
            let id = created["id"].as_str().expect("id");
            let state = fs::read_to_string(sandbox.home().join("state.json")).expect("state");
            assert!(state.contains(id), "an accepted session was lost: {state}");
        }
    }
}

#[test]
fn a_hook_connection_cannot_shut_the_daemon_down() {
    let mut sandbox = Sandbox::new();
    let daemon = sandbox.start_daemon();
    let mut hook = Client::connect(&sandbox.socket());
    hook.call(
        "hello",
        json!({"protocol": 1, "client": "hook", "role": "hook"}),
    );
    let err = hook
        .try_call("daemon.shutdown", json!({}))
        .expect_err("must be refused");
    assert_eq!(err.code, code::UNAUTHORIZED);
    sleep(Duration::from_millis(500));
    assert!(alive(daemon));
    assert!(sandbox.client().call("$/ping", Value::Null).is_object());
}

#[test]
fn an_existing_home_open_to_group_or_others_is_tightened() {
    // An installer may create the home under umask 022.
    for mode in [0o750, 0o755] {
        let mut sandbox = Sandbox::new();
        fs::set_permissions(sandbox.home(), fs::Permissions::from_mode(mode)).expect("chmod");
        fs::create_dir(sandbox.home().join("sessions")).expect("mkdir");
        fs::set_permissions(
            sandbox.home().join("sessions"),
            fs::Permissions::from_mode(mode),
        )
        .expect("chmod");
        sandbox.start_daemon();
        for dir in [
            sandbox.home().to_path_buf(),
            sandbox.home().join("sessions"),
        ] {
            let now = fs::metadata(&dir).expect("meta").permissions().mode();
            assert_eq!(now & 0o777, 0o700, "{} from {mode:o}", dir.display());
        }
    }
}

#[test]
fn the_socket_is_private_to_the_owner() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mode = fs::metadata(sandbox.socket())
        .expect("meta")
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "socket mode {mode:o}");
}

#[test]
fn the_socket_answers_while_a_holder_is_slow_to_reattach() {
    let mut sandbox = Sandbox::new();
    let first = sandbox.start_daemon();
    let mut client = sandbox.client();
    let live = sandbox.create_shell(&mut client);
    kill(first, "-KILL");
    wait_until("daemon to die", 5, || !alive(first));

    // A second session whose holder accepts the connection and never answers. It is listed
    // first, so a one-by-one recovery would wait out its 10 s timeout before the live one.
    let hung = "00hung";
    let sock = sandbox.home().join("sessions").join(format!("{hung}.sock"));
    let listener = UnixListener::bind(&sock).expect("bind fake holder");
    let silent = std::thread::spawn(move || {
        let held: Vec<_> = listener.incoming().take(1).flatten().collect();
        // It hangs up after a while: the daemon then gives that session up.
        sleep(Duration::from_secs(8));
        drop(held);
    });
    let state = sandbox.home().join("state.json");
    let mut doc: Value = serde_json::from_slice(&fs::read(&state).expect("state")).expect("json");
    doc["sessions"].as_array_mut().expect("sessions").insert(
        0,
        json!({
            "id": hung, "cmd": ["sh"], "cwd": "/tmp", "status": "running", "cols": 80, "rows": 24
        }),
    );
    fs::write(&state, serde_json::to_vec(&doc).expect("json")).expect("write state");

    let started = Instant::now();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "hello waited for recovery"
    );
    let status_of = |list: &Value, id: &str| {
        list.as_array()
            .expect("list")
            .iter()
            .find(|s| s["id"] == id)
            .map(|s| s["status"].clone())
    };
    // Both are listed at once, as detached, before any holder answered.
    let first = client.call("session.list", Value::Null);
    assert!(
        status_of(&first, hung).is_some() && status_of(&first, &live).is_some(),
        "{first}"
    );
    wait_until("the live session to come back", 5, || {
        status_of(&client.call("session.list", Value::Null), &live) == Some(json!("running"))
    });
    // The slow one stays listed as detached, and stays in the state file.
    assert_eq!(
        status_of(&client.call("session.list", Value::Null), hung),
        Some(json!("detached")),
        "the slow holder held the others up or vanished from the list"
    );
    let saved = fs::read_to_string(sandbox.home().join("state.json")).expect("state");
    assert!(
        saved.contains(hung),
        "a detached session fell out of the state file"
    );
    // Giving up on the slow one is announced as a settled status.
    client.pump_until("session.updated", 15, |c| {
        c.notes
            .iter()
            .any(|(m, p)| m == "session.updated" && p["id"] == hung && p["status"] == "exited")
    });
    client.call("session.close", json!({"id": live, "graceful": false}));
    drop(silent);
}

#[test]
fn responses_queued_before_the_clients_eof_are_still_delivered() {
    let mut sandbox = Sandbox::new();
    // Twenty finished sessions with long titles make each list answer about 6 KB.
    let sessions: Vec<Value> = (0..20)
        .map(|i| {
            json!({"id": format!("e{i:02}"), "cmd": ["sh"], "cwd": "/tmp", "title": "t".repeat(300),
                   "status": "exited", "exitCode": 0, "cols": 80, "rows": 24})
        })
        .collect();
    let state = json!({"schemaVersion": 1, "sessions": sessions});
    fs::write(sandbox.home().join("state.json"), state.to_string()).expect("state");
    sandbox.start_daemon();

    let mut client = Client::connect(&sandbox.socket());
    let requests = 100;
    let hello = Request::new(1, "hello", json!({"protocol": 1, "client": "eof-test"}));
    client
        .stream
        .write_all(&encode(&hello.frame()))
        .expect("write");
    for id in 2..2 + requests {
        let list = Request::new(id, "session.list", Value::Null);
        client
            .stream
            .write_all(&encode(&list.frame()))
            .expect("write");
    }
    // The client stops writing but keeps reading: the daemon sees EOF while its answers are
    // still queued behind a full socket.
    client
        .stream
        .shutdown(std::net::Shutdown::Write)
        .expect("half-close");
    sleep(Duration::from_millis(500));
    let mut decoder = Decoder::new();
    let mut answered = 0;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut buf = [0u8; 64 * 1024];
    loop {
        match client.stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                for frame in decoder.push(&buf[..n]).expect("decode") {
                    if let Frame::Json(text) = frame {
                        if matches!(Message::parse(&text), Ok(Message::Response(_))) {
                            answered += 1;
                        }
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => panic!("read failed: {e}"),
        }
        assert!(
            Instant::now() < deadline,
            "the daemon never closed; {answered} answers"
        );
    }
    assert_eq!(answered, 1 + requests, "answers were dropped at EOF");
}

fn daemon_with_log(sandbox: &mut Sandbox, level: Option<&str>) -> std::path::PathBuf {
    let log = sandbox.home().join("daemon.log");
    let mut command = Command::new(BIN);
    command
        .arg("daemon")
        .env("SUSHIAI_HOME", sandbox.home())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&log).expect("log"));
    if let Some(level) = level {
        command.env("SUSHIAI_LOG", level);
    }
    sandbox.daemons.push(command.spawn().expect("spawn"));
    let socket = sandbox.socket();
    wait_until("daemon socket", 15, || {
        std::os::unix::net::UnixStream::connect(&socket).is_ok()
    });
    log
}

#[test]
fn the_daemon_logs_at_warn_by_default() {
    let mut quiet = Sandbox::new();
    let log = daemon_with_log(&mut quiet, None);
    Client::connect(&quiet.socket()).call("hello", json!({"protocol": 1, "client": "test"}));
    sleep(Duration::from_millis(300));
    let text = fs::read_to_string(&log).expect("log");
    assert!(
        !text.contains("daemon listening"),
        "info reached the default log: {text}"
    );

    let mut loud = Sandbox::new();
    let log = daemon_with_log(&mut loud, Some("info"));
    wait_until("the listening line", 5, || {
        fs::read_to_string(&log).is_ok_and(|t| t.contains("daemon listening"))
    });
}

fn holder_signal(pid: u32, signal: &str) {
    kill(pid, signal);
}

/// Kills the daemon, freezes the session's holder so its reattach hangs, starts a new daemon,
/// and thaws the holder after `thaw`. The new daemon's socket is answering meanwhile.
fn restart_with_a_slow_holder(sandbox: &mut Sandbox, daemon: u32, holder: u32, thaw: Duration) {
    kill(daemon, "-KILL");
    wait_until("daemon to die", 5, || !alive(daemon));
    holder_signal(holder, "-STOP");
    sandbox.start_daemon();
    std::thread::spawn(move || {
        sleep(thaw);
        holder_signal(holder, "-CONT");
    });
}

#[test]
fn requests_for_a_restored_session_wait_for_its_holder() {
    let mut sandbox = Sandbox::new();
    let first = sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create_shell(&mut client);
    print_marker(&mut client, &id, 5);
    wait_snapshot_contains(&mut client, &id, "marker-5");
    let holder = sandbox.holder_pid(&id);

    // attach, sent while the holder is frozen: it succeeds once the holder answers.
    restart_with_a_slow_holder(&mut sandbox, first, holder, Duration::from_millis(1500));
    let mut client = sandbox.client();
    assert_eq!(info_of(&mut client, &id)["status"], "detached");
    let started = Instant::now();
    assert!(client.attach(&id).contains("marker-5"));
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "attach did not wait"
    );
    assert_eq!(info_of(&mut client, &id)["status"], "running");

    // input and resize wait too.
    let second = sandbox.lock_pid().expect("pid");
    restart_with_a_slow_holder(&mut sandbox, second, holder, Duration::from_millis(1500));
    let mut client = sandbox.client();
    print_marker(&mut client, &id, 6);
    client.call("session.resize", json!({"id": id, "cols": 100, "rows": 30}));
    wait_snapshot_contains(&mut client, &id, "marker-6");

    // A close sent during recovery is not lost: the session ends.
    let third = sandbox.lock_pid().expect("pid");
    restart_with_a_slow_holder(&mut sandbox, third, holder, Duration::from_millis(1500));
    let mut client = sandbox.client();
    client.call("session.close", json!({"id": id, "graceful": false}));
    wait_until("the session to end", 10, || {
        info_of(&mut client, &id)["status"] == "exited"
    });
}

#[test]
fn a_request_for_a_session_that_never_comes_back_gets_the_normal_error() {
    let mut sandbox = Sandbox::new();
    let first = sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create_shell(&mut client);
    let holder = sandbox.holder_pid(&id);
    kill(first, "-KILL");
    wait_until("daemon to die", 5, || !alive(first));
    // The holder dies while no daemon runs: the restored session ends as exited.
    kill(holder, "-KILL");
    wait_until("holder to die", 5, || !alive(holder));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let err = client
        .try_call("session.input", json!({"id": id, "data": "x"}))
        .expect_err("not running");
    assert_eq!(err.code, code::SESSION_NOT_RUNNING);
}

#[test]
fn a_keyed_launch_is_announced_once_and_refused_while_stopping() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut watcher = sandbox.client();
    let mut client = sandbox.client();
    let create = json!({"cmd": ["/bin/sh", "-c", "sleep 300"], "cwd": "/tmp", "cols": 80,
                        "rows": 24, "idempotencyKey": "k1"});
    let first = client.call("session.create", create.clone());
    let again = client.call("session.create", create.clone());
    assert_eq!(first["id"], again["id"]);
    watcher.wait_note("session.created");
    // A later notification marks the end of what the watcher will hear.
    client.call("session.update", json!({"id": first["id"], "title": "t"}));
    watcher.wait_note("session.updated");
    let created = watcher
        .notes
        .iter()
        .filter(|n| n.0 == "session.created")
        .count();
    assert_eq!(created, 1, "the idempotent return was announced again");

    client.call("daemon.shutdown", Value::Null);
    let keyed = json!({"cmd": ["/bin/sh", "-c", "sleep 300"], "cwd": "/tmp", "cols": 80,
                       "rows": 24, "idempotencyKey": "k2"});
    let err = client
        .try_call("session.create", keyed)
        .expect_err("a stopping daemon takes no keyed launch");
    assert_eq!(err.code, code::SHUTTING_DOWN);
}

#[test]
fn session_read_waits_for_a_restored_session() {
    let mut sandbox = Sandbox::new();
    let first = sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create_shell(&mut client);
    print_marker(&mut client, &id, 9);
    wait_snapshot_contains(&mut client, &id, "marker-9");
    let holder = sandbox.holder_pid(&id);
    restart_with_a_slow_holder(&mut sandbox, first, holder, Duration::from_millis(1500));
    let mut client = sandbox.client();
    let read = client.call("session.read", json!({"id": id}));
    assert!(read["text"].as_str().expect("text").contains("marker-9"));
}
