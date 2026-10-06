//! Launch options and reads: `claudeSettings`, `idempotencyKey`, an account `CODEX_HOME`,
//! attach/read with scrollback, and the `sushiai open` signal. Real daemon and holders; the
//! fake agents are shell scripts first on `PATH`. Temporary homes only.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use serde_json::{json, Value};

/// A temp dir with fake agents in `bin/`; a sandbox whose daemon finds them first.
struct Fakes {
    dir: tempfile::TempDir,
}

impl Fakes {
    fn new() -> Fakes {
        let dir = tempfile::Builder::new()
            .prefix("lg")
            .tempdir_in("/tmp")
            .expect("tempdir");
        fs::create_dir(dir.path().join("bin")).expect("mkdir");
        Fakes { dir }
    }

    fn add(&self, name: &str, body: &str) {
        let bin = self.dir.path().join("bin").join(name);
        fs::write(&bin, format!("#!/bin/sh\n{body}\n")).expect("write");
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn sandbox(&self) -> Sandbox {
        let mut sandbox = Sandbox::new();
        let path = std::env::var("PATH").unwrap_or_default();
        sandbox.env.push((
            "PATH".into(),
            format!("{}:{path}", self.dir.path().join("bin").display()),
        ));
        sandbox
            .env
            .push(("HOME".into(), self.dir.path().display().to_string()));
        sandbox
    }
}

fn agent_params(agent: &str) -> Value {
    json!({"agent": agent, "cwd": "/tmp", "cols": 80, "rows": 24})
}

const SECRET: &str = "sk-test-secret-0123456789";

fn settings_params() -> Value {
    let mut p = agent_params("claude");
    p["claudeSettings"] = json!({"apiKeyHelper": "/opt/helper.sh", "model": "opus"});
    // The secret travels in the session variables, which reach the holder, not argv.
    p["env"] = json!({"ANTHROPIC_API_KEY": SECRET});
    p
}

// G1
#[test]
fn claude_settings_join_our_hooks_in_the_single_settings_argument() {
    let fakes = Fakes::new();
    let out = fakes.path("argv");
    fakes.add(
        "claude",
        &format!(
            "printf '%s' \"$ANTHROPIC_API_KEY\" > {}.var\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> {}; done\nsleep 60",
            out.display(),
            out.display()
        ),
    );
    let mut sandbox = fakes.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let created = client.call("session.create", settings_params());
    let id = created["id"].as_str().expect("id").to_string();
    wait_until("the fake claude argv", 15, || {
        fs::read_to_string(&out).is_ok_and(|t| t.contains("--settings"))
    });
    let argv = fs::read_to_string(&out).expect("argv");
    let lines: Vec<&str> = argv.lines().collect();
    assert_eq!(lines.iter().filter(|l| **l == "--settings").count(), 1);
    let at = lines.iter().position(|l| *l == "--settings").expect("flag");
    let settings: Value = serde_json::from_str(lines[at + 1]).expect("settings json");
    assert_eq!(settings["apiKeyHelper"], "/opt/helper.sh");
    assert!(
        settings.get("env").is_none(),
        "no variables in the settings: {settings}"
    );
    assert!(!argv.contains(SECRET), "the secret reached argv: {argv}");
    let var_file = format!("{}.var", out.display());
    assert_eq!(fs::read_to_string(var_file).expect("var file"), SECRET);
    assert_eq!(settings["model"], "opus");
    assert!(
        settings["hooks"]["PermissionRequest"].is_array(),
        "our hooks must stay: {settings}"
    );

    // Never persisted, never listed.
    wait_until("the state file", 15, || {
        fs::read_to_string(sandbox.home().join("state.json")).is_ok_and(|t| t.contains(&id))
    });
    sandbox_flush(&mut client);
    let state = fs::read_to_string(sandbox.home().join("state.json")).expect("state");
    let list = client.call("session.list", Value::Null).to_string();
    for text in [&state, &list] {
        assert!(!text.contains(SECRET), "secret leaked: {text}");
        assert!(!text.contains("apiKeyHelper"), "settings leaked: {text}");
        assert!(text.contains("PermissionRequest"), "our hooks are recorded");
    }
}

/// The state file is written in the background: a second create round trip lets it catch up.
fn sandbox_flush(client: &mut Client) {
    std::thread::sleep(std::time::Duration::from_millis(700));
    client.call("session.list", Value::Null);
}

// G1
#[test]
fn claude_settings_with_a_hooks_key_or_a_non_claude_agent_are_rejected() {
    let sandbox_fakes = Fakes::new();
    sandbox_fakes.add("claude", "sleep 60");
    let mut sandbox = sandbox_fakes.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let mut p = settings_params();
    p["claudeSettings"]["hooks"] = json!({});
    let e = client.try_call("session.create", p).expect_err("hooks key");
    assert_eq!(e.code, -32602);
    assert!(e.message.contains("hooks"), "{}", e.message);
    assert!(!e.message.contains(SECRET));

    // Only account and model keys pass; nothing that could switch our hooks off, and no
    // variable that carries the session.
    for (what, settings) in [
        ("disableAllHooks", json!({"disableAllHooks": true})),
        ("unknown key", json!({"statusLine": {"command": "x"}})),
        ("variables key", json!({"env": {"ANTHROPIC_API_KEY": "x"}})),
    ] {
        let mut p = agent_params("claude");
        p["claudeSettings"] = settings;
        let e = client.try_call("session.create", p).expect_err(what);
        assert_eq!(e.code, -32602, "{what}");
    }

    let mut p = settings_params();
    p["agent"] = json!("codex");
    let e = client.try_call("session.create", p).expect_err("codex");
    assert_eq!(e.code, -32602);
    let list = client.call("session.list", Value::Null);
    assert_eq!(
        list.as_array().map(Vec::len),
        Some(0),
        "nothing was started"
    );
}

// G6
#[test]
fn the_same_idempotency_key_returns_the_same_session_even_after_a_restart() {
    let mut sandbox = Sandbox::new();
    let daemon = sandbox.start_daemon();
    let mut client = sandbox.client();
    let params = |cwd: &str, key: &str| {
        json!({"cmd": ["/bin/sh", "-c", "sleep 60"], "cwd": cwd, "cols": 80, "rows": 24,
               "idempotencyKey": key})
    };
    let first = client.call("session.create", params("/tmp", "k1"))["id"].clone();
    let again = client.call("session.create", params("/tmp", "k1"))["id"].clone();
    assert_eq!(first, again);
    // The key alone decides: other params under it still return the same session.
    assert_eq!(
        client.call("session.create", params("/var", "k1"))["id"],
        first
    );
    let other = client.call("session.create", params("/tmp", "k2"))["id"].clone();
    assert_ne!(first, other);
    let list = client.call("session.list", Value::Null);
    assert_eq!(list.as_array().map(Vec::len), Some(2), "{list}");
    assert!(
        !list.to_string().contains("k1"),
        "keys are not shown to clients"
    );

    // The key is persisted with the session.
    wait_until("the key in the state file", 15, || {
        fs::read_to_string(sandbox.home().join("state.json")).is_ok_and(|t| t.contains("\"k1\""))
    });
    kill(daemon, "-KILL");
    wait_until("daemon to die", 5, || !alive(daemon));
    sandbox.start_daemon();
    let mut client = sandbox.client();
    assert_eq!(
        client.call("session.create", params("/tmp", "k1"))["id"],
        first
    );
}

fn codex_files(dir: &Path) -> (String, String) {
    let read = |n: &str| fs::read_to_string(dir.join(n)).unwrap_or_default();
    (read("hooks.json"), read("config.toml"))
}

// G12
#[test]
fn an_account_codex_home_gets_our_hooks_and_trust_once() {
    let fakes = Fakes::new();
    fakes.add("codex", "sleep 60");
    let account = fakes.path("account-home");
    let mut sandbox = fakes.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let mut p = agent_params("codex");
    p["env"] = json!({"CODEX_HOME": account.display().to_string()});
    client.call("session.create", p.clone());
    let (hooks, config) = codex_files(&account);
    assert!(
        hooks.contains("hook session-start --agent codex"),
        "hooks.json: {hooks}"
    );
    let link = sandbox.home().join("bin/sushiai");
    assert!(hooks.contains(&link.display().to_string()), "{hooks}");
    assert!(config.contains("trusted_hash"), "config.toml: {config}");
    let before = fs::read_dir(&account).expect("dir").count();

    // A second launch changes nothing and leaves no backup behind.
    client.call("session.create", p);
    assert_eq!(codex_files(&account), (hooks, config));
    assert_eq!(fs::read_dir(&account).expect("dir").count(), before);
    // The default home (HOME/.codex) was never touched.
    assert!(!fakes.path(".codex").exists());
}

/// Codex resolves `$CODEX_HOME` (symlinks included) and keys its hook trust by the resolved
/// path, so trust written under the unresolved path is never found and Codex asks again.
#[test]
fn an_account_codex_home_behind_a_symlink_is_trusted_by_its_resolved_path() {
    let fakes = Fakes::new();
    fakes.add("codex", "sleep 60");
    let real = fakes.path("real-home");
    fs::create_dir(&real).expect("mkdir");
    let link = fakes.path("linked-home");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let mut sandbox = fakes.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let mut p = agent_params("codex");
    p["env"] = json!({"CODEX_HOME": link.display().to_string()});
    client.call("session.create", p);
    let (_, config) = codex_files(&real);
    let resolved = fs::canonicalize(&real).expect("canonicalize");
    let key = format!(
        "{}:session_start:0:0",
        resolved.join("hooks.json").display()
    );
    assert!(config.contains(&key), "config.toml: {config}");
    assert!(
        !config.contains(&link.display().to_string()),
        "config.toml: {config}"
    );
}

const HISTORY: &str =
    "i=1; while [ $i -le 100 ]; do echo histline-$i; i=$((i+1)); done; echo history-done; sleep 60";

fn history_session(sandbox: &Sandbox, client: &mut Client) -> String {
    let id = sandbox.create(client, HISTORY);
    wait_snapshot_contains(client, &id, "history-done");
    id
}

fn attach_text(client: &mut Client, id: &str, scrollback: Option<u32>) -> String {
    let mut p = json!({"id": id});
    if let Some(n) = scrollback {
        p["scrollback"] = json!(n);
    }
    let result = client.call("session.attach", p);
    String::from_utf8_lossy(&base64_decode(
        result["snapshot"].as_str().expect("snapshot"),
    ))
    .into_owned()
}

// G2
#[test]
fn attach_with_scrollback_puts_history_lines_before_the_screen() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = history_session(&sandbox, &mut client);

