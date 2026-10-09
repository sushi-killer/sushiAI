//! Session hibernation and wake end to end: a real daemon and holders in a temp home, with fake
//! `claude` and `codex` node scripts on `PATH`. They send their hooks through `sushiai hook` like
//! the real CLIs (see `agents.rs`), log their argv, variables and keystrokes to a file, and stay
//! alive until they are told to end.

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use common::rig::*;
use common::*;
use serde_json::{json, Value};

fn mode(path: &Path) -> u32 {
    fs::metadata(path).expect("metadata").mode() & 0o777
}

fn launch_file(sandbox: &Sandbox, id: &str) -> PathBuf {
    sandbox.home().join("sessions").join(format!("{id}.launch"))
}

fn tail_file(sandbox: &Sandbox, id: &str) -> PathBuf {
    sandbox.home().join("sessions").join(format!("{id}.tail"))
}

#[test]
fn an_idle_agent_sleeps_keeps_its_screen_and_wakes_on_typing_in_order() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(Some(1500));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let hello = client.call("hello", json!({"protocol": 1, "client": "t"}));
    assert!(hello["capabilities"]
        .as_array()
        .is_some_and(|c| c.iter().any(|c| c == "hibernate")));
    let id = rig.create(&mut client);
    wait_idle(&mut client, &id);
    let session = entry(&mut client, &id)["agentSession"].clone();

    wait_status(&mut client, &id, "hibernated");
    let asleep = entry(&mut client, &id);
    assert!(asleep["hibernatedAt"].as_u64().is_some());
    assert!(asleep.get("incarnation").is_none(), "first process is 0");
    assert!(asleep.get("tokenHash").is_none());
    wait_until("the holder to end", 10, || sandbox.holders().is_empty());
    // The screen it had is served without waking it.
    let text = client.call("session.read", json!({"id": id}))["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(text.contains("tail-marker-claude"), "{text:?}");
    assert!(client.attach(&id).contains("tail-marker-claude"));
    assert_eq!(status_of(&mut client, &id), "hibernated", "attach woke it");
    assert_eq!(mode(&tail_file(&sandbox, &id)), 0o600);

    // Keep it awake from here on, so the checks below do not race a second sleep.
    client.call("session.update", json!({"id": id, "pinned": true}));
    client.type_text(&id, "abc");
    client.type_text(&id, "def");
    wait_until("the typed text to arrive", 20, || rig.typed() == "abcdef");
    wait_status(&mut client, &id, "running");
    let awake = entry(&mut client, &id);
    assert_eq!(awake["id"], id.as_str());
    assert_eq!(awake["incarnation"], 1);
    assert_eq!(awake["pinned"], true);
    assert!(awake.get("hibernatedAt").is_none());
    let argvs = rig.argvs();
    assert_eq!(argvs.len(), 2, "{argvs:?}");
    assert!(argvs[0].contains("--session-id"), "{}", argvs[0]);
    let resume = format!("\"--resume\",{session}");
    assert!(argvs[1].contains(&resume), "{} lacks {resume}", argvs[1]);
    assert_eq!(
        rig.log().matches(&format!("env {SECRET}")).count(),
        2,
        "the launch variables came back with the wake"
    );
    // The stream carries on in one piece: the woken process starts with a terminal reset.
    client.type_text(&id, "!");
    wait_until("more input", 10, || rig.typed() == "abcdef!");
}

