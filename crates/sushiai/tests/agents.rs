//! Agent sessions end to end. The fake `claude` and `codex` are node scripts that run the hook
//! commands they find where the real CLIs would: in the `--settings` argument (claude) and in
//! `$CODEX_HOME/hooks.json` written by `sushiai hooks install` (codex). Fixtures supply the
//! payloads. A real daemon and holder run them; no real CLI is involved.

mod common;

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::*;
use serde_json::{json, Value};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../sushiai-agents/tests/fixtures"
);

const PRELUDE: &str = r#"#!/usr/bin/env node
const { spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const KIND = '@KIND@', BIN = '@BIN@', F = '@F@', OUT = '@OUT@';
function commandFor(event) {
  if (KIND === 'claude') {
    const s = JSON.parse(process.argv[process.argv.indexOf('--settings') + 1]);
    return s.hooks[event][0].hooks[0].command;
  }
  const h = JSON.parse(fs.readFileSync(path.join(process.env.CODEX_HOME, 'hooks.json'), 'utf8'));
  for (const g of h.hooks[event]) for (const x of g.hooks) if (x.command.includes('sushiai')) return x.command;
  throw new Error('no hook for ' + event);
}
function run(event, file, out) {
  const r = spawnSync('sh', ['-c', commandFor(event)], { input: fs.readFileSync(path.join(F, file)), encoding: 'utf8' });
  if (out) fs.writeFileSync(path.join(OUT, out), r.stdout);
  return r;
}
function say(name, text) { fs.writeFileSync(path.join(OUT, name), (text || 'x') + '\n'); }
function stay() { setTimeout(() => {}, 60000); }
"#;

/// A temp tree holding a fake agent on `PATH` and the fixtures it replays.
struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    /// `body` is node code run as the agent `name` (`claude` or `codex`). It can call
    /// `run(event, fixtureFile, outName)`, `say(name, text)` and `stay()`.
    fn new(name: &str, body: &str) -> Fake {
        let dir = tempfile::Builder::new()
            .prefix("fk")
            .tempdir_in("/tmp")
            .expect("tempdir");
        for sub in ["bin", "fx", "out"] {
            fs::create_dir(dir.path().join(sub)).expect("mkdir");
        }
        for entry in fs::read_dir(Path::new(FIXTURES).join(name)).expect("fixtures") {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|e| e == "json") {
                let target = dir.path().join("fx").join(path.file_name().expect("name"));
                fs::copy(&path, target).expect("copy");
            }
        }
        let script = PRELUDE
            .replace("@KIND@", name)
            .replace("@BIN@", BIN)
            .replace("@F@", &dir.path().join("fx").display().to_string())
            .replace("@OUT@", &dir.path().join("out").display().to_string())
            + body;
        let bin = dir.path().join("bin").join(name);
        fs::write(&bin, script).expect("write script");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
        Fake { dir }
    }

    fn out(&self, name: &str) -> PathBuf {
        self.dir.path().join("out").join(name)
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.out(name)).unwrap_or_default()
    }

    /// Makes the captured Claude fixtures one consistent turn (they were recorded in
    /// separate runs): one session id, one prompt id, and the ask is for the tool call.
    fn unify_claude(&self) {
        let fx = self.dir.path().join("fx");
        let load = |n: &str| -> Value {
            serde_json::from_slice(&fs::read(fx.join(n)).expect("read")).expect("json")
        };
        let prompt = load("UserPromptSubmit.json");
        let tool_input = load("PreToolUse-bash.json")["tool_input"].clone();
        for n in fs::read_dir(&fx).expect("dir").flatten() {
            let mut v: Value =
                serde_json::from_slice(&fs::read(n.path()).expect("read")).expect("json");
            v["session_id"] = prompt["session_id"].clone();
            v["prompt_id"] = prompt["prompt_id"].clone();
            if v["hook_event_name"] == "PermissionRequest" {
                v["tool_input"] = tool_input.clone();
            }
            fs::write(n.path(), serde_json::to_vec(&v).expect("json")).expect("write");
        }
    }

    /// A sandbox whose daemon finds this fake first on `PATH`.
    fn sandbox(&self) -> Sandbox {
        let mut sandbox = Sandbox::new();
        let path = std::env::var("PATH").unwrap_or_default();
        sandbox.env.push((
            "PATH".into(),
            format!("{}:{path}", self.dir.path().join("bin").display()),
        ));
        sandbox
    }

    fn session_id(&self, file: &str) -> String {
        let v: Value =
            serde_json::from_slice(&fs::read(self.dir.path().join("fx").join(file)).expect("read"))
                .expect("json");
        v["session_id"].as_str().expect("session_id").to_string()
    }
}

