//! `sushiai mcp`, `sushiai orch hook` and `sushiai orch register` in the real binary.
//!
//! The mcp tests run against a real daemon with the orchestration module enabled. The hook
//! test needs `orch.hook.stop` answered with a chosen reply, so a fake daemon in the test
//! plays the socket there.

mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use serde_json::{json, Value};
use sushiai_protocol::{encode, Decoder, Frame, Message};

/// A socket at `<home>/daemon.sock` that answers `hello` and the methods in `answers`;
/// every request is recorded as `(method, params)`.
struct FakeDaemon {
    seen: Arc<Mutex<Vec<(String, Value)>>>,
}

impl FakeDaemon {
    fn start(sandbox: &Sandbox, answers: Vec<(&'static str, Value)>) -> FakeDaemon {
        let listener = UnixListener::bind(sandbox.socket()).expect("bind");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let answers = answers.clone();
                let log = log.clone();
                std::thread::spawn(move || {
                    let mut decoder = Decoder::new();
                    let mut buf = [0u8; 8192];
                    loop {
                        let n = match stream.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => n,
                        };
                        for frame in decoder.push(&buf[..n]).expect("decode") {
                            let Frame::Json(text) = frame else { continue };
                            let Ok(Message::Request(r)) = Message::parse(&text) else {
                                continue;
                            };
                            log.lock()
                                .unwrap()
                                .push((r.method.clone(), r.params.clone()));
                            let result = if r.method == "hello" {
                                json!({"protocol": 1, "capabilities": ["orch"], "daemon": "fake"})
                            } else {
                                answers
                                    .iter()
                                    .find(|(m, _)| *m == r.method)
                                    .map(|(_, v)| v.clone())
                                    .unwrap_or(Value::Null)
                            };
                            let reply = json!({"jsonrpc": "2.0", "id": r.id, "result": result});
                            let _ = stream.write_all(&encode(&Frame::Json(reply.to_string())));
                        }
                    }
                });
            }
        });
        FakeDaemon { seen }
    }

    fn requests(&self, method: &str) -> Vec<Value> {
        let seen = self.seen.lock().unwrap();
        seen.iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }
}

/// A running `sushiai mcp`: one request line in, one response line out.
struct Mcp {
    child: Child,
    lines: Receiver<String>,
    next_id: u64,
}

impl Mcp {
    fn start(sandbox: &Sandbox, args: &[&str]) -> Mcp {
        let mut child = Command::new(BIN)
            .arg("mcp")
            .args(args)
            .env("SUSHIAI_HOME", sandbox.home())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn mcp");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Mcp {
            child,
            lines,
            next_id: 1,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let stdin = self.child.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{line}").expect("write");
        let text = self
            .lines
            .recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|_| panic!("no answer to {method}"));
        let answer: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(answer["id"], id);
        answer
    }

    fn tool_names(&mut self) -> Vec<String> {
        let list = self.request("tools/list", json!({}));
        list["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|t| t["name"].as_str().expect("name").to_string())
            .collect()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A one-commit git repo with synthetic content, for `orch.task.create`.
fn repo(dir: &std::path::Path) -> String {
    let path = dir.join("repo");
    std::fs::create_dir_all(&path).expect("repo dir");
    std::fs::write(path.join("a.txt"), "one\n").expect("file");
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    ] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(&path)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }
    path.to_string_lossy().into_owned()
}

#[test]
fn mcp_against_the_real_daemon_lists_the_tools_and_returns_the_modules_tasks() {
    let mut sandbox = Sandbox::new();
    let modules = sandbox.home().join("modules");
    std::fs::create_dir_all(&modules).expect("modules dir");
    std::fs::write(modules.join("orch.enabled"), b"").expect("flag");
    sandbox.start_daemon();
    let mut client = sandbox.client();
    let task = client.call(
        "orch.task.create",
        json!({"repo": repo(sandbox.home()), "title": "Add a line", "goal": "Add a line",
               "criteria": ["a.txt has two lines"], "verify": ["true"]}),
    );
    let id = task["id"].as_str().expect("task id").to_string();

    let mut full = Mcp::start(&sandbox, &[]);
    let init = full.request("initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["result"]["serverInfo"]["name"], "sushiai-orchestrator");
    assert_eq!(full.tool_names().len(), 23);
    let out = full.request("tools/call", json!({"name": "task_list", "arguments": {}}));
    assert!(out["result"].get("isError").is_none(), "{out}");
    let text = out["result"]["content"][0]["text"].as_str().expect("text");
    let tasks: Value = serde_json::from_str(text).expect("tasks");
    assert_eq!(tasks[0]["id"], id.as_str(), "{tasks}");

    let mut task = Mcp::start(&sandbox, &["--task", "t1"]);
    assert_eq!(
        task.tool_names(),
        ["peer_list", "peer_send", "inbox_read", "ask_orchestrator"]
    );
    let mut read_only = Mcp::start(&sandbox, &["--read-only"]);
    assert_eq!(read_only.tool_names().len(), 4);
}

#[test]
fn mcp_against_a_daemon_without_the_module_returns_a_tool_error() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut mcp = Mcp::start(&sandbox, &[]);
    let out = mcp.request("tools/call", json!({"name": "task_list", "arguments": {}}));
    assert_eq!(out["result"]["isError"], true, "{out}");
    let text = out["result"]["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("orch.task.list"), "{text}");
}

#[test]
fn mcp_without_a_daemon_still_answers_initialize_and_tools_list() {
    let sandbox = Sandbox::new();
    let mut mcp = Mcp::start(&sandbox, &[]);
    mcp.request("initialize", json!({}));
    assert_eq!(mcp.tool_names().len(), 23);
}

fn hook_command(sandbox: &Sandbox, event: &str, token: Option<&str>) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(["orch", "hook", event])
        .env("SUSHIAI_HOME", sandbox.home())
        .env_remove("SUSHIAI_ORCH_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(token) = token {
        command.env("SUSHIAI_ORCH_TOKEN", token);
    }
    command
}