#[test]
fn a_woken_stream_continues_the_sequence_and_starts_with_a_reset() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(Some(1500));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_status(&mut client, &id, "hibernated");
    let attached = client.attach_result(&id);
    let floor = attached["seq"].as_u64().expect("seq");
    client.call("session.update", json!({"id": id, "pinned": true}));
    client.call("session.wake", json!({"id": id}));
    client.pump_until("the reset", 20, |c| {
        c.output.iter().any(|(_, d)| d.starts_with(b"\x1bc"))
    });
    client.wait_streamed("tail-marker-claude");
    let (seq, _) = client
        .output
        .iter()
        .find(|(_, d)| d.starts_with(b"\x1bc"))
        .cloned()
        .expect("reset chunk");
    assert_eq!(seq, floor, "the stream carries on from the snapshot");
    let marker = client
        .output
        .iter()
        .find(|(_, d)| String::from_utf8_lossy(d).contains("tail-marker-claude"))
        .map(|(s, _)| *s)
        .expect("marker");
    assert!(
        marker > floor,
        "output of the new process comes after the reset"
    );
    wait_status(&mut client, &id, "running");
    assert_eq!(entry(&mut client, &id)["incarnation"], 1);
}

#[test]
fn pinned_and_focused_sessions_and_shells_stay_awake() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(Some(1500));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let shell = sandbox.create_shell(&mut client);
    let pinned = rig.create(&mut client);
    client.call("session.update", json!({"id": pinned, "pinned": true}));
    let focused = rig.create(&mut client);
    client.call("session.focus", json!({"id": focused, "focused": true}));
    let gone = rig.create(&mut client);
    let control = rig.create(&mut client);
    // A client that looks at a session and then goes away.
    let mut viewer = sandbox.client();
    viewer.call("session.focus", json!({"id": gone, "focused": true}));

    wait_status(&mut client, &control, "hibernated");
    std::thread::sleep(Duration::from_millis(2500));
    for id in [&pinned, &focused, &gone, &shell] {
        assert_eq!(status_of(&mut client, id), "running", "{id} went to sleep");
    }
    // The viewer leaves: its focus goes with it.
    drop(viewer);
    wait_status(&mut client, &gone, "hibernated");
    assert_eq!(status_of(&mut client, &focused), "running");
    client.call("session.focus", json!({"id": focused, "focused": false}));
    wait_status(&mut client, &focused, "hibernated");
    assert_eq!(status_of(&mut client, &pinned), "running");
    assert_eq!(status_of(&mut client, &shell), "running");
    let refused = client
        .try_call("session.hibernate", json!({"id": shell}))
        .expect_err("a shell never sleeps");
    assert_eq!(refused.code, -32602);
}

#[test]
fn a_session_whose_holder_died_with_the_daemon_sleeps_and_wakes() {
    let rig = Rig::new("claude");
    // The default threshold: only the restart puts it to sleep.
    let mut sandbox = rig.sandbox(None);
    let daemon = sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_idle(&mut client, &id);
    let holder = sandbox.holder_pid(&id);
    let child = child_of(holder);
    // A reboot: everything dies at once.
    kill(daemon, "-KILL");
    kill(holder, "-KILL");
    kill(child, "-KILL");
    wait_until("the processes to end", 10, || {
        !alive(holder) && !alive(daemon)
    });
    sandbox.start_daemon();
    let mut client = sandbox.client();
    wait_status(&mut client, &id, "hibernated");
    let asleep = entry(&mut client, &id);
    assert!(asleep["agentSession"].is_string());
    let text = client.call("session.read", json!({"id": id}))["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        text.contains("tail-marker-claude"),
        "no tail after a crash: {text:?}"
    );

    client.type_text(&id, "xyz");
    wait_until("the typed text to arrive", 20, || rig.typed() == "xyz");
    wait_status(&mut client, &id, "running");
    assert_eq!(entry(&mut client, &id)["incarnation"], 1);
    let argvs = rig.argvs();
    assert!(argvs[1].contains("--resume"), "{argvs:?}");
}

#[test]
fn stopping_the_daemon_saves_the_tail_of_a_running_agent() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(None);
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_idle(&mut client, &id);
    // The tail written at the change to idle is gone; only the stop can bring it back.
    wait_until("the idle tail", 10, || tail_file(&sandbox, &id).exists());
    fs::remove_file(tail_file(&sandbox, &id)).expect("remove");
    client.call("daemon.shutdown", Value::Null);
    wait_until("the tail saved on stop", 10, || {
        fs::read(tail_file(&sandbox, &id))
            .is_ok_and(|t| String::from_utf8_lossy(&t).contains("tail-marker-claude"))
    });
    assert_eq!(mode(&tail_file(&sandbox, &id)), 0o600);
}