fn create_agent(client: &mut Client, agent: &str) -> String {
    let created = client.call(
        "session.create",
        json!({"agent": agent, "cwd": "/tmp", "cols": 80, "rows": 24}),
    );
    created["id"].as_str().expect("id").to_string()
}

/// The statuses announced for a session, consecutive repeats removed.
fn statuses(client: &Client, id: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (name, params) in &client.notes {
        if name == "session.status" && params["id"] == id {
            let s = params["status"].as_str().expect("status").to_string();
            if out.last() != Some(&s) {
                out.push(s);
            }
        }
    }
    out
}

fn wait_status(client: &mut Client, id: &str, count: usize, last: &str) {
    client.pump_until(last, 15, |c| {
        let s = statuses(c, id);
        s.len() >= count && s.last().is_some_and(|l| l == last)
    });
}

fn wait_file(path: &Path, needle: &str) {
    wait_until(needle, 15, || {
        fs::read_to_string(path).is_ok_and(|t| t.contains(needle))
    });
}

fn list_entry(client: &mut Client, id: &str) -> Value {
    // After a daemon restart a session is `detached` until its holder answered.
    wait_settled(client, id);
    let list = client.call("session.list", Value::Null);
    list.as_array()
        .and_then(|l| l.iter().find(|s| s["id"] == id))
        .cloned()
        .expect("session in list")
}

fn hook_client(sandbox: &Sandbox) -> Client {
    let mut hook = Client::connect(&sandbox.socket());
    hook.call(
        "hello",
        json!({"protocol": 1, "client": "t", "role": "hook"}),
    );
    hook
}

fn hook_params(id: &str, token: &str, agent: &str, payload: Value) -> Value {
    json!({"session": id, "token": token, "event": "x", "agent": agent, "payload": payload})
}

const CLAUDE_FLOW: &str = r#"
run('SessionStart', 'SessionStart-startup.json');
run('UserPromptSubmit', 'UserPromptSubmit.json');
run('PreToolUse', 'PreToolUse-bash.json');
run('PermissionRequest', 'PermissionRequest-bash.json', 'answer');
run('PostToolUse', 'PostToolUse-bash.json');
run('Stop', 'Stop.json');
say('done');
stay();
"#;

#[test]
fn claude_hooks_from_settings_drive_status_asks_and_the_session_id() {
    let fake = Fake::new("claude", CLAUDE_FLOW);
    fake.unify_claude();
    let mut sandbox = fake.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = create_agent(&mut client, "claude");
    // The fake sends SessionStart at once, so a fast machine may already be idle.
    let first = list_entry(&mut client, &id)["agentStatus"].clone();
    assert!(first == "starting" || first == "idle", "{first}");

    let ask = client.wait_note("session.ask");
    assert_eq!(ask["session"], id.as_str());
    assert_eq!(ask["tool"], "Bash");
    assert_eq!(ask["input"]["command"], "touch probe.txt");
    let entry = list_entry(&mut client, &id);
    assert_eq!(entry["agentStatus"], "blocked");
    assert_eq!(entry["asks"][0]["askId"], ask["askId"]);
    assert_eq!(entry["status"], "running", "process status stays separate");
    assert!(
        entry.get("tokenHash").is_none(),
        "token hash leaked to a client"
    );

    client.call(
        "ask.respond",
        json!({"askId": ask["askId"], "decision": "allow"}),
    );
    wait_file(&fake.out("done"), "x");
    let answer: Value = serde_json::from_str(&fake.read("answer")).expect("answer json");
    assert_eq!(
        answer["hookSpecificOutput"]["hookEventName"],
        "PermissionRequest"
    );
    assert_eq!(
        answer["hookSpecificOutput"]["decision"]["behavior"],
        "allow"
    );

    wait_status(&mut client, &id, 5, "idle");
    assert_eq!(
        statuses(&client, &id),
        ["idle", "working", "blocked", "working", "idle"]
    );
    let closed = client.wait_note("session.askClosed");
    assert_eq!(closed["askId"], ask["askId"]);
    assert_eq!(closed["decided"], true);
    let entry = list_entry(&mut client, &id);
    assert_eq!(
        entry["agentSession"],
        fake.session_id("SessionStart-startup.json").as_str()
    );
    assert!(entry.get("asks").is_none(), "answered ask still listed");
    let meta = client.wait_note("session.meta");
    assert_eq!(meta["agentSession"], entry["agentSession"]);
}