    let plain = attach_text(&mut client, &id, None);
    assert!(
        !plain.contains("histline-2\x1b"),
        "default is the screen only"
    );
    assert!(plain.contains("history-done"));

    let some = attach_text(&mut client, &id, Some(10));
    let full = attach_text(&mut client, &id, Some(500));
    assert!(some.len() > plain.len() && full.len() > some.len());
    let at = |text: &str, needle: &str| text.find(needle).unwrap_or_else(|| panic!("no {needle}"));
    // Oldest first, then the screen.
    assert!(at(&full, "histline-2\x1b") < at(&full, "histline-3\x1b"));
    assert!(at(&full, "histline-3\x1b") < at(&full, "histline-50\x1b"));
    assert!(at(&full, "histline-50\x1b") < at(&full, "history-done"));
    assert!(!some.contains("histline-2\x1b"), "only the newest 10 lines");
    assert!(
        full.ends_with(plain.as_str()),
        "history, then the unchanged snapshot"
    );
}

// G4
#[test]
fn read_returns_plain_text_with_optional_scrollback() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = history_session(&sandbox, &mut client);

    let screen = client.call("session.read", json!({"id": id}));
    assert_eq!(screen["rows"], 24);
    assert_eq!(screen["cols"], 80);
    let text = screen["text"].as_str().expect("text");
    assert!(text.contains("history-done") && text.contains("histline-100"));
    assert!(!text.contains('\x1b'), "plain text only");
    assert!(!text.contains("histline-2\n"), "default is the screen only");

    let all = client.call("session.read", json!({"id": id, "scrollback": 500}));
    let text = all["text"].as_str().expect("text");
    let lines: Vec<&str> = text.lines().collect();
    let first = lines
        .iter()
        .position(|l| *l == "histline-1")
        .expect("first");
    let last = lines
        .iter()
        .position(|l| *l == "histline-100")
        .expect("last");
    assert_eq!(last - first, 99, "every history line, in order, once");
    assert_eq!(lines.last().copied(), Some("history-done"));

    let e = client
        .try_call("session.read", json!({"id": "nope"}))
        .expect_err("unknown");
    assert_eq!(e.code, 1003);
}

