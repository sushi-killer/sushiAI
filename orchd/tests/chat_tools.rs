//! Connected tools of the orchestrator chat, through a real daemon whose chat
//! harness is a fake and whose one connected server is a fake stdio MCP
//! server that logs every tool call.

mod common;
use common::*;
use serde_json::json;
use std::time::{Duration, Instant};

const FAKE_CLAUDE: &str = r##"#!/bin/sh
printf '%s\n' "$@" > "$ARGV_OUT"
while [ $# -gt 0 ]; do
  [ "$1" = "--mcp-config" ] && cp "$2" "$MCP_OUT"
  shift
done
p=$(cat)
printf '%s' "$p" > "$PROMPT_OUT"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-chat"}'
case "$p" in
*zzsend*)
  r='I will post it.\n```sushi-action\n{\"server\":\"fake\",\"tool\":\"send_note\",\"args\":{\"text\":\"hello\"},\"summary\":\"Post a note\",\"target\":\"#general\"}\n```' ;;
*zzread*)
  r='Trying a read.\n```sushi-action\n{\"server\":\"fake\",\"tool\":\"find_item\",\"args\":{}}\n```' ;;
*)
  r='Plain reply.' ;;
esac
printf '%s\n' '{"type":"result","session_id":"sess-chat","result":"'"$r"'"}'
"##;

const FAKE_MCP: &str = r##"import json, os, sys
TOOLS = [
    {"name": "get_item", "annotations": {"readOnlyHint": True}},
    {"name": "find_item"},
    {"name": "send_note", "annotations": {"readOnlyHint": False}},
    {"name": "post_quiet", "annotations": {"readOnlyHint": True}},
]
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    method = msg["method"]
    if method == "initialize":
        result = {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "fake", "version": "0"}}
    elif method == "tools/list":
        result = {"tools": TOOLS}
    elif method == "tools/call":
        with open(os.environ["CALLS_OUT"], "a") as log:
            log.write(json.dumps(msg["params"], sort_keys=True) + "\n")
        text = "done " + json.dumps(msg["params"]["arguments"], sort_keys=True)
        result = {"content": [{"type": "text", "text": text}]}
        if os.environ.get("FAIL"):
            result = {"isError": True, "content": [{"type": "text", "text": "channel is archived"}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)
"##;

struct Env {
    daemon: Daemon,
    _scripts: tempfile::TempDir,
    scripts: std::path::PathBuf,
    repo: tempfile::TempDir,
    home: tempfile::TempDir,
}

fn spawn() -> Env {
    let scripts = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(scripts.path(), "fake-claude-tools.sh", FAKE_CLAUDE);
    let mcp = scripts.path().join("fake_mcp.py");
    std::fs::write(&mcp, FAKE_MCP).unwrap();
    let home = tempfile::tempdir().unwrap();
    let calls = scripts.path().join("calls");
    std::fs::write(
        home.path().join(".claude.json"),
        json!({"mcpServers": {
            "fake": {"type": "stdio", "command": "python3", "args": [mcp],
                     "env": {"CALLS_OUT": calls}},
            "other": {"type": "http", "url": "https://example.invalid/mcp"},
        }})
        .to_string(),
    )
    .unwrap();
    let s = |name: &str| scripts.path().join(name).to_str().unwrap().to_string();
    let (argv, mcp_out, prompt) = (s("argv"), s("mcp-out"), s("prompt"));
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("HOME", home.path().to_str().unwrap()),
        ("ARGV_OUT", &argv),
        ("MCP_OUT", &mcp_out),
        ("PROMPT_OUT", &prompt),
    ]);
    Env {
        daemon,
        scripts: scripts.path().to_path_buf(),
        _scripts: scripts,
        repo: init_git_repo(),
        home,
    }
}

impl Env {
    fn path(&self) -> String {
        self.repo.path().to_str().unwrap().to_string()
    }

    fn set_tools(&self, tools: serde_json::Value) {
        let mut settings = self.daemon.request("settings.get", json!({}));
        settings["chatTools"] = tools;
        self.daemon
            .request("settings.set", json!({"settings": settings}));
    }

    fn fake_tool(&self, enabled: bool) -> serde_json::Value {
        json!({"id": "fake", "label": "Fake", "enabled": enabled,
               "server": {"ref": "claude-json:fake"}})
    }