#[test]
fn a_denied_ask_prints_the_deny_answer() {
    let fake = Fake::new("claude", CLAUDE_FLOW);
    fake.unify_claude();
    let mut sandbox = fake.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    create_agent(&mut client, "claude");
    let ask = client.wait_note("session.ask");
    client.call(
        "ask.respond",
        json!({"askId": ask["askId"], "decision": "deny", "message": "not now"}),
    );
    wait_file(&fake.out("answer"), "deny");
    assert!(fake.read("answer").contains("not now"));
    let missing = client.try_call("ask.respond", json!({"askId": "nope", "decision": "allow"}));
    assert_eq!(missing.expect_err("unknown ask").code, 1008);
}

#[test]
fn an_unanswered_ask_times_out_and_is_reported_closed_while_the_agent_lives() {
    let flow = r#"
run('SessionStart', 'SessionStart-startup.json');
run('UserPromptSubmit', 'UserPromptSubmit.json');
run('PermissionRequest', 'PermissionRequest-bash.json', 'answer');
say('done');
stay();
"#;
    let fake = Fake::new("claude", flow);
    fake.unify_claude();
    let mut sandbox = fake.sandbox();
    sandbox
        .env
        .push(("SUSHIAI_ASK_TIMEOUT_MS".into(), "500".into()));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = create_agent(&mut client, "claude");
    client.wait_note("session.ask");
    let closed = client.wait_note("session.askClosed");
    assert_eq!(closed["decided"], false);
    wait_file(&fake.out("done"), "x");
    assert_eq!(
        fake.read("answer").trim(),
        "",
        "a timed-out ask printed a decision"
    );
    let entry = list_entry(&mut client, &id);
    assert!(entry.get("asks").is_none(), "expired ask still listed");
    assert_eq!(entry["status"], "running", "the agent is still alive");
    assert_eq!(
        entry["agentStatus"], "blocked",
        "a handed-back ask stays blocked"
    );
}

fn codex_env(home: &Path) -> Vec<(String, String)> {
    vec![
        ("HOME".into(), home.display().to_string()),
        (
            "CODEX_HOME".into(),
            home.join(".codex").display().to_string(),
        ),
    ]
}