fn run_hook(mut command: Command, input: &str) -> (i32, String) {
    let mut child = command.spawn().expect("spawn hook");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    (
        out.status.code().expect("code"),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

#[test]
fn a_hook_with_no_daemon_prints_an_empty_object_and_exits_zero() {
    let sandbox = Sandbox::new();
    for event in ["stop", "edit", "bogus"] {
        let (code, out) = run_hook(hook_command(&sandbox, event, Some("tok")), "{\"a\":1}");
        assert_eq!((code, out.as_str()), (0, "{}"), "{event}");
    }
    // No token: also `{}`, and no daemon is started.
    let (code, out) = run_hook(hook_command(&sandbox, "stop", None), "{}");
    assert_eq!((code, out.as_str()), (0, "{}"));
    assert!(!sandbox.socket().exists());
}

#[test]
fn a_hook_forwards_the_payload_with_its_token_and_prints_the_answer() {
    let sandbox = Sandbox::new();
    let fake = FakeDaemon::start(
        &sandbox,
        vec![(
            "orch.hook.stop",
            json!({"decision": "block", "reason": "run the tests"}),
        )],
    );
    let (code, out) = run_hook(
        hook_command(&sandbox, "stop", Some("tok-1")),
        "{\"cwd\":\"/w\"}",
    );
    assert_eq!(code, 0);
    let answer: Value = serde_json::from_str(&out).expect("json");
    assert_eq!(answer["decision"], "block");
    assert_eq!(
        fake.requests("orch.hook.stop"),
        [json!({"token": "tok-1", "payload": {"cwd": "/w"}})]
    );
}

fn register(home: &std::path::Path, codex: &std::path::Path, action: &str) -> String {
    let out = Command::new(BIN)
        .args(["orch", action])
        .env("HOME", home)
        .env("CODEX_HOME", codex)
        .env("SUSHIAI_HOME", home.join(".sushiai"))
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn backups(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir)
        .expect("dir")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains("sushiai-bak"))
        .count()
}

#[test]
fn register_is_idempotent_and_keeps_other_mcp_entries() {
    let tmp = tempfile::Builder::new()
        .prefix("sd")
        .tempdir_in("/tmp")
        .expect("tempdir");
    let home = tmp.path().join("home");
    let codex = home.join(".codex");
    std::fs::create_dir_all(&codex).expect("dirs");
    std::fs::create_dir_all(home.join(".claude")).expect("dirs");
    let claude_file = home.join(".claude.json");
    std::fs::write(
        &claude_file,
        r#"{"theme":"dark","mcpServers":{"other":{"type":"stdio","command":"/bin/other","args":[]}}}"#,
    )
    .expect("write");
    let codex_file = codex.join("config.toml");
    std::fs::write(
        &codex_file,
        "model = \"m\"\n\n[mcp_servers.other]\ncommand = \"/bin/other\"\n",
    )
    .expect("write");

    register(&home, &codex, "register");
    let after_first: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_file).unwrap()).unwrap();
    assert_eq!(after_first["theme"], "dark");
    assert_eq!(after_first["mcpServers"]["other"]["command"], "/bin/other");
    let ours = &after_first["mcpServers"]["sushiai-orchestrator"];
    assert_eq!(ours["args"], json!(["mcp"]));
    assert!(ours["command"]
        .as_str()
        .unwrap()
        .ends_with(".sushiai/bin/sushiai"));
    let toml_first = std::fs::read_to_string(&codex_file).unwrap();
    assert!(toml_first.contains("[mcp_servers.other]"));
    assert!(toml_first.contains("[mcp_servers.sushiai-orchestrator]"));
    assert!(toml_first.contains("model = \"m\""));
    assert!(home
        .join(".claude/skills/sushiai-orchestrator/SKILL.md")
        .exists());
    let (claude_backups, codex_backups) = (backups(&home), backups(&codex));
    assert_eq!((claude_backups, codex_backups), (1, 1));

    let second = register(&home, &codex, "register");
    assert!(second.contains("no change in"), "{second}");
    assert_eq!(std::fs::read_to_string(&codex_file).unwrap(), toml_first);
    let after_second: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_file).unwrap()).unwrap();
    assert_eq!(after_second, after_first);
    assert_eq!(
        (backups(&home), backups(&codex)),
        (claude_backups, codex_backups)
    );

    register(&home, &codex, "unregister");
    let gone: Value =
        serde_json::from_str(&std::fs::read_to_string(&claude_file).unwrap()).unwrap();
    assert!(gone["mcpServers"].get("sushiai-orchestrator").is_none());
    assert_eq!(gone["mcpServers"]["other"]["command"], "/bin/other");
    let toml_gone = std::fs::read_to_string(&codex_file).unwrap();
    assert!(!toml_gone.contains("sushiai-orchestrator"));
    assert!(toml_gone.contains("[mcp_servers.other]"));
    assert!(!home.join(".claude/skills/sushiai-orchestrator").exists());
}