// G5
#[test]
fn an_agent_open_reaches_normal_clients_as_session_open() {
    let fakes = Fakes::new();
    fakes.add(
        "claude",
        &format!("{BIN} open preview/files notes/a.md\nsleep 60"),
    );
    let mut sandbox = fakes.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = client.call("session.create", agent_params("claude"))["id"]
        .as_str()
        .expect("id")
        .to_string();
    let note = client.wait_note("session.open");
    assert_eq!(note["id"], id.as_str());
    assert_eq!(note["target"], "preview/files");
    // The holder's cwd is the real path of /tmp (a symlink on macOS).
    let arg = note["arg"].as_str().expect("arg");
    assert!(
        arg.starts_with('/') && arg.ends_with("/tmp/notes/a.md"),
        "{arg}"
    );
    assert!(note["nonce"].as_str().is_some_and(|n| n.len() >= 8));
}

// G5
#[test]
fn open_fails_open_without_a_session_and_the_daemon_checks_the_token() {
    let out = Command::new(BIN)
        .args(["open", "preview/files", "a.md"])
        .env_remove("SUSHIAI_SOCKET")
        .env_remove("SUSHIAI_SESSION_ID")
        .env_remove("SUSHIAI_SESSION_TOKEN")
        .output()
        .expect("run");
    assert!(out.status.success(), "must exit 0");
    assert!(!out.stderr.is_empty(), "and say why on stderr");

    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let params = json!({"session": "x", "token": "bad", "target": "t", "arg": "/a"});
    let e = client
        .try_call("hook.open", params.clone())
        .expect_err("not a hook connection");
    assert_eq!(e.code, 1007);
    let mut hook = Client::connect(&sandbox.socket());
    hook.call(
        "hello",
        json!({"protocol": 1, "client": "t", "role": "hook"}),
    );
    let e = hook.try_call("hook.open", params).expect_err("bad token");
    assert_eq!(e.code, 1007);
    // A round trip on the normal client files every notification sent before it.
    client.call("session.list", Value::Null);
    assert!(client.notes.iter().all(|n| n.0 != "session.open"));
}