fn sushiai_hooks(home: &Path, action: &str) -> String {
    let mut command = Command::new(BIN);
    command.args(["hooks", action]).env_remove("SUSHIAI_HOME");
    for (k, v) in codex_env(home) {
        command.env(k, v);
    }
    let out = command.output().expect("run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn codex_hooks_from_the_installed_hooks_json_report_the_session_id_and_idle() {
    let flow = r#"
run('SessionStart', 'SessionStart-startup.json');
run('UserPromptSubmit', 'UserPromptSubmit.json');
run('Stop', 'Stop.json');
say('done');
stay();
"#;
    let fake = Fake::new("codex", flow);
    let home = tempfile::Builder::new()
        .prefix("hm")
        .tempdir_in("/tmp")
        .expect("tempdir");
    sushiai_hooks(home.path(), "install");
    let mut sandbox = fake.sandbox();
    sandbox.env.extend(codex_env(home.path()));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = create_agent(&mut client, "codex");
    wait_file(&fake.out("done"), "x");
    wait_status(&mut client, &id, 3, "idle");
    assert_eq!(statuses(&client, &id), ["idle", "working", "idle"]);
    let entry = list_entry(&mut client, &id);
    assert_eq!(
        entry["agentSession"],
        fake.session_id("SessionStart-startup.json").as_str()
    );
    assert_eq!(entry["agent"], "codex");
    assert_eq!(entry["statusSource"], "hook");
}

#[test]
fn a_nested_agent_cannot_report_into_its_parent_session() {
    // The parent is claude; it then runs a nested codex hook with the same variables.
    let flow = r#"
run('SessionStart', 'SessionStart-startup.json');
run('UserPromptSubmit', 'UserPromptSubmit.json');
const nested = spawnSync(BIN, ['hook', 'permission', '--agent', 'codex'], {
  input: JSON.stringify({ hook_event_name: 'PermissionRequest', session_id: 'nested-codex', tool_name: 'Bash', tool_input: { command: 'rm x' } }),
  encoding: 'utf8',
});
say('nested', nested.status + ':' + nested.stdout);
say('done');
stay();
"#;
    let fake = Fake::new("claude", flow);
    fake.unify_claude();
    let mut sandbox = fake.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = create_agent(&mut client, "claude");
    wait_file(&fake.out("done"), "x");
    assert_eq!(
        fake.read("nested").trim(),
        "0:",
        "nested hook must be silent"
    );
    wait_status(&mut client, &id, 2, "working");
    let entry = list_entry(&mut client, &id);
    assert_eq!(
        entry["agentSession"],
        fake.session_id("SessionStart-startup.json").as_str()
    );
    assert_eq!(entry["agentStatus"], "working");
    assert!(entry.get("asks").is_none());
    assert!(!client.notes.iter().any(|n| n.0 == "session.ask"));
}

#[test]
fn a_turn_stop_while_a_subagent_runs_keeps_the_session_working() {
    let fake = Fake::new(
        "claude",
        "say('token', process.env.SUSHIAI_SESSION_TOKEN); stay();",
    );
    let mut sandbox = fake.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = create_agent(&mut client, "claude");
    wait_until("token file", 15, || !fake.read("token").is_empty());
    let token = fake.read("token").trim().to_string();
    let mut hook = hook_client(&sandbox);
    let mut send = |payload: Value| {
        hook.call("hook.event", hook_params(&id, &token, "claude", payload));
    };
    let sub = |name: &str| json!({"hook_event_name": name, "agent_id": "a1"});
    send(json!({"hook_event_name": "UserPromptSubmit"}));
    send(sub("SubagentStart"));
    send(json!({"hook_event_name": "Stop"}));
    assert_eq!(list_entry(&mut client, &id)["agentStatus"], "working");
    send(sub("SubagentStop"));
    assert_eq!(list_entry(&mut client, &id)["agentStatus"], "idle");
}

#[test]
fn a_hook_for_another_agent_is_rejected_by_the_daemon_too() {
    let fake = Fake::new(
        "claude",
        "say('token', process.env.SUSHIAI_SESSION_TOKEN); stay();",
    );
    let mut sandbox = fake.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = create_agent(&mut client, "claude");
    wait_until("token file", 15, || !fake.read("token").is_empty());
    let token = fake.read("token").trim().to_string();
    let mut hook = hook_client(&sandbox);
    let payload = json!({"hook_event_name": "SessionStart", "session_id": "other"});
    let other = hook.try_call(
        "hook.event",
        hook_params(&id, &token, "codex", payload.clone()),
    );
    assert_eq!(other.expect_err("other agent").code, 1007);
    hook.call("hook.event", hook_params(&id, &token, "claude", payload));
    assert_eq!(list_entry(&mut client, &id)["agentSession"], "other");
}

#[test]
fn a_wrong_token_is_rejected_and_hook_connections_only_call_hooks() {
    let fake = Fake::new(
        "claude",
        "say('token', process.env.SUSHIAI_SESSION_TOKEN); stay();",
    );
    let mut sandbox = fake.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = create_agent(&mut client, "claude");
    let mut hook = hook_client(&sandbox);
    let payload = json!({"hook_event_name": "UserPromptSubmit", "session_id": "x"});
    let bad = hook.try_call(
        "hook.event",
        hook_params(&id, "wrong", "claude", payload.clone()),
    );
    assert_eq!(bad.expect_err("wrong token").code, 1007);
    let list = hook.try_call("session.list", Value::Null);
    assert_eq!(list.expect_err("hook role").code, 1007);
    // hook.event is for hook connections only: a normal client is refused even with the
    // right token.
    wait_until("token file", 15, || !fake.read("token").is_empty());
    let token = fake.read("token").trim().to_string();
    let normal = client.try_call("hook.event", hook_params(&id, &token, "claude", payload));
    assert_eq!(normal.expect_err("normal client").code, 1007);
    assert_eq!(list_entry(&mut client, &id)["agentStatus"], "starting");
}

fn hook_command(event: &str) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(["hook", event])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run_hook_with(
    mut command: Command,
    payload: &[u8],
) -> (std::process::ExitStatus, String, Duration) {
    let start = Instant::now();
    let mut child = command.spawn().expect("spawn hook");
    // A hook that refuses its input may exit before reading it: ignore a broken pipe.
    let _ = child.stdin.take().expect("stdin").write_all(payload);
    let out = child.wait_with_output().expect("wait");
    (
        out.status,
        String::from_utf8_lossy(&out.stdout).into_owned(),
        start.elapsed(),
    )
}

fn run_hook(command: Command) -> (std::process::ExitStatus, String, Duration) {
    run_hook_with(command, br#"{"hook_event_name":"Stop"}"#)
}

#[test]
fn the_hook_log_names_variables_and_outcomes_but_never_values() {
    let dir = tempfile::Builder::new()
        .prefix("hl")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let log = dir.path().join("hook.log");
    let secret = "tok-secret-0123456789";
    // No daemon at the socket: the trail records the failed connect.
    let mut command = hook_command("stop");
    command
        .args(["--agent", "codex"])
        .env("SUSHIAI_HOOK_LOG", &log)
        .env("SUSHIAI_AGENT", "codex")
        .env("SUSHIAI_SOCKET", dir.path().join("none.sock"))
        .env("SUSHIAI_SESSION_ID", "sess-id-value")
        .env("SUSHIAI_SESSION_TOKEN", secret)
        .env_remove("SUSHIAI_HOME");
    let (status, stdout, _) = run_hook_with(
        command,
        br#"{"hook_event_name":"Stop","x":"payload-value"}"#,
    );
    assert!(status.success());
    assert_eq!(stdout, "");
    // Without session variables the trail stops at the first step.
    let mut command = hook_command("prompt");
    command
        .args(["--agent", "codex"])
        .env("SUSHIAI_HOOK_LOG", &log);
    for var in [
        "SUSHIAI_SOCKET",
        "SUSHIAI_SESSION_ID",
        "SUSHIAI_SESSION_TOKEN",
        "SUSHIAI_AGENT",
    ] {
        command.env_remove(var);
    }
    let (status, _, _) = run_hook(command);
    assert!(status.success());

    let text = fs::read_to_string(&log).expect("log");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[0].contains("event=stop agent=codex"), "{text}");
    assert!(lines[0].contains("SUSHIAI_SESSION_TOKEN=set"), "{text}");
    assert!(lines[0].contains("SUSHIAI_HOME=unset"), "{text}");
    assert!(lines[0].contains("step=connect connect=NotFound"), "{text}");
    assert!(lines[1].contains("event=prompt"), "{text}");
    assert!(lines[1].contains("SUSHIAI_AGENT=unset"), "{text}");
    assert!(lines[1].contains("step=env"), "{text}");
    for value in [secret, "sess-id-value", "none.sock", "payload-value"] {
        assert!(!text.contains(value), "{value} leaked: {text}");
    }
    let mode = fs::metadata(&log).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn a_hook_without_session_variables_exits_zero_silently() {
    let mut command = hook_command("permission");
    for var in [
        "SUSHIAI_SOCKET",
        "SUSHIAI_SESSION_ID",
        "SUSHIAI_SESSION_TOKEN",
        "SUSHIAI_AGENT",
    ] {
        command.env_remove(var);
    }
    let (status, stdout, _) = run_hook(command);
    assert!(status.success());
    assert_eq!(stdout, "");
}

#[test]
fn a_hook_with_the_daemon_down_exits_zero_within_two_seconds() {
    let dir = tempfile::Builder::new()
        .prefix("hd")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let mut command = hook_command("stop");
    command
        .env("SUSHIAI_SOCKET", dir.path().join("none.sock"))
        .env("SUSHIAI_SESSION_ID", "s")
        .env("SUSHIAI_SESSION_TOKEN", "t")
        .env("SUSHIAI_AGENT", "claude");
    let (status, stdout, took) = run_hook(command);
    assert!(status.success());
    assert_eq!(stdout, "");
    assert!(took < Duration::from_secs(2), "took {took:?}");
}

#[test]
fn status_and_a_blocked_ask_survive_a_killed_daemon_and_the_token_is_never_stored() {
    let flow = r#"
say('token', process.env.SUSHIAI_SESSION_TOKEN);
run('SessionStart', 'SessionStart-startup.json');
run('UserPromptSubmit', 'UserPromptSubmit.json');
run('PermissionRequest', 'PermissionRequest-bash.json', 'answer');
stay();
"#;
    let fake = Fake::new("claude", flow);
    fake.unify_claude();
    let mut sandbox = fake.sandbox();
    let first = sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = create_agent(&mut client, "claude");
    client.wait_note("session.ask");
    wait_status(&mut client, &id, 3, "blocked");
    let token = fake.read("token").trim().to_string();
    assert!(!token.is_empty());
    let state_path = sandbox.home().join("state.json");
    wait_until("blocked in the state file", 5, || {
        fs::read_to_string(&state_path).is_ok_and(|t| t.contains("blocked"))
    });

    kill(first, "-KILL");
    wait_until("daemon to die", 5, || !alive(first));
    let state = fs::read_to_string(&state_path).expect("state");
    assert!(
        !state.contains(&token),
        "the token was written to state.json"
    );

    sandbox.start_daemon();
    let mut client = sandbox.client();
    let entry = list_entry(&mut client, &id);
    assert_eq!(entry["agentStatus"], "blocked", "restart lost the status");
    assert_eq!(entry["statusSource"], "hook");
    assert!(entry.get("asks").is_none(), "an ask survived the daemon");
    assert_eq!(
        entry["agentSession"],
        fake.session_id("SessionStart-startup.json").as_str()
    );

    // The agent still holds the original token: the new daemon checks it against the hash.
    let mut hook = hook_client(&sandbox);
    let stop = fs::read_to_string(Path::new(FIXTURES).join("claude/Stop.json")).expect("stop");
    let payload: Value = serde_json::from_str(&stop).expect("json");
    let wrong = hook.try_call(
        "hook.event",
        hook_params(&id, "wrong", "claude", payload.clone()),
    );
    assert_eq!(wrong.expect_err("wrong").code, 1007);
    hook.call("hook.event", hook_params(&id, &token, "claude", payload));
    assert_eq!(list_entry(&mut client, &id)["agentStatus"], "idle");
}

#[test]
fn asks_are_limited_in_number_and_size_and_oversize_hook_input_is_not_forwarded() {
    let flow = r#"
say('token', process.env.SUSHIAI_SESSION_TOKEN);
run('SessionStart', 'SessionStart-startup.json');
run('UserPromptSubmit', 'UserPromptSubmit.json');
say('ready');
stay();
"#;
    let fake = Fake::new("claude", flow);
    fake.unify_claude();
    let mut sandbox = fake.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = create_agent(&mut client, "claude");
    wait_file(&fake.out("ready"), "x");
    let token = fake.read("token").trim().to_string();

    // A 1.5 MiB payload is never forwarded: the status does not move.
    let mut command = hook_command("stop");
    command
        .env("SUSHIAI_SOCKET", sandbox.socket())
        .env("SUSHIAI_SESSION_ID", &id)
        .env("SUSHIAI_SESSION_TOKEN", &token)
        .env("SUSHIAI_AGENT", "claude");
    let big = json!({"hook_event_name": "Stop", "pad": "x".repeat(1536 * 1024)}).to_string();
    let (status, _, _) = run_hook_with(command, big.as_bytes());
    assert!(status.success());
    assert_eq!(list_entry(&mut client, &id)["agentStatus"], "working");

    // 21 asks: the first has a huge input, the 21st gets no decision and no ask.
    let ask_payload = |input: Value| json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash", "tool_input": input});
    let mut waiting = Vec::new();
    for n in 0..21 {
        let input = if n == 0 {
            json!({"command": "y".repeat(200 * 1024)})
        } else {
            json!({"command": format!("echo {n}")})
        };
        let mut hook = hook_client(&sandbox);
        let rid = hook.send(
            "hook.event",
            hook_params(&id, &token, "claude", ask_payload(input)),
        );
        waiting.push((hook, rid));
        if n < 20 {
            let want = n + 1;
            client.pump_until("ask", 15, |c| {
                c.notes.iter().filter(|x| x.0 == "session.ask").count() >= want
            });
        }
    }
    let (mut last, rid) = waiting.pop().expect("21st");
    let result = last.response("hook.event", rid).expect("21st answered");
    assert!(result["answer"].is_null(), "the 21st ask got a decision");
    let asks: Vec<&(String, Value)> = client
        .notes
        .iter()
        .filter(|n| n.0 == "session.ask")
        .collect();
    assert_eq!(asks.len(), 20, "more than 20 asks were opened");
    let first = &asks[0].1;
    assert_eq!(first["tool"], "Bash");
    let text = first["input"].as_str().expect("clipped input is a string");
    assert!(text.len() <= 64 * 1024 && text.starts_with("{\"command\""));
    assert_eq!(
        list_entry(&mut client, &id)["asks"]
            .as_array()
            .map(Vec::len),
        Some(20)
    );
}

#[test]
fn an_exited_session_takes_no_more_hooks() {
    let fake = Fake::new(
        "claude",
        "say('token', process.env.SUSHIAI_SESSION_TOKEN); say('done');",
    );
    let mut sandbox = fake.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = create_agent(&mut client, "claude");
    client.wait_note("session.exited");
    let token = fake.read("token").trim().to_string();
    let mut hook = hook_client(&sandbox);
    let payload = json!({"hook_event_name": "UserPromptSubmit"});
    let start = Instant::now();
    let late = hook.try_call("hook.event", hook_params(&id, &token, "claude", payload));
    assert_eq!(late.expect_err("exited").code, 1007);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "waited for a dead actor"
    );
    let (state_path, hash) = (
        sandbox.home().join("state.json"),
        common::sha256_hex(&token),
    );
    wait_until("the token hash to leave the state file", 5, || {
        let state = fs::read_to_string(&state_path).unwrap_or_default();
        state.contains("exited") && !state.contains(&hash)
    });
}

#[test]
fn hooks_install_wires_trust_idempotently_and_uninstall_restores_user_entries() {
    let home = tempfile::Builder::new()
        .prefix("hm")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let codex = home.path().join(".codex");
    fs::create_dir(&codex).expect("mkdir");
    let user = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "echo mine", "timeout": 5}]}]}});
    let file = codex.join("hooks.json");
    fs::write(&file, serde_json::to_vec_pretty(&user).expect("json")).expect("write");
    let config = codex.join("config.toml");
    let user_config =
        "model = \"gpt\"\n\n[hooks.state.\"user-key\"]\ntrusted_hash = \"sha256:abc\"\n";
    fs::write(&config, user_config).expect("config");

    let first = sushiai_hooks(home.path(), "install");
    assert!(
        first.contains("bin link:")
            && first.contains("hooks.json: installed")
            && first.contains("config.toml: trusted"),
        "each step reports its result: {first}"
    );
    assert!(!first.contains("not implemented") && !first.contains("must trust"));
    let link = home.path().join(".sushiai/bin/sushiai");
    assert_eq!(fs::read_link(&link).expect("symlink"), Path::new(BIN));
    let installed = fs::read_to_string(&file).expect("read");
    assert!(installed.contains("echo mine") && installed.contains("hook session-start"));
    let trusted = fs::read_to_string(&config).expect("config");
    assert!(trusted.contains("model = \"gpt\"") && trusted.contains("user-key"));
    assert!(
        trusted.matches("trusted_hash").count() > 1,
        "no trust keys for our handlers: {trusted}"
    );

    let second = sushiai_hooks(home.path(), "install");
    assert!(second.contains("no change"), "second install: {second}");
    assert_eq!(fs::read_to_string(&file).expect("read"), installed);
    assert_eq!(fs::read_to_string(&config).expect("config"), trusted);

    let removed = sushiai_hooks(home.path(), "uninstall");
    assert!(removed.contains("config.toml: untrusted") && removed.contains("hooks.json: removed"));
    let restored: Value = serde_json::from_slice(&fs::read(&file).expect("read")).expect("json");
    assert_eq!(restored, user);
    let config_after = fs::read_to_string(&config).expect("config");
    assert!(config_after.contains("model = \"gpt\"") && config_after.contains("user-key"));
    assert_eq!(
        config_after.matches("trusted_hash").count(),
        1,
        "{config_after}"
    );
}