    fn wait_idle(&self) -> serde_json::Value {
        let start = Instant::now();
        loop {
            let thread = self
                .daemon
                .request("chat.get", json!({"repo": self.path()}));
            if thread["busy"] == json!(false) {
                return thread;
            }
            assert!(start.elapsed() < Duration::from_secs(30), "never idle");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn say(&self, text: &str, mode: Option<&str>) -> serde_json::Value {
        let mut params = json!({"repo": self.path(), "text": text});
        if let Some(mode) = mode {
            params["mode"] = json!(mode);
        }
        self.daemon.request("chat.send", params);
        std::thread::sleep(Duration::from_millis(100));
        self.wait_idle()
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.scripts.join(name)).unwrap_or_default()
    }

    fn allowed(&self) -> String {
        let argv: Vec<String> = self.read("argv").lines().map(str::to_string).collect();
        argv[argv.iter().position(|a| a == "--allowedTools").unwrap() + 1].clone()
    }

    fn mcp_servers(&self) -> Vec<String> {
        let config: serde_json::Value = serde_json::from_str(&self.read("mcp-out")).unwrap();
        config["mcpServers"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    fn calls(&self) -> Vec<String> {
        self.read("calls").lines().map(str::to_string).collect()
    }

    fn action(&self) -> serde_json::Value {
        let thread = self
            .daemon
            .request("chat.get", json!({"repo": self.path()}));
        thread["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|m| m.get("action").is_some())
            .unwrap()
            .clone()
    }
}

#[test]
fn settings_carry_the_four_defaults_by_reference_and_offer_only_servers_not_added() {
    let env = spawn();
    let settings = env.daemon.request("settings.get", json!({}));
    let tools = settings["chatTools"].as_array().unwrap();
    let ids: Vec<&str> = tools.iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["google-sheets", "openviking", "slack", "asana"]);
    assert!(tools.iter().all(|t| t["enabled"] == json!(true)));
    let text = settings["chatTools"].to_string();
    assert!(text.contains("claude-json:google-sheets-main") && !text.contains("SECRET"));

    let offered = env.daemon.request("chat.toolServers", json!({}));
    let refs: Vec<&str> = offered["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["ref"].as_str().unwrap())
        .collect();
    assert_eq!(refs, ["claude-json:fake", "claude-json:other"]);

    let mut added = tools.clone();
    added.push(env.fake_tool(true));
    env.set_tools(json!(added));
    let offered = env.daemon.request("chat.toolServers", json!({}));
    assert_eq!(offered["servers"].as_array().unwrap().len(), 1);
    assert_eq!(offered["servers"][0]["ref"], "claude-json:other");

    let shown = env.daemon.request("chat.tools", json!({}));
    let row = shown["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "fake")
        .unwrap();
    assert_eq!(row["status"], "ok");
    let kinds: Vec<(String, String)> = row["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].to_string(), t["kind"].to_string()))
        .collect();
    assert!(kinds.contains(&("\"get_item\"".into(), "\"read\"".into())));
    assert!(kinds.contains(&("\"find_item\"".into(), "\"read\"".into())));
    assert!(kinds.contains(&("\"send_note\"".into(), "\"write\"".into())));
    let missing = shown["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "asana")
        .unwrap();
    // Enabled by default, so the probe runs; with no sign-in it is never "ok".
    assert_ne!(missing["status"], "ok");
    assert!(missing["reason"].as_str().is_some());
    let slack = shown["tools"][2].clone();
    assert_eq!(slack["status"], "unavailable");
    assert!(slack["reason"].as_str().unwrap().contains("not installed"));
    let _ = env.home;
}

#[test]
fn a_read_tool_joins_every_mode_a_write_tool_never_and_a_failing_server_is_skipped() {
    let env = spawn();
    env.set_tools(json!([env.fake_tool(true), {
        "id": "broken", "label": "Broken", "enabled": true,
        "server": {"command": "/nonexistent/mcp-server", "args": []}
    }]));
    for mode in [None, Some("brainstorm"), Some("plan")] {
        env.say("hello", mode);
        let allowed = env.allowed();
        assert!(allowed.contains("mcp__fake__get_item"), "{allowed}");
        assert!(allowed.contains("mcp__fake__find_item"), "{allowed}");
        assert!(allowed.contains("mcp__fake__post_quiet"), "{allowed}");
        assert!(!allowed.contains("send_note"), "{allowed}");
        assert!(!allowed.contains("broken"), "{allowed}");
        let servers = env.mcp_servers();
        assert!(servers.contains(&"fake".to_string()));
        assert!(servers.contains(&"sushiai-orchestrator".to_string()));
        assert!(!servers.contains(&"broken".to_string()));
    }
    let shown = env.daemon.request("chat.tools", json!({}));
    let broken = &shown["tools"][1];
    assert_eq!(broken["status"], "failed");
    assert!(broken["reason"]
        .as_str()
        .unwrap()
        .contains("could not start"));
    let prompt = env.read("argv");
    assert!(prompt.contains("sushi-action"));

    // Disabled, the tool is gone from the next turn.
    env.set_tools(json!([env.fake_tool(false)]));
    env.say("hello again", None);
    assert!(!env.allowed().contains("mcp__fake"));
    assert!(!env.mcp_servers().contains(&"fake".to_string()));

    // The owner can overrule the read/write guess.
    let mut tool = env.fake_tool(true);
    tool["overrides"] = json!({"find_item": "write", "send_note": "read"});
    env.set_tools(json!([tool]));
    env.say("once more", None);
    let allowed = env.allowed();
    assert!(allowed.contains("send_note") && !allowed.contains("find_item"));
}

#[test]
fn a_write_is_a_pending_action_that_runs_once_with_the_edited_args() {
    let env = spawn();
    env.set_tools(json!([env.fake_tool(true)]));
    let thread = env.say("zzsend now", None);
    let message = thread["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(message["text"], "I will post it.");
    let action = &message["action"];
    assert_eq!(action["state"], "pending");
    assert_eq!(action["tool"], "send_note");
    assert_eq!(action["target"], "#general");
    assert_eq!(action["summary"], "Post a note");
    assert!(env.calls().is_empty(), "nothing runs before the owner's OK");

    let id = message["id"].as_str().unwrap().to_string();
    env.daemon.request(
        "chat.actionSend",
        json!({"repo": env.path(), "messageId": id, "args": {"text": "edited"}}),
    );
    env.wait_idle();
    let sent = env.action();
    assert_eq!(sent["action"]["state"], "sent");
    assert_eq!(sent["action"]["args"], json!({"text": "edited"}));
    assert!(sent["action"]["result"]
        .as_str()
        .unwrap()
        .contains("edited"));
    assert_eq!(env.calls().len(), 1);
    assert!(env.calls()[0].contains("\"name\": \"send_note\""));
    assert!(env.calls()[0].contains("edited"));
    let thread = env.daemon.request("chat.get", json!({"repo": env.path()}));
    assert!(last_text(&thread).starts_with("Sent: Post a note."));

    // It cannot run twice, nor be declined afterwards.
    for method in ["chat.actionSend", "chat.actionDecline"] {
        let error = env
            .daemon
            .request_error(method, json!({"repo": env.path(), "messageId": id}));
        assert!(error.contains("already sent"), "{error}");
    }
    assert_eq!(env.calls().len(), 1);

    // The orchestrator hears how it ended, once.
    env.say("thanks", None);
    let prompt = env.read("prompt");
    assert!(prompt.contains("approved Post a note"), "{prompt}");
    env.say("and?", None);
    assert!(!env.read("prompt").contains("approved Post a note"));
}

fn last_text(thread: &serde_json::Value) -> String {
    thread["messages"].as_array().unwrap().last().unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn declining_records_the_refusal_and_tells_the_model_on_its_next_turn() {
    let env = spawn();
    env.set_tools(json!([env.fake_tool(true)]));
    let thread = env.say("zzsend now", None);
    let id = thread["messages"].as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    env.daemon.request(
        "chat.actionDecline",
        json!({"repo": env.path(), "messageId": id}),
    );
    assert_eq!(env.action()["action"]["state"], "declined");
    let error = env.daemon.request_error(
        "chat.actionSend",
        json!({"repo": env.path(), "messageId": id}),
    );
    assert!(error.contains("already declined"), "{error}");
    assert!(env.calls().is_empty());
    env.say("ok then", None);
    assert!(env.read("prompt").contains("declined Post a note"));
}

#[test]
fn a_reply_that_proposes_a_read_tool_or_an_unknown_service_is_not_an_action() {
    let env = spawn();
    env.set_tools(json!([env.fake_tool(true)]));
    let thread = env.say("zzread now", None);
    assert!(thread["messages"][1].get("action").is_none());
    // The fenced block stays in the text the owner sees.
    assert!(thread["messages"][1]["text"]
        .as_str()
        .unwrap()
        .contains("sushi-action"));
}

#[test]
fn a_failing_call_is_recorded_with_its_error_and_never_retried() {
    let env = spawn();
    let mut tool = env.fake_tool(true);
    let mut server: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(env.home.path().join(".claude.json")).unwrap(),
    )
    .unwrap();
    server = server["mcpServers"]["fake"].clone();
    server["env"]["FAIL"] = json!("1");
    tool["server"] = server;
    env.set_tools(json!([tool]));
    let thread = env.say("zzsend now", None);
    let id = thread["messages"].as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    env.daemon.request(
        "chat.actionSend",
        json!({"repo": env.path(), "messageId": id}),
    );
    env.wait_idle();
    let failed = env.action();
    assert_eq!(failed["action"]["state"], "failed");
    assert_eq!(failed["action"]["error"], "channel is archived");
    let error = env.daemon.request_error(
        "chat.actionSend",
        json!({"repo": env.path(), "messageId": id}),
    );
    assert!(error.contains("already failed"), "{error}");
    assert_eq!(env.calls().len(), 1);
    env.say("so?", None);
    assert!(env.read("prompt").contains("channel is archived"));
}

#[test]
fn a_server_that_cannot_be_reached_offers_no_actions() {
    let env = spawn();
    let mut tool = env.fake_tool(true);
    tool["server"] = json!({"command": "/nonexistent/mcp-server"});
    env.set_tools(json!([tool]));
    let thread = env.say("zzsend now", None);
    assert!(thread["messages"][1].get("action").is_none());
}