// G6
#[test]
fn parallel_creates_with_one_key_start_one_session() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let socket = sandbox.socket();
    let start = std::sync::Barrier::new(8);
    let ids: Vec<Value> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    let mut client = Client::connect(&socket);
                    client.call("hello", json!({"protocol": 1, "client": "t"}));
                    start.wait();
                    client.call(
                        "session.create",
                        json!({"cmd": ["/bin/sh", "-c", "sleep 60"], "cwd": "/tmp",
                               "cols": 80, "rows": 24, "idempotencyKey": "same"}),
                    )["id"]
                        .clone()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().expect("worker"))
            .collect()
    });
    assert!(ids.iter().all(|id| *id == ids[0]), "{ids:?}");
    let mut client = sandbox.client();
    let list = client.call("session.list", Value::Null);
    assert_eq!(list.as_array().map(Vec::len), Some(1), "{list}");
}

// G12
#[test]
fn a_codex_home_that_is_relative_or_has_a_tilde_is_refused_before_any_write() {
    let fakes = Fakes::new();
    fakes.add("codex", "sleep 60");
    let mut sandbox = fakes.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let tilde = fakes.path("a~b");
    for home in ["relative-home".to_string(), tilde.display().to_string()] {
        let mut p = agent_params("codex");
        p["env"] = json!({"CODEX_HOME": home});
        let e = client.try_call("session.create", p).expect_err(&home);
        assert_eq!(e.code, -32602, "{home}");
    }
    assert!(!tilde.exists(), "nothing was created");
    // The daemon runs in this test's working directory.
    let cwd = std::env::current_dir().expect("cwd");
    assert!(!cwd.join("relative-home").exists());
    let list = client.call("session.list", Value::Null);
    assert_eq!(list.as_array().map(Vec::len), Some(0));
}