#[test]
fn hooks_install_refuses_to_overwrite_a_regular_file_at_the_stable_path() {
    let home = tempfile::Builder::new()
        .prefix("hm")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let bin = home.path().join(".sushiai/bin");
    fs::create_dir_all(&bin).expect("mkdir");
    fs::write(bin.join("sushiai"), "mine").expect("file");
    let mut command = Command::new(BIN);
    command
        .args(["hooks", "install"])
        .env_remove("SUSHIAI_HOME");
    for (k, v) in codex_env(home.path()) {
        command.env(k, v);
    }
    let out = command.output().expect("run");
    assert!(!out.status.success());
    assert_eq!(
        fs::read_to_string(bin.join("sushiai")).expect("read"),
        "mine"
    );
    assert!(!home.path().join(".codex/hooks.json").exists());
}

#[test]
fn running_the_binary_through_its_own_link_keeps_the_link_pointing_at_the_binary() {
    let home = tempfile::Builder::new()
        .prefix("hm")
        .tempdir_in("/tmp")
        .expect("tempdir");
    sushiai_hooks(home.path(), "install");
    let link = home.path().join(".sushiai/bin/sushiai");
    let real = fs::canonicalize(BIN).expect("real binary");
    assert_eq!(fs::canonicalize(&link).expect("link resolves"), real);

    // Install again, this time started through the link.
    let mut command = Command::new(&link);
    command
        .args(["hooks", "install"])
        .env_remove("SUSHIAI_HOME");
    for (k, v) in codex_env(home.path()) {
        command.env(k, v);
    }
    let out = command.output().expect("run through the link");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::canonicalize(&link).expect("link still resolves"),
        real,
        "the link no longer reaches the binary"
    );
    let usage = Command::new(&link).output().expect("run");
    assert!(String::from_utf8_lossy(&usage.stderr).contains("usage:"));
}

