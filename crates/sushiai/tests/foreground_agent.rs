//! An agent started by hand in a shell session is detected from the PTY's foreground process.
//! The fake `claude` is a node script (a real one runs the same way): it writes the session
//! file Claude keeps under `$CLAUDE_CONFIG_DIR/sessions/<pid>.json` and exits on any input.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;

use common::*;
use serde_json::{json, Value};

const SESSION: &str = "00000000-0000-4000-8000-000000000001";

const FAKE: &str = r#"#!/usr/bin/env node
const fs = require('fs');
const path = require('path');
const dir = path.join(process.env.CLAUDE_CONFIG_DIR, 'sessions');
fs.mkdirSync(dir, { recursive: true });
fs.writeFileSync(path.join(dir, process.pid + '.json'),
  JSON.stringify({ pid: process.pid, sessionId: '@SESSION@', cwd: '/tmp' }));
fs.writeFileSync(path.join(process.env.FAKE_OUT, 'argv'), process.argv.slice(2).join(' '));
console.log('fake agent up');
process.stdin.on('data', () => process.exit(0));
setTimeout(() => {}, 60000);
"#;

const CODEX_SESSION: &str = "00000000-0000-4000-8000-000000000002";

/// An in-process Codex holds its rollout file open for the whole run.
const FAKE_CODEX: &str = r#"#!/usr/bin/env node
const fs = require('fs');
const path = require('path');
const dir = path.join(process.env.CODEX_HOME, 'sessions', '2026', '01', '01');
fs.mkdirSync(dir, { recursive: true });
const id = '@SESSION@';
const file = path.join(dir, 'rollout-2026-01-01T00-00-00-' + id + '.jsonl');
fs.writeFileSync(file, JSON.stringify({ type: 'session_meta', payload: { id, cwd: '/tmp' } }) + '\n');
const held = fs.openSync(file, 'r');
console.log('fake codex up');
process.stdin.on('data', () => process.exit(0));
setTimeout(() => fs.closeSync(held), 60000);
"#;

struct World {
    dir: tempfile::TempDir,
    sandbox: Sandbox,
}

fn world() -> World {
    let dir = tempfile::Builder::new()
        .prefix("fg")
        .tempdir_in("/tmp")
        .expect("tempdir");
    for sub in ["bin", "claude", "out"] {
        fs::create_dir(dir.path().join(sub)).expect("mkdir");
    }
    let bin = dir.path().join("bin/claude");
    fs::write(&bin, FAKE.replace("@SESSION@", SESSION)).expect("write");
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
    let codex = dir.path().join("bin/codex");
    fs::write(&codex, FAKE_CODEX.replace("@SESSION@", CODEX_SESSION)).expect("write");
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).expect("chmod");
    fs::create_dir(dir.path().join("codex")).expect("mkdir");
    let mut sandbox = Sandbox::new();
    sandbox.env.push((
        "CODEX_HOME".into(),
        dir.path().join("codex").display().to_string(),
    ));
    let path = std::env::var("PATH").unwrap_or_default();
    sandbox.env.push((
        "PATH".into(),
        format!("{}:{path}", dir.path().join("bin").display()),
    ));
    sandbox.env.push((
        "CLAUDE_CONFIG_DIR".into(),
        dir.path().join("claude").display().to_string(),
    ));
    sandbox.env.push((
        "FAKE_OUT".into(),
        dir.path().join("out").display().to_string(),
    ));
    World { dir, sandbox }
}

fn meta_with(client: &Client, id: &str, foreground: Value) -> Option<Value> {
    client
        .notes
        .iter()
        .rev()
        .find(|(name, p)| {
            name == "session.meta" && p["id"] == id && p["foregroundAgent"] == foreground
        })
        .map(|(_, p)| p.clone())
}

