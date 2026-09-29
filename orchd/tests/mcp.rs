//! Black-box integration test for `orchd mcp`: spawns a real `orchd serve`
//! daemon, then a real `orchd mcp` process talking newline-delimited
//! JSON-RPC 2.0 over its own stdin/stdout, exactly like an MCP-capable
//! agent harness would attach it. Minimal spawn/tempdir pattern copied from
//! `tests/integration.rs`; kept separate rather than shared because each
//! file under `tests/` is its own crate.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

/// macOS caps a unix-domain-socket path around 104 bytes; `tempfile`'s own
/// tempdir already nests several segments deep (see
/// `electron/orchestrator.cjs`'s `socketPathFor`), so the socket lives
/// directly under `$TMPDIR` instead of inside the per-test tempdir.
fn short_socket_path(label: &str) -> PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(base).join(format!(
        "orchd-mcp-test-{label}-{}.sock",
        std::process::id()
    ))
}

fn wait_for_socket(socket: &Path) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if socket.exists() && UnixStream::connect(socket).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("orchd did not open its socket in time");
}

fn read_control_token(data_dir: &Path) {
    let start = Instant::now();
    let path = data_dir.join("control.token");
    loop {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if !text.trim().is_empty() {
                return;
            }
        }
        if start.elapsed() > Duration::from_secs(10) {
            panic!("orchd did not write control.token in time");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct Daemon {
    child: Child,
    socket: PathBuf,
    data_dir: tempfile::TempDir,
}

/// A test that panics before finishing must not leave the daemon running.
impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

impl Daemon {
    fn spawn(label: &str) -> Daemon {
        let data_dir = tempfile::tempdir().unwrap();
        let socket = short_socket_path(label);
        let bin = env!("CARGO_BIN_EXE_orchd");
        let child = Command::new(bin)
            .args([
                "serve",
                "--data",
                data_dir.path().to_str().unwrap(),
                "--socket",
                socket.to_str().unwrap(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn orchd serve");
        wait_for_socket(&socket);
        read_control_token(data_dir.path());
        Daemon {
            child,
            socket,
            data_dir,
        }
    }

    fn data_dir(&self) -> &Path {
        self.data_dir.path()
    }
}

struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// A test that panics before finishing must not leave the mcp process
/// running either.
impl Drop for McpClient {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl McpClient {
    fn spawn(data_dir: &Path, socket: &Path) -> McpClient {
        McpClient::spawn_with(data_dir, socket, &[])
    }

    fn spawn_with(data_dir: &Path, socket: &Path, extra: &[&str]) -> McpClient {
        let bin = env!("CARGO_BIN_EXE_orchd");
        let mut child = Command::new(bin)
            .args([
                "mcp",
                "--data",
                data_dir.to_str().unwrap(),
                "--socket",
                socket.to_str().unwrap(),
            ])
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn orchd mcp");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        McpClient {
            child,
            stdin,
            stdout,
        }
    }

    fn request(&mut self, id: i64, method: &str, params: serde_json::Value) -> serde_json::Value {
        let req =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut line = req.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .expect("write mcp request");
        self.stdin.flush().expect("flush mcp request");

        let mut response_line = String::new();
        self.stdout
            .read_line(&mut response_line)
            .expect("read mcp response line");
        assert!(
            !response_line.trim().is_empty(),
            "orchd mcp closed stdout before responding"
        );
        serde_json::from_str(response_line.trim()).expect("valid JSON-RPC response")
    }
}

#[test]
fn initialize_tools_list_and_tools_call_over_stdio() {
    let daemon = Daemon::spawn("daemon");
    let mut mcp = McpClient::spawn(daemon.data_dir(), &daemon.socket);

    let init = mcp.request(
        1,
        "initialize",
        serde_json::json!({"protocolVersion": "2025-06-18"}),
    );
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "sushiai-orchestrator");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(init["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("task_preflight"));

    let list = mcp.request(2, "tools/list", serde_json::json!({}));
    let tools = list["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for expected in [
        "task_list",
        "task_get",
        "task_create",
        "task_start",
        "task_stop",
        "task_answer",
        "task_preflight",
        "settings_get",
        "task_archive",
        "task_unarchive",
        "peer_list",
        "peer_send",
        "inbox_read",
        "ask_orchestrator",
        "orchestrator_reply",
        "repo_audit",
        "task_amend",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool {expected}: {names:?}"
        );
    }
    assert_eq!(names.len(), 17, "unexpected extra tools: {names:?}");
    for tool in tools {
        assert!(
            !tool["inputSchema"]["properties"].is_null(),
            "{tool} has no inputSchema"
        );
    }
    assert!(
        !names.contains(&"task_delete"),
        "task_delete must never be exposed: {names:?}"
    );

    let settings = mcp.request(
        3,
        "tools/call",
        serde_json::json!({"name": "settings_get", "arguments": {}}),
    );
    assert!(
        settings["result"].get("isError").is_none(),
        "settings_get should succeed: {settings}"
    );
    assert!(!settings["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .is_empty());

    let listed = mcp.request(
        4,
        "tools/call",
        serde_json::json!({"name": "task_list", "arguments": {}}),
    );
    assert!(
        listed["result"].get("isError").is_none(),
        "task_list should succeed on an empty store: {listed}"
    );

    let bogus = mcp.request(
        5,
        "tools/call",
        serde_json::json!({"name": "task_get", "arguments": {"id": "does-not-exist"}}),
    );
    assert_eq!(
        bogus["result"]["isError"], true,
        "a bogus task id should come back as a tool error: {bogus}"
    );

    let unknown = mcp.request(
        6,
        "tools/call",
        serde_json::json!({"name": "peer_send", "arguments": {"to": "nobody", "text": "hi"}}),
    );
    assert_eq!(unknown["result"]["isError"], true, "{unknown}");
    let to_self = mcp.request(
        7,
        "tools/call",
        serde_json::json!({"name": "ask_orchestrator", "arguments": {"text": "hi"}}),
    );
    assert_eq!(to_self["result"]["isError"], true, "{to_self}");
    assert!(!daemon.data_dir().join("messages.json").exists());
}

#[test]
fn a_task_scoped_bridge_offers_only_the_messaging_tools() {
    let daemon = Daemon::spawn("task");
    let task = "11111111-1111-4111-8111-111111111111";
    let mut mcp = McpClient::spawn_with(daemon.data_dir(), &daemon.socket, &["--task", task]);

    let init = mcp.request(1, "initialize", serde_json::json!({}));
    assert_eq!(init["result"]["serverInfo"]["name"], "sushiai-messages");
    let list = mcp.request(2, "tools/list", serde_json::json!({}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names.join(","),
        "peer_list,peer_send,inbox_read,ask_orchestrator"
    );

    let create = mcp.request(
        3,
        "tools/call",
        serde_json::json!({"name": "task_create", "arguments": {"repo": "/"}}),
    );
    assert_eq!(create["result"]["isError"], true, "{create}");
    // The task doesn't exist in this daemon, so it can't send as it either.
    let ask = mcp.request(
        4,
        "tools/call",
        serde_json::json!({"name": "ask_orchestrator", "arguments": {"text": "hi"}}),
    );
    assert_eq!(ask["result"]["isError"], true, "{ask}");
}