// G2
#[test]
fn the_attach_history_stays_within_its_byte_budget() {
    // Paced, so the daemon keeps up with the stream and its screen holds every line.
    let script = r#"
let n = 1;
function tick() {
  let out = '';
  for (let k = 0; k < 4 && n <= 1500; k++, n++) {
    out += 'L' + n + 'X';
    for (let i = 0; i < 380; i++) out += '\x1b[38;2;' + i + ';' + (n % 256) + ';' + ((i * 7) % 256) + 'm\x1b[48;2;' + (255 - i % 256) + ';' + (i * 3 % 256) + ';' + (n % 256) + 'mx';
    out += '\x1b[m\r\n';
  }
  if (n > 1500) out += 'budget-done\r\n';
  process.stdout.write(out);
  setTimeout(n > 1500 ? () => {} : tick, n > 1500 ? 60000 : 10);
}
tick();
"#;
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let created = client.call(
        "session.create",
        json!({"cmd": ["node", "-e", script], "cwd": "/tmp", "cols": 400, "rows": 24}),
    );
    let id = created["id"].as_str().expect("id").to_string();
    wait_snapshot_contains(&mut client, &id, "budget-done");
    let history = attach_text(&mut client, &id, Some(2000));
    let plain = attach_text(&mut client, &id, None);
    let mib = 1024 * 1024;
    assert!(
        history.len() < 4 * mib + plain.len() + mib / 4,
        "{}",
        history.len()
    );
    assert!(
        history.len() > 3 * mib,
        "most of the budget is used: {}",
        history.len()
    );
    assert!(history.contains("L1400X"), "the newest history stays");
    assert!(
        !history.contains("L1X"),
        "the oldest lines are dropped first"
    );
}

// G4
#[test]
fn read_joins_wrapped_history_rows() {
    let long = "w".repeat(200);
    let script = format!(
        "echo L{long}; i=1; while [ $i -le 60 ]; do echo fill-$i; i=$((i+1)); done; echo read-done; sleep 60"
    );
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = sandbox.create(&mut client, &script);
    wait_snapshot_contains(&mut client, &id, "read-done");
    let read = client.call("session.read", json!({"id": id, "scrollback": 500}));
    let text = read["text"].as_str().expect("text");
    assert!(text.contains(&format!("L{long}\n")), "one logical line");
}

// G5
#[test]
fn open_checks_target_and_path() {
    let fakes = Fakes::new();
    let token = fakes.path("token");
    fakes.add(
        "claude",
        &format!(
            "printf %s \"$SUSHIAI_SESSION_TOKEN\" > {}\nsleep 60",
            token.display()
        ),
    );
    let mut sandbox = fakes.sandbox();
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let id = client.call("session.create", agent_params("claude"))["id"]
        .as_str()
        .expect("id")
        .to_string();
    wait_until("the token", 15, || {
        fs::read_to_string(&token).is_ok_and(|t| !t.is_empty())
    });
    let token = fs::read_to_string(&token).expect("token");
    let mut hook = Client::connect(&sandbox.socket());
    hook.call(
        "hello",
        json!({"protocol": 1, "client": "t", "role": "hook"}),
    );
    let mut open = |target: &str, arg: &str| {
        hook.try_call(
            "hook.open",
            json!({"session": id, "token": token, "target": target, "arg": arg}),
        )
    };
    for target in [
        "Preview/files",
        "preview",
        "preview/files/x",
        "preview/",
        "pre view/a",
    ] {
        assert_eq!(
            open(target, "/a.md").expect_err(target).code,
            -32602,
            "{target}"
        );
    }
    for arg in [
        "relative.md",
        "",
        "/a\nb.md",
        "/a\u{7}.md",
        &format!("/{}", "a".repeat(4096)),
    ] {
        assert_eq!(
            open("preview/files", arg).expect_err(arg).code,
            -32602,
            "{arg:?}"
        );
    }
    open("preview/files", "/ok.md").expect("a valid request passes");
    client.call("session.list", Value::Null);
    let opened = client
        .notes
        .iter()
        .filter(|n| n.0 == "session.open")
        .count();
    assert_eq!(opened, 1, "refused requests sent no notification");
}