#[test]
fn a_hand_started_claude_is_reported_with_its_session_and_cleared_on_exit() {
    let mut w = world();
    w.sandbox.start_daemon();
    let mut client = w.sandbox.client();
    let id = w.sandbox.create_shell(&mut client);
    client.attach(&id);
    client.type_text(&id, "claude\n");
    client.wait_streamed("fake agent up");

    client.pump_until("claude detected", 6, |c| {
        meta_with(c, &id, json!("claude")).is_some()
    });
    let meta = meta_with(&client, &id, json!("claude")).expect("meta");
    // The session file may land just after the first detection.
    client.pump_until("claude session id", 6, |c| {
        meta_with(c, &id, json!("claude")).is_some_and(|m| m["agentSession"] == SESSION)
    });
    assert!(meta["transcriptPath"].is_null());
    let listed = client.call("session.list", Value::Null);
    let entry = listed
        .as_array()
        .and_then(|l| l.iter().find(|s| s["id"] == id.as_str()))
        .expect("listed");
    assert_eq!(entry["foregroundAgent"], "claude");
    assert_eq!(entry["agentSession"], SESSION);
    assert!(
        entry["foregroundCwd"]
            .as_str()
            .is_some_and(|cwd| cwd.ends_with("/tmp")),
        "the agent's folder is reported: {entry}"
    );
    assert!(
        entry.get("agent").is_none(),
        "detection is not a launch agent"
    );
    assert!(entry.get("agentStatus").is_none(), "no status machine");

    client.type_text(&id, "\n");
    client.pump_until("claude cleared", 6, |c| {
        meta_with(c, &id, Value::Null).is_some_and(|m| m["agentSession"].is_null())
    });
    let listed = client.call("session.list", Value::Null);
    let entry = listed
        .as_array()
        .and_then(|l| l.iter().find(|s| s["id"] == id.as_str()))
        .expect("listed");
    assert!(entry.get("foregroundAgent").is_none());
    assert!(entry.get("agentSession").is_none());
    drop(w.dir);
}

#[test]
fn a_shell_that_exits_by_itself_right_after_its_agent_leaves_no_agent_to_reopen() {
    let mut w = world();
    w.sandbox.start_daemon();
    let mut client = w.sandbox.client();
    let id = w.sandbox.create_shell(&mut client);
    client.attach(&id);
    client.type_text(&id, "claude\n");
    client.wait_streamed("fake agent up");
    client.pump_until("claude detected", 6, |c| {
        meta_with(c, &id, json!("claude")).is_some()
    });
    // The agent quits and the shell is left before the next poll could notice.
    client.type_text(&id, "\n");
    std::thread::sleep(std::time::Duration::from_millis(300));
    client.type_text(&id, "exit\n");
    client.wait_note("session.exited");
    let listed = client.call("session.list", Value::Null);
    let entry = listed
        .as_array()
        .and_then(|l| l.iter().find(|s| s["id"] == id.as_str()))
        .expect("listed");
    assert_eq!(entry["status"], "exited");
    for key in ["foregroundAgent", "foregroundCwd", "agentSession"] {
        assert!(entry.get(key).is_none(), "{key} survived: {entry}");
    }
    assert!(meta_with(&client, &id, Value::Null).is_some());
}

#[test]
fn a_hand_started_codex_reports_the_rollout_it_holds_open_and_none_after_it_quits() {
    let mut w = world();
    w.sandbox.start_daemon();
    let mut client = w.sandbox.client();
    let id = w.sandbox.create_shell(&mut client);
    client.attach(&id);
    client.type_text(&id, "codex\n");
    client.wait_streamed("fake codex up");
    client.pump_until("codex and its conversation", 8, |c| {
        meta_with(c, &id, json!("codex")).is_some_and(|m| m["agentSession"] == CODEX_SESSION)
    });
    client.type_text(&id, "\n");
    client.pump_until("codex cleared", 8, |c| {
        meta_with(c, &id, Value::Null).is_some_and(|m| m["agentSession"].is_null())
    });
}