#[test]
fn a_codex_session_wakes_with_the_resume_subcommand() {
    let rig = Rig::new("codex");
    rig.install_codex_hooks();
    let mut sandbox = rig.sandbox(Some(1500));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_status(&mut client, &id, "hibernated");
    let session = entry(&mut client, &id)["agentSession"].clone();
    assert!(
        session.is_string(),
        "codex learned its conversation from a hook"
    );
    client.call("session.update", json!({"id": id, "pinned": true}));
    client.call("session.wake", json!({"id": id}));
    wait_status(&mut client, &id, "running");
    let argvs = rig.argvs();
    assert_eq!(argvs.len(), 2, "{argvs:?}");
    let resume = format!("\"resume\",{session}");
    assert!(argvs[1].contains(&resume), "{} lacks {resume}", argvs[1]);
}

#[test]
fn the_launch_is_sealed_and_a_wake_without_it_is_refused() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(None);
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_idle(&mut client, &id);
    let file = launch_file(&sandbox, &id);
    let bytes = fs::read(&file).expect("launch file");
    assert!(
        !bytes.windows(SECRET.len()).any(|w| w == SECRET.as_bytes()),
        "the launch holds its variables in plain text"
    );
    assert_eq!(mode(&file), 0o600);
    assert!(
        !String::from_utf8_lossy(&fs::read(sandbox.home().join("state.json")).expect("state"))
            .contains(SECRET)
    );

    client.call("session.hibernate", json!({"id": id}));
    wait_status(&mut client, &id, "hibernated");
    // A file that does not open means: no stored launch, and never one again. The record
    // ends as exited, so no later wake can start a second agent from it.
    fs::write(&file, b"SLK1garbage-garbage-garbage").expect("corrupt");
    let broken = client
        .try_call("session.wake", json!({"id": id}))
        .expect_err("a damaged launch cannot wake");
    assert_eq!(broken.code, 1012);
    assert_eq!(status_of(&mut client, &id), "exited");
    assert!(!file.exists() && !tail_file(&sandbox, &id).exists());
    let again = client
        .try_call("session.wake", json!({"id": id}))
        .expect_err("no launch, no wake");
    assert_eq!(again.code, 1012);
    let unknown = client
        .try_call("session.wake", json!({"id": "nope"}))
        .expect_err("unknown");
    assert_eq!(unknown.code, 1003);
}

#[test]
fn a_key_that_cannot_be_read_keeps_the_record_asleep_with_error_1013() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(None);
    let daemon = sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_idle(&mut client, &id);
    client.call("session.hibernate", json!({"id": id}));
    wait_status(&mut client, &id, "hibernated");
    client.call("daemon.shutdown", Value::Null);
    wait_until("the daemon to stop", 10, || !alive(daemon));
    // A new daemon has not read the key yet; the key file is now unreadable (a directory).
    let key = rig.dir.path().join("launch.key");
    let saved = fs::read(&key).expect("key");
    fs::remove_file(&key).expect("remove key");
    fs::create_dir(&key).expect("key dir");
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let unavailable = client
        .try_call("session.wake", json!({"id": id}))
        .expect_err("no key, no wake");
    assert_eq!(unavailable.code, 1013);
    assert_eq!(status_of(&mut client, &id), "hibernated");
    assert!(launch_file(&sandbox, &id).exists(), "the launch is kept");
    // The key is back (the keychain unlocked): the same record wakes.
    fs::remove_dir(&key).expect("remove dir");
    fs::write(&key, saved).expect("restore key");
    client.call("session.wake", json!({"id": id}));
    wait_status(&mut client, &id, "running");
}

