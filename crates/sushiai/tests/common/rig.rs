//! A fake `claude` / `codex` on `PATH` for the end-to-end tests that need a real agent session
//! (hibernation, scale): hooks go through `sushiai hook` like the real CLIs, and the fake logs
//! its argv, variables and keystrokes to a file and stays alive until it is told to end.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

use super::{wait_until, Client, Sandbox, BIN};

pub const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../sushiai-agents/tests/fixtures"
);

pub const SECRET: &str = "launch-secret-value-7431";

const FAKE: &str = r#"#!/usr/bin/env node
const { spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');
const KIND = '@KIND@', F = '@F@', OUT = '@OUT@', FLOOD = @FLOOD@;
const log = (line) => fs.appendFileSync(path.join(OUT, 'log'), line + '\n');
log('argv ' + JSON.stringify(process.argv.slice(2)));
log('env ' + (process.env.API_TOKEN || ''));
log('size ' + process.stdout.columns + 'x' + process.stdout.rows);
process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.on('data', (d) => log('stdin ' + d.toString()));
// FAKE_TERM_DELAY_MS keeps the process alive that long after TERM (a slow shutdown).
process.on('SIGTERM', () => setTimeout(() => process.exit(0), Number(process.env.FAKE_TERM_DELAY_MS || 0)));
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
// FAKE_QUIET_RESUME: a resumed agent sends no hook until its first prompt (Codex).
const QUIET = process.env.FAKE_QUIET_RESUME && process.argv.includes('resume');
for (let i = 0; i < FLOOD; i++) console.log('flood-' + KIND + '-' + i + ' ' + 'x'.repeat(180));
console.log('tail-marker-' + KIND);
Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 300);
if (!QUIET) run('SessionStart', 'SessionStart-startup.json');
if (KIND === 'codex' && !QUIET) {
  run('UserPromptSubmit', 'UserPromptSubmit.json');
  run('Stop', 'Stop.json');
}
log('ready');
setInterval(() => {}, 60000);
"#;

/// A fake agent on `PATH`, with the temp variables a daemon needs to stay away from the real
/// ones: its own `HOME`, config directories and launch key file.
pub struct Rig {
    pub dir: tempfile::TempDir,
    pub kind: &'static str,
}

impl Rig {
    pub fn new(kind: &'static str) -> Rig {
        Rig::flooding(kind, 0)
    }

    /// A rig whose fake `kind` prints `lines` lines of 200 columns before anything else.
    pub fn flooding(kind: &'static str, lines: u32) -> Rig {
        let dir = tempfile::Builder::new()
            .prefix("hb")
            .tempdir_in("/tmp")
            .expect("tempdir");
        for sub in ["bin", "fx", "out", "home"] {
            fs::create_dir(dir.path().join(sub)).expect("mkdir");
        }
        let rig = Rig { dir, kind };
        rig.install(kind, lines);
        rig
    }

    /// Puts one more fake agent, `kind`, on this rig's `PATH`.
    pub fn install(&self, kind: &'static str, lines: u32) {
        let fixtures = self.dir.path().join("fx").join(kind);
        fs::create_dir(&fixtures).expect("mkdir");
        for entry in fs::read_dir(Path::new(FIXTURES).join(kind)).expect("fixtures") {
            let path = entry.expect("entry").path();
            if path.extension().is_some_and(|e| e == "json") {
                let target = fixtures.join(path.file_name().expect("name"));
                fs::copy(&path, target).expect("copy");
            }
        }
        let script = FAKE
            .replace("@KIND@", kind)
            .replace("@F@", &fixtures.display().to_string())
            .replace("@OUT@", &self.dir.path().join("out").display().to_string())
            .replace("@FLOOD@", &lines.to_string());
        let bin = self.dir.path().join("bin").join(kind);
        fs::write(&bin, script).expect("write script");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    /// A sandbox whose daemon finds the fake first on `PATH`. `after_ms` is the idle time that
    /// puts a session to sleep; None leaves the default (hours).
    pub fn sandbox(&self, after_ms: Option<u64>) -> Sandbox {
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
    pub fn install_codex_hooks(&self) {
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

    pub fn create(&self, client: &mut Client) -> String {
        self.create_as(client, self.kind)
    }

    pub fn create_as(&self, client: &mut Client, kind: &str) -> String {
        let created = client.call(
            "session.create",
            json!({
                "agent": kind, "cwd": "/tmp", "cols": 80, "rows": 24,
                "env": {"API_TOKEN": SECRET},
            }),
        );
        created["id"].as_str().expect("id").to_string()
    }

    pub fn log(&self) -> String {
        fs::read_to_string(self.dir.path().join("out").join("log")).unwrap_or_default()
    }

    /// The argv lines of the fake, one per process it ran as.
    pub fn argvs(&self) -> Vec<String> {
        self.log()
            .lines()
            .filter_map(|l| l.strip_prefix("argv "))
            .map(str::to_string)
            .collect()
    }

    /// Everything typed into the fake, in order.
    pub fn typed(&self) -> String {
        self.log()
            .lines()
            .filter_map(|l| l.strip_prefix("stdin "))
            .collect()
    }
}

pub fn entry(client: &mut Client, id: &str) -> Value {
    client
        .call("session.list", Value::Null)
        .as_array()
        .and_then(|l| l.iter().find(|s| s["id"] == id))
        .cloned()
        .expect("session in list")
}

pub fn status_of(client: &mut Client, id: &str) -> String {
    entry(client, id)["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

pub fn wait_status(client: &mut Client, id: &str, want: &str) {
    wait_until(&format!("status {want}"), 30, || {
        status_of(client, id) == want
    });
}

/// Waits until the agent is idle with a conversation id: the state a session may sleep from.
pub fn wait_idle(client: &mut Client, id: &str) {
    wait_until("an idle agent with a session id", 20, || {
        let e = entry(client, id);
        e["agentStatus"] == "idle" && e["agentSession"].is_string()
    });
}