#[test]
fn a_session_that_lost_its_holder_across_a_restart_drops_its_token() {
    let mut sandbox = Sandbox::new();
    let state = json!({"schemaVersion": 1, "sessions": [{
        "id": "dead1", "cmd": ["claude"], "cwd": "/tmp", "title": null,
        "status": "running", "cols": 80, "rows": 24, "agent": "claude",
        "agentStatus": "working", "statusSource": "hook", "statusSince": 1,
        "tokenHash": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
    }]});
    let path = sandbox.home().join("state.json");
    fs::write(&path, serde_json::to_vec(&state).expect("json")).expect("write state");
    sandbox.start_daemon();
    let mut client = sandbox.client();
    assert_eq!(list_entry(&mut client, "dead1")["status"], "exited");
    wait_until("the hash to leave the state file", 5, || {
        let text = fs::read_to_string(&path).unwrap_or_default();
        text.contains("exited") && !text.contains("deadbeef")
    });
}

#[test]
fn hooks_install_puts_the_link_under_sushiai_home_and_needs_codex_home_for_a_custom_home() {
    let home = tempfile::Builder::new()
        .prefix("hm")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let custom = home.path().join("other");
    let run = |codex: Option<&Path>| {
        let mut command = Command::new(BIN);
        command
            .args(["hooks", "install"])
            .env("HOME", home.path())
            .env("SUSHIAI_HOME", &custom)
            .env_remove("CODEX_HOME");
        if let Some(codex) = codex {
            command.env("CODEX_HOME", codex);
        }
        command.output().expect("run")
    };
    // Without CODEX_HOME the default ~/.codex would change: refused, nothing written.
    let refused = run(None);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("CODEX_HOME"));
    assert!(!home.path().join(".codex").exists());
    assert!(!custom.join("bin/sushiai").exists());

    // With an explicit CODEX_HOME the link lives under SUSHIAI_HOME, not ~/.sushiai.
    let codex = home.path().join("codex-test");
    let ok = run(Some(&codex));
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert_eq!(
        fs::read_link(custom.join("bin/sushiai")).expect("link"),
        Path::new(BIN)
    );
    assert!(!home.path().join(".sushiai").exists());
    assert!(!home.path().join(".codex").exists());
    assert!(codex.join("hooks.json").exists());
}