#[test]
fn a_late_wake_cannot_revive_a_session_the_owner_closed() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(None);
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_idle(&mut client, &id);
    client.call("session.hibernate", json!({"id": id}));
    wait_status(&mut client, &id, "hibernated");
    client.call("session.close", json!({"id": id, "graceful": true}));
    assert_eq!(status_of(&mut client, &id), "exited");
    let late = client
        .try_call("session.wake", json!({"id": id}))
        .expect_err("closed");
    assert_eq!(late.code, 1012);
    assert_eq!(status_of(&mut client, &id), "exited");
    assert_eq!(rig.argvs().len(), 1, "no second agent started");
}

#[test]
fn closing_a_session_deletes_its_launch_and_a_clean_exit_keeps_it_for_reopen() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(None);
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let closed = rig.create(&mut client);
    wait_idle(&mut client, &closed);
    assert!(launch_file(&sandbox, &closed).exists());
    client.call("session.close", json!({"id": closed, "graceful": true}));
    wait_status(&mut client, &closed, "exited");
    wait_until("the launch to go", 10, || {
        !launch_file(&sandbox, &closed).exists()
    });
    let refused = client
        .try_call("session.wake", json!({"id": closed}))
        .expect_err("a closed session has no launch");
    assert_eq!(refused.code, 1012);

    // An agent that ended by itself can be woken in place: Reopen on the same id.
    let ended = rig.create(&mut client);
    wait_idle(&mut client, &ended);
    let holder = sandbox.holder_pid(&ended);
    kill(child_of(holder), "-KILL");
    wait_status(&mut client, &ended, "exited");
    assert!(launch_file(&sandbox, &ended).exists());
    client.call("session.wake", json!({"id": ended}));
    wait_status(&mut client, &ended, "running");
    assert_eq!(entry(&mut client, &ended)["incarnation"], 1);
    // Removing the record removes its launch.
    client.call("session.close", json!({"id": ended, "graceful": false}));
    wait_status(&mut client, &ended, "exited");
    client.call("session.remove", json!({"id": ended}));
    assert!(!launch_file(&sandbox, &ended).exists());
}

#[test]
fn configure_saves_the_setting_and_a_state_file_of_schema_one_loads() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(None);
    let old = json!({"schemaVersion": 1, "sessions": [{
        "id": "old1", "cmd": ["/bin/sh"], "cwd": "/tmp", "title": "kept",
        "status": "exited", "exitCode": 0, "cols": 80, "rows": 24
    }]});
    fs::write(
        sandbox.home().join("state.json"),
        serde_json::to_vec(&old).expect("json"),
    )
    .expect("write state");
    sandbox.start_daemon();
    let mut client = sandbox.client();
    assert_eq!(entry(&mut client, "old1")["title"], "kept");
    // 1 s of idle sleeps an agent; the setting reaches the file with mode 0600.
    client.call("daemon.configure", json!({"hibernateAfterSecs": 1}));
    let settings = sandbox.home().join("settings.json");
    assert_eq!(mode(&settings), 0o600);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&settings).expect("settings")).expect("json")
            ["hibernateAfterSecs"],
        1
    );
    let id = rig.create(&mut client);
    wait_status(&mut client, &id, "hibernated");
    wait_until("the state file to be schema 2", 10, || {
        fs::read_to_string(sandbox.home().join("state.json"))
            .is_ok_and(|t| t.contains("\"schemaVersion\": 2") && t.contains("hibernated"))
    });
    // Off means never: a woken session stays up.
    client.call("daemon.configure", json!({"hibernateAfterSecs": 0}));
    client.call("session.wake", json!({"id": id}));
    wait_status(&mut client, &id, "running");
    std::thread::sleep(Duration::from_millis(3000));
    assert_eq!(status_of(&mut client, &id), "running");
}

