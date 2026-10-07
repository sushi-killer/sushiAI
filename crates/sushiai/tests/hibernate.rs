//! Session hibernation and wake end to end: a real daemon and holders in a temp home, with fake
//! `claude` and `codex` node scripts on `PATH`. They send their hooks through `sushiai hook` like
//! the real CLIs (see `agents.rs`), log their argv, variables and keystrokes to a file, and stay
//! alive until they are told to end.

mod common;

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use common::*;
use serde_json::{json, Value};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../sushiai-agents/tests/fixtures"
);

const SECRET: &str = "launch-secret-value-7431";

const FAKE: &str = r#"#!/usr/bin/env node
const { spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const KIND = '@KIND@', F = '@F@', OUT = '@OUT@';
const log = (line) => fs.appendFileSync(path.join(OUT, 'log'), line + '\n');
log('argv ' + JSON.stringify(process.argv.slice(2)));
log('env ' + (process.env.API_TOKEN || ''));
process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.on('data', (d) => log('stdin ' + d.toString()));
process.on('SIGTERM', () => process.exit(0));
function commandFor(event) {
  if (KIND === 'claude') {
    const s = JSON.parse(process.argv[process.argv.indexOf('--settings') + 1]);
    return s.hooks[event][0].hooks[0].command;
  }
  const h = JSON.parse(fs.readFileSync(path.join(process.env.CODEX_HOME, 'hooks.json'), 'utf8'));
  for (const g of h.hooks[event]) for (const x of g.hooks) if (x.command.includes('sushiai')) return x.command;
  throw new Error('no hook for ' + event);
}
function run(event, file) {
  spawnSync('sh', ['-c', commandFor(event)], { input: fs.readFileSync(path.join(F, file)) });
}
console.log('tail-marker-' + KIND);
Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 300);
run('SessionStart', 'SessionStart-startup.json');
if (KIND === 'codex') {
  run('UserPromptSubmit', 'UserPromptSubmit.json');
  run('Stop', 'Stop.json');
}
log('ready');
setInterval(() => {}, 60000);
"#;

/// A fake agent on `PATH`, with the temp variables a daemon needs to stay away from the real
/// ones: its own `HOME`, config directories and launch key file.
struct Rig {
    dir: tempfile::TempDir,
    kind: &'static str,
}

impl Rig {
    fn new(kind: &'static str) -> Rig {
        let dir = tempfile::Builder::new()
            .prefix("hb")
            .tempdir_in("/tmp")
            .expect("tempdir");
        for sub in ["bin", "fx", "out", "home"] {
            fs::create_dir(dir.path().join(sub)).expect("mkdir");
        }
        for entry in fs::read_dir(Path::new(FIXTURES).join(kind)).expect("fixtures") {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|e| e == "json") {
                let target = dir.path().join("fx").join(path.file_name().expect("name"));
                fs::copy(&path, target).expect("copy");
            }
        }
        let script = FAKE
            .replace("@KIND@", kind)
            .replace("@F@", &dir.path().join("fx").display().to_string())
            .replace("@OUT@", &dir.path().join("out").display().to_string());
        let bin = dir.path().join("bin").join(kind);
        fs::write(&bin, script).expect("write script");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
        Rig { dir, kind }
    }

    /// A sandbox whose daemon finds the fake first on `PATH`. `after_ms` is the idle time that
    /// puts a session to sleep; None leaves the default (hours).
    fn sandbox(&self, after_ms: Option<u64>) -> Sandbox {
        let mut sandbox = Sandbox::new();
        let path = std::env::var("PATH").unwrap_or_default();
        let home = self.dir.path().join("home");
        let env = [
            (
                "PATH",
                format!("{}:{path}", self.dir.path().join("bin").display()),
            ),
            ("HOME", home.display().to_string()),
            (
                "CLAUDE_CONFIG_DIR",
                home.join(".claude").display().to_string(),
            ),
            ("CODEX_HOME", home.join(".codex").display().to_string()),
            (
                "SUSHIAI_LAUNCH_KEY_FILE",
                self.dir.path().join("launch.key").display().to_string(),
            ),
        ];
        sandbox
            .env
            .extend(env.into_iter().map(|(k, v)| (k.to_string(), v)));
        if let Some(ms) = after_ms {
            sandbox
                .env
                .push(("SUSHIAI_HIBERNATE_AFTER_MS".into(), ms.to_string()));
        }
        sandbox
    }

    /// `sushiai hooks install` for the fake codex, which reads `$CODEX_HOME/hooks.json`.
    fn install_codex_hooks(&self) {
        let home = self.dir.path().join("home");
        let out = Command::new(BIN)
            .args(["hooks", "install"])
            .env_remove("SUSHIAI_HOME")
            .env("HOME", &home)
            .env("CODEX_HOME", home.join(".codex"))
            .output()
            .expect("run");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn create(&self, client: &mut Client) -> String {
        let created = client.call(
            "session.create",
            json!({
                "agent": self.kind, "cwd": "/tmp", "cols": 80, "rows": 24,
                "env": {"API_TOKEN": SECRET},
            }),
        );
        created["id"].as_str().expect("id").to_string()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.path().join("out").join("log")).unwrap_or_default()
    }

    /// The argv lines of the fake, one per process it ran as.
    fn argvs(&self) -> Vec<String> {
        self.log()
            .lines()
            .filter_map(|l| l.strip_prefix("argv "))
            .map(str::to_string)
            .collect()
    }

    /// Everything typed into the fake, in order.
    fn typed(&self) -> String {
        self.log()
            .lines()
            .filter_map(|l| l.strip_prefix("stdin "))
            .collect()
    }
}

fn entry(client: &mut Client, id: &str) -> Value {
    client
        .call("session.list", Value::Null)
        .as_array()
        .and_then(|l| l.iter().find(|s| s["id"] == id))
        .cloned()
        .expect("session in list")
}

fn status_of(client: &mut Client, id: &str) -> String {
    entry(client, id)["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn wait_status(client: &mut Client, id: &str, want: &str) {
    wait_until(&format!("status {want}"), 30, || {
        status_of(client, id) == want
    });
}

/// Waits until the agent is idle with a conversation id: the state a session may sleep from.
fn wait_idle(client: &mut Client, id: &str) {
    wait_until("an idle agent with a session id", 20, || {
        let e = entry(client, id);
        e["agentStatus"] == "idle" && e["agentSession"].is_string()
    });
}

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
    // A file that does not open, and no file at all, both mean: no stored launch.
    fs::write(&file, b"SLK1garbage-garbage-garbage").expect("corrupt");
    let broken = client
        .try_call("session.wake", json!({"id": id}))
        .expect_err("a damaged launch cannot wake");
    assert_eq!(broken.code, 1012);
    fs::remove_file(&file).expect("remove");
    let missing = client
        .try_call("session.wake", json!({"id": id}))
        .expect_err("no launch, no wake");
    assert_eq!(missing.code, 1012);
    assert_eq!(status_of(&mut client, &id), "hibernated");
    let unknown = client
        .try_call("session.wake", json!({"id": "nope"}))
        .expect_err("unknown");
    assert_eq!(unknown.code, 1003);

    // Closing a sleeping session ends it and removes what it left.
    client.call("session.close", json!({"id": id, "graceful": true}));
    assert_eq!(status_of(&mut client, &id), "exited");
    assert!(!tail_file(&sandbox, &id).exists());
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