/// Creates a plain-command session labelled as `agent` (gemini and cursor-agent have no hooks).
fn create_screen_agent(client: &mut Client, agent: &str, script: &str) -> String {
    let created = client.call(
        "session.create",
        json!({"agent": agent, "cmd": ["sh", "-c", script], "cwd": "/tmp", "cols": 80, "rows": 24}),
    );
    created["id"].as_str().expect("id").to_string()
}

fn agent_status(client: &mut Client, id: &str) -> (String, String) {
    let entry = list_entry(client, id);
    (
        entry["agentStatus"].as_str().unwrap_or_default().into(),
        entry["statusSource"].as_str().unwrap_or_default().into(),
    )
}

#[test]
fn gemini_and_cursor_status_is_read_from_the_screen() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    // Gemini asks for confirmation, then (a second screen later) shows nothing it knows.
    let gemini = create_screen_agent(
        &mut client,
        "gemini",
        "printf 'Waiting for user confirmation\\n'; sleep 1; printf '\\033[2J\\033[Hready\\n'; sleep 60",
    );
    wait_status(&mut client, &gemini, 1, "blocked");
    assert_eq!(
        agent_status(&mut client, &gemini),
        ("blocked".into(), "heuristic".into())
    );
    wait_status(&mut client, &gemini, 2, "idle");

    // Cursor shows its working marker.
    let cursor = create_screen_agent(
        &mut client,
        "cursor-agent",
        "printf 'thinking...\\nctrl+c to stop\\n'; sleep 60",
    );
    wait_status(&mut client, &cursor, 1, "working");
    assert_eq!(
        agent_status(&mut client, &cursor),
        ("working".into(), "heuristic".into())
    );
}