#[test]
fn resizing_a_sleeping_session_succeeds_and_the_wake_uses_the_new_size() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(Some(1500));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_idle(&mut client, &id);
    wait_status(&mut client, &id, "hibernated");

    client.call("session.resize", json!({"id": id, "cols": 132, "rows": 41}));
    let asleep = entry(&mut client, &id);
    assert_eq!(asleep["status"], "hibernated", "a resize must not wake it");
    assert_eq!(
        (asleep["cols"].as_u64(), asleep["rows"].as_u64()),
        (Some(132), Some(41))
    );

    client.call("session.update", json!({"id": id, "pinned": true}));
    client.type_text(&id, "x");
    wait_status(&mut client, &id, "running");
    wait_until("the woken process to report its size", 20, || {
        rig.log().contains("size 132x41")
    });
}

#[test]
fn input_typed_while_the_agent_ends_for_a_sleep_arrives_after_the_wake_in_order() {
    let rig = Rig::new("claude");
    // The default threshold: only `session.hibernate` puts it to sleep. The fake needs 2.5 s
    // to end after TERM, which is the grace the input used to be lost in.
    let mut sandbox = rig.sandbox(None);
    sandbox
        .env
        .push(("FAKE_TERM_DELAY_MS".into(), "2500".into()));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_idle(&mut client, &id);
    client.call("session.hibernate", json!({"id": id}));
    assert_eq!(status_of(&mut client, &id), "running", "still ending");
    client.type_text(&id, "abc");
    client.type_text(&id, "def");
    wait_until("the wake after the sleep", 30, || {
        entry(&mut client, &id)["incarnation"] == 1
    });
    wait_until("the typed text to arrive", 20, || rig.typed() == "abcdef");
    wait_status(&mut client, &id, "running");
}

#[test]
fn a_wake_asked_while_the_agent_ends_for_a_sleep_wakes_it_once_asleep() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(None);
    sandbox
        .env
        .push(("FAKE_TERM_DELAY_MS".into(), "2000".into()));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_idle(&mut client, &id);
    client.call("session.hibernate", json!({"id": id}));
    client.call("session.wake", json!({"id": id}));
    wait_until("the wake after the sleep", 30, || {
        entry(&mut client, &id)["incarnation"] == 1
    });
    wait_status(&mut client, &id, "running");
    assert_eq!(rig.argvs().len(), 2);
}

#[test]
fn a_woken_codex_that_sends_no_hook_is_idle_so_it_can_sleep_again() {
    let rig = Rig::new("codex");
    rig.install_codex_hooks();
    let mut sandbox = rig.sandbox(None);
    sandbox.env.push(("FAKE_QUIET_RESUME".into(), "1".into()));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_idle(&mut client, &id);
    client.call("session.hibernate", json!({"id": id}));
    wait_status(&mut client, &id, "hibernated");
    client.call("session.wake", json!({"id": id}));
    wait_status(&mut client, &id, "running");
    wait_until("idle after the wake", 10, || {
        entry(&mut client, &id)["agentStatus"] == "idle"
    });
    // And idle means it sleeps again once the threshold passes.
    client.call("daemon.configure", json!({"hibernateAfterSecs": 1}));
    wait_status(&mut client, &id, "hibernated");
}

#[test]
fn a_hibernated_record_without_a_launch_at_start_is_exited() {
    let rig = Rig::new("claude");
    let mut sandbox = rig.sandbox(None);
    let daemon = sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = rig.create(&mut client);
    wait_idle(&mut client, &id);
    client.call("session.hibernate", json!({"id": id}));
    wait_status(&mut client, &id, "hibernated");
    client.call("daemon.shutdown", Value::Null);
    wait_until("the daemon to stop", 10, || !alive(daemon));
    fs::remove_file(launch_file(&sandbox, &id)).expect("remove launch");
    sandbox.start_daemon();
    let mut client = sandbox.client();
    assert_eq!(status_of(&mut client, &id), "exited");
}