#[test]
fn a_codex_dialog_no_hook_reports_blocks_the_session_until_hooks_take_over() {
    let flow = r#"
process.stdout.write('Do you trust the contents of this directory?\n');
say('dialog');
setTimeout(() => {
  run('SessionStart', 'SessionStart-startup.json');
  say('hooked');
  process.stdout.write('Allow command?\n');
  say('done');
}, 1500);
stay();
"#;
    let fake = Fake::new("codex", flow);
    let home = tempfile::Builder::new()
        .prefix("hm")
        .tempdir_in("/tmp")
        .expect("tempdir");
    sushiai_hooks(home.path(), "install");
    let mut sandbox = fake.sandbox();
    sandbox.env.extend(codex_env(home.path()));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = create_agent(&mut client, "codex");
    // Before any hook the trust dialog on the screen is all there is to go by.
    wait_status(&mut client, &id, 1, "blocked");
    assert_eq!(
        agent_status(&mut client, &id),
        ("blocked".into(), "heuristic".into())
    );
    // Once the hooks speak, they win: the later "Allow command?" text changes nothing.
    wait_file(&fake.out("done"), "x");
    std::thread::sleep(Duration::from_millis(1200));
    let (status, source) = agent_status(&mut client, &id);
    assert_eq!(source, "hook", "hooks took over");
    assert_eq!(status, "idle", "the screen text did not override the hook");
}
