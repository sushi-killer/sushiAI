//! `orchd mcp --data <dir> [--socket <path>]`: a stdio MCP (Model Context
//! Protocol) server, so any MCP-capable agent harness (Claude Code, Codex,
//! ...) can attach orchd and act as the owner's task orchestrator. This is
//! purely a bridge: it speaks newline-delimited JSON-RPC 2.0 on
//! stdin/stdout and translates `tools/call` into a request against the same
//! control-token-gated unix socket `protocol::serve` already listens on. It
//! never touches engine/store internals directly -- the daemon started by
//! `orchd serve` must already be running. Deliberately synchronous (plain
//! `std::io` / `std::os::unix::net`, no tokio): one line in, one orchd
//! round trip, one line out, nothing else runs concurrently in this
//! process.

use crate::model::ORCHESTRATOR;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// Protocol version handed back when the client didn't ask for a specific
/// one. When it did ask, we just echo it back rather than maintaining a
/// compatibility matrix here -- this bridge has no version-specific
/// behavior of its own to gate on.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// The orchestrator-agent role handed to the client in `initialize`'s
/// `instructions` field -- this IS the system prompt for whatever harness
/// attaches orchd as an MCP server.
const INSTRUCTIONS: &str = "\
You are the owner's task orchestrator. Turn a request into tasks tracked by \
orchd -- don't write the code yourself.

- Before creating a task, call task_preflight with the goal/criteria/verify \
you have. If clarity is low, ask the owner one precise question at a time \
in chat before creating anything.
- Prefer task_create with {repo, request, start: true}: the planner drafts \
title/goal/criteria/verify for you. Use the full {repo, title, goal, \
criteria, verify} form only when the owner already specified it.
- A task branches from the repo's current HEAD; pass `base` to start it \
from another branch (e.g. the feature branch the owner is working on).
- To speed up one feature, split it into 2-3 tasks that touch different \
files, give them the same `base`, and start them together; merge each \
into the base once it is done. Keep parts that edit the same file in one \
task.
- Use task_list / task_get to check on progress instead of guessing.
- When a task is `waiting`, answer it yourself with task_answer if the repo \
or the task's own context already answers the question. Otherwise bring \
the owner one precise question with concrete options.
- Never answer \"approve\" for a protected-path decision or for committing \
an attempt the review gave no verdict on, and never call task_stop, on the \
owner's behalf -- these are the owner's call to make.
- Never delete tasks; this tool intentionally cannot.
- Tasks ask you questions without stopping (ask_orchestrator). One reaches \
you marked as a task's question with its message id: answer it with \
orchestrator_reply {question, text}. inbox_read shows everything sent to you.
- peer_send {to, text} messages a task and peer_list {repo} lists a \
repository's tasks. A task reads a message on its next attempt; the attempt \
it is running is never interrupted.
";

/// The role handed to a task agent attached through `orchd mcp --task`.
const TASK_INSTRUCTIONS: &str = "\
You are one of several task agents working in the same repository, \
coordinated by orchd. peer_list shows the other tasks, peer_send messages \
one of them, inbox_read shows what was sent to you, and ask_orchestrator \
asks the owner's orchestrator a question without stopping your work. \
Replies and messages reach you with your next attempt's brief, so keep \
working meanwhile.
";

/// What a task agent gets (`--task`): only messaging, always as itself.
pub const TASK_TOOLS: [&str; 4] = [
    // In `tool_specs` order, so a task's tools/list reads the same way.
    "peer_list",
    "peer_send",
    "inbox_read",
    "ask_orchestrator",
];

/// What the orchestrator agent gets: every tool below.
pub const ORCHESTRATOR_TOOLS: [&str; 15] = [
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
];

fn from_schema() -> Value {
    json!({
        "type": "string",
        "description": "Sender: a task id, or \"orchestrator\" (the default).",
    })
}

/// name, orchd method, description, JSON Schema for `inputSchema`. No
/// `task_delete` (destructive, and never the agent's call), no
/// `settings_set` (would let an agent silently change parallelism/planner
/// config the owner didn't ask to change), no `secrets_*` (an agent has no
/// business reading or writing API keys) -- all three are deliberately left
/// off this surface, not just forgotten.
fn tool_specs() -> Vec<(&'static str, &'static str, &'static str, Value)> {
    vec![
        (
            "task_list",
            "task.list",
            "List orchd tasks, optionally filtered by repo.",
            json!({
                "type": "object",
                "properties": {
                    "repo": {"type": "string", "description": "Absolute path; only list tasks for this repo."},
                    "includeArchived": {"type": "boolean", "description": "Include archived tasks; omitted or false hides them."},
                },
            }),
        ),
        (
            "task_get",
            "task.get",
            "Get one task by id.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "task_create",
            "task.create",
            "Create a task. Prefer {repo, request, start: true} and let the planner draft title/goal/criteria/verify; use the full {repo, title, goal, criteria, verify} form only when the owner already specified it.",
            json!({
                "type": "object",
                "properties": {
                    "repo": {"type": "string"},
                    "request": {"type": "string", "description": "One-sentence ask; drafted by the planner."},
                    "title": {"type": "string"},
                    "goal": {"type": "string"},
                    "criteria": {"type": "array", "items": {"type": "string"}},
                    "verify": {"type": "array", "items": {"type": "string"}},
                    "branch": {"type": "string"},
                    "base": {"type": "string", "description": "Branch or commit to start from; defaults to the repo's current HEAD."},
                    "start": {"type": "boolean"},
                },
                "required": ["repo"],
            }),
        ),
        (
            "task_start",
            "task.start",
            "Start (or resume) a queued, stopped, failed or drafting task.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "task_stop",
            "task.stop",
            "Stop a running or waiting task.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "task_answer",
            "task.answer",
            "Answer a task's pending question (send \"stop\" as the answer to cancel it instead).",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}, "answer": {"type": "string"}},
                "required": ["id", "answer"],
            }),
        ),
        (
            "task_preflight",
            "task.preflight",
            "Check whether a goal/criteria/verify are specific enough to act on before creating a task.",
            json!({
                "type": "object",
                "properties": {
                    "goal": {"type": "string"},
                    "criteria": {"type": "array", "items": {"type": "string"}},
                    "verify": {"type": "array", "items": {"type": "string"}},
                },
            }),
        ),
        (
            "settings_get",
            "settings.get",
            "Read the orchd settings (parallelism, planner, classifier, profiles).",
            json!({"type": "object", "properties": {}}),
        ),
        (
            "task_archive",
            "task.archive",
            "Archive a task, hiding it from the default task_list without deleting it. Refused for a running, drafting, or waiting task.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "task_unarchive",
            "task.unarchive",
            "Restore an archived task so it appears in the default task_list again.",
            json!({
                "type": "object",
                "properties": {"id": {"type": "string"}},
                "required": ["id"],
            }),
        ),
        (
            "peer_list",
            "peer.list",
            "List the other tasks in a repository (id, title, status) you can message.",
            json!({
                "type": "object",
                "properties": {
                    "from": from_schema(),
                    "repo": {"type": "string", "description": "Absolute path; needed only when sending as the orchestrator."},
                },
            }),
        ),
        (
            "peer_send",
            "message.send",
            "Message another task by id. It reads the message on its next attempt; the attempt it is running is not interrupted.",
            json!({
                "type": "object",
                "properties": {
                    "from": from_schema(),
                    "to": {"type": "string", "description": "The recipient task's id."},
                    "text": {"type": "string"},
                    "replyTo": {"type": "string", "description": "Id of a message sent to you that this answers."},
                },
                "required": ["to", "text"],
            }),
        ),
        (
            "inbox_read",
            "message.inbox",
            "Read every message sent to you, delivered or still waiting for your next turn.",
            json!({"type": "object", "properties": {"from": from_schema()}}),
        ),
        (
            "ask_orchestrator",
            "message.send",
            "Ask the orchestrator a question without stopping your work. Its reply arrives with your next attempt.",
            json!({
                "type": "object",
                "properties": {"from": from_schema(), "text": {"type": "string"}},
                "required": ["text"],
            }),
        ),
        (
            "orchestrator_reply",
            "message.send",
            "Answer a question a task asked you; the task reads it on its next attempt.",
            json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string", "description": "The question's message id."},
                    "text": {"type": "string"},
                },
                "required": ["question", "text"],
            }),
        ),
    ]
}

/// The bridge's scope: the whole orchestrator surface, or one task's
/// messaging only (`--task <id>`), where every message is sent as that task.
struct Bridge {
    socket: PathBuf,
    token: String,
    task: Option<String>,
}

impl Bridge {
    fn offers(&self, tool_name: &str) -> bool {
        self.task.is_none() || TASK_TOOLS.contains(&tool_name)
    }
}

fn orchd_method_for(bridge: &Bridge, tool_name: &str) -> Option<&'static str> {
    tool_specs()
        .into_iter()
        .find(|(name, ..)| *name == tool_name && bridge.offers(name))
        .map(|(_, method, ..)| method)
}

fn tools_list_result(bridge: &Bridge) -> Value {
    let tools: Vec<Value> = tool_specs()
        .into_iter()
        .filter(|(name, ..)| bridge.offers(name))
        .map(|(name, _, description, mut input_schema)| {
            if bridge.task.is_some() {
                if let Some(props) = input_schema["properties"].as_object_mut() {
                    props.remove("from");
                }
            }
            json!({"name": name, "description": description, "inputSchema": input_schema})
        })
        .collect();
    json!({"tools": tools})
}

/// Turns a messaging tool's arguments into its orchd params: the sender is
/// the bridge's task when it has one (an agent can't speak as another), else
/// `from`, else the orchestrator.
fn message_params(tool: &str, mut args: Value, task: Option<&str>) -> Value {
    let Some(obj) = args.as_object_mut() else {
        return args;
    };
    let sender = task
        .or_else(|| obj.get("from").and_then(|v| v.as_str()))
        .unwrap_or(ORCHESTRATOR)
        .to_string();
    match tool {
        "peer_list" | "peer_send" => {
            obj.insert("from".to_string(), json!(sender));
        }
        "ask_orchestrator" => {
            obj.insert("from".to_string(), json!(sender));
            obj.insert("to".to_string(), json!(ORCHESTRATOR));
        }
        "inbox_read" => return json!({"id": sender}),
        "orchestrator_reply" => {
            return json!({
                "from": ORCHESTRATOR,
                "replyTo": obj.get("question").cloned().unwrap_or(Value::Null),
                "text": obj.get("text").cloned().unwrap_or(Value::Null),
            })
        }
        _ => {}
    }
    args
}

/// A parsed incoming JSON-RPC line. `id` stays a `Value` (JSON-RPC allows a
/// string, number, or null) so it can be echoed back verbatim; its absence
/// is what marks a notification.
#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

fn handle_initialize(bridge: &Bridge, params: &Value) -> Value {
    let protocol_version = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or(PROTOCOL_VERSION);
    let (name, instructions) = match bridge.task {
        Some(_) => ("sushiai-messages", TASK_INSTRUCTIONS),
        None => ("sushiai-orchestrator", INSTRUCTIONS),
    };
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": name, "version": env!("CARGO_PKG_VERSION")},
        "instructions": instructions,
    })
}

fn tool_ok(result: Value) -> Value {
    let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
    json!({"content": [{"type": "text", "text": text}]})
}

fn tool_error(message: String) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}

fn handle_tools_call(bridge: &Bridge, params: &Value) -> Value {
    let p: ToolCallParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return tool_error(format!("invalid tools/call params: {e}")),
    };
    let Some(method) = orchd_method_for(bridge, &p.name) else {
        return tool_error(format!("unknown tool: {}", p.name));
    };
    let args = if p.arguments.is_null() {
        json!({})
    } else {
        p.arguments
    };
    let task_mcp = std::env::var("ORCHD_TASK_MCP")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok());
    let args = with_task_mcp(method, args, task_mcp);
    let args = message_params(&p.name, args, bridge.task.as_deref());
    match call_orchd(&bridge.socket, &bridge.token, method, args) {
        Ok(result) => tool_ok(result),
        Err(message) => tool_error(message),
    }
}

/// A task the agent creates gets the MCP servers the app resolved for its
/// project (`ORCHD_TASK_MCP` names a `{repo, mcp}` file the app writes), the
/// same thing the app adds when the owner creates a task directly. Only for
/// that repo, and never over an `mcp` the caller set itself.
fn with_task_mcp(method: &str, mut args: Value, task_mcp: Option<Value>) -> Value {
    let Some(task_mcp) = task_mcp else {
        return args;
    };
    if method == "task.create"
        && args.get("mcp").is_none()
        && args.get("repo").is_some()
        && args.get("repo") == task_mcp.get("repo")
    {
        if let (Some(obj), Some(mcp)) = (args.as_object_mut(), task_mcp.get("mcp")) {
            obj.insert("mcp".to_string(), mcp.clone());
        }
    }
    args
}

/// Routes one already-parsed method/params pair to its result, or a
/// JSON-RPC-level `(code, message)` error for anything this bridge doesn't
/// know at all -- a bad tool name or a failed orchd call is *not* one of
/// these, both come back as a normal (non-error) tool result with
/// `isError: true`, same as the MCP spec wants.
fn dispatch(bridge: &Bridge, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(handle_initialize(bridge, params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list_result(bridge)),
        "tools/call" => Ok(handle_tools_call(bridge, params)),
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

/// Sends one request over `socket` with orchd's own `auth` field set (the
/// control token), the same envelope shape as `electron/orchestrator.cjs`'s
/// `orchdRequest` -- deliberately not `protocol::client_request`, which has
/// no `auth` field at all (that module belongs to the daemon side, which
/// never needs to authenticate itself).
fn call_orchd(socket: &Path, token: &str, method: &str, params: Value) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|e| format!("cannot reach orchd at {}: {e}", socket.display()))?;
    let id = uuid::Uuid::new_v4().to_string();
    let req = json!({"id": id, "method": method, "params": params, "auth": token});
    stream
        .write_all(req.to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    stream.write_all(b"\n").map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("orchd closed the connection before responding".to_string());
    }
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| e.to_string())?;
    if let Some(err) = v.get("error") {
        let message = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("orchd error")
            .to_string();
        Err(message)
    } else {
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}

fn read_control_token(data_dir: &Path) -> Result<String, String> {
    let path = data_dir.join("control.token");
    let contents = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "cannot read {}: {e} (is `orchd serve --data {}` running?)",
            path.display(),
            data_dir.display()
        )
    })?;
    let token = contents.trim().to_string();
    if token.is_empty() {
        return Err(format!("{} is empty", path.display()));
    }
    Ok(token)
}

/// One line in (a JSON-RPC request), at most one line out. A parse failure
/// can't be tied to a request `id` (there wasn't a valid one to read), so it
/// always answers with `id: null`, matching JSON-RPC's own parse-error
/// convention. Notifications (no `id` field, parsed or not) get no
/// response at all.
fn handle_line(bridge: &Bridge, line: &str) -> Option<Value> {
    let req: RpcRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32700, "message": format!("parse error: {e}")},
            }))
        }
    };
    let id = req.id?;
    let response = match dispatch(bridge, &req.method, &req.params) {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
        }
    };
    Some(response)
}

pub fn run(args: &[String]) -> i32 {
    let mut data_dir_arg: Option<String> = None;
    let mut socket_arg: Option<String> = None;
    let mut task: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--data" if i + 1 < args.len() => {
                data_dir_arg = Some(args[i + 1].clone());
                i += 2;
            }
            "--socket" if i + 1 < args.len() => {
                socket_arg = Some(args[i + 1].clone());
                i += 2;
            }
            "--task" if i + 1 < args.len() => {
                task = Some(args[i + 1].clone());
                i += 2;
            }
            _ => i += 1,
        }
    }
    let Some(data_dir_arg) = data_dir_arg else {
        eprintln!("orchd mcp: --data <dir> is required");
        return 2;
    };
    let data_dir = PathBuf::from(data_dir_arg);
    let socket = socket_arg
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("orchd.sock"));
    let token = match read_control_token(&data_dir) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("orchd mcp: {e}");
            return 1;
        }
    };
    let bridge = Bridge {
        socket,
        token,
        task,
    };

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(&bridge, &line) {
            let mut out = response.to_string();
            out.push('\n');
            let mut handle = stdout.lock();
            if handle.write_all(out.as_bytes()).is_err() {
                break;
            }
            if handle.flush().is_err() {
                break;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::engine::App;
    use crate::protocol::Dispatcher;
    use std::sync::Arc;

    fn bridge(task: Option<&str>) -> Bridge {
        Bridge {
            // Never dialed by the branches under test here (`initialize`,
            // `ping`, `tools/list`, and the invalid-params/unknown-tool
            // exits out of `tools/call`); a bogus path just documents that.
            socket: PathBuf::from("/nonexistent/orchd.sock"),
            token: "t".to_string(),
            task: task.map(str::to_string),
        }
    }

    fn orchestrator() -> Bridge {
        bridge(None)
    }

    #[test]
    fn initialize_echoes_a_requested_protocol_version_and_carries_the_role() {
        let params = json!({"protocolVersion": "2024-11-05"});
        let result = dispatch(&orchestrator(), "initialize", &params).unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "sushiai-orchestrator");
        assert!(result["instructions"]
            .as_str()
            .unwrap()
            .contains("task_preflight"));
    }

    #[test]
    fn initialize_defaults_the_protocol_version_when_absent() {
        let result = dispatch(&orchestrator(), "initialize", &json!({})).unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn ping_returns_an_empty_object() {
        let result = dispatch(&orchestrator(), "ping", &json!({})).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn tools_list_has_exactly_the_fifteen_tools_and_no_delete_or_settings_set() {
        let result = dispatch(&orchestrator(), "tools/list", &json!({})).unwrap();
        let tools = result["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
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
            ]
        );
        assert_eq!(names, ORCHESTRATOR_TOOLS);
        for t in tools {
            assert!(t["inputSchema"]["type"] == "object");
            assert!(!t["inputSchema"]["properties"].is_null());
        }
        for name in TASK_TOOLS {
            let tool = tools.iter().find(|t| t["name"] == name).unwrap();
            let props = tool["inputSchema"]["properties"].as_object().unwrap();
            assert!(!props.is_empty(), "{name}");
        }
    }

    #[test]
    fn a_task_bridge_offers_only_messaging_and_never_a_sender_choice() {
        let task = bridge(Some("t1"));
        let result = dispatch(&task, "tools/list", &json!({})).unwrap();
        let tools = result["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, TASK_TOOLS);
        for t in tools {
            assert!(t["inputSchema"]["properties"].get("from").is_none());
        }
        let init = dispatch(&task, "initialize", &json!({})).unwrap();
        assert_eq!(init["serverInfo"]["name"], "sushiai-messages");
        for name in ["task_create", "task_stop", "orchestrator_reply"] {
            let call = json!({"name": name, "arguments": {}});
            let out = dispatch(&task, "tools/call", &call).unwrap();
            assert_eq!(out["isError"], true, "{name}");
        }
    }

    #[test]
    fn message_params_send_as_the_bridge_task_whatever_the_agent_claims() {
        let sent = message_params(
            "peer_send",
            json!({"from": "someone-else", "to": "t2", "text": "hi"}),
            Some("t1"),
        );
        assert_eq!(sent["from"], "t1");
        let asked = message_params("ask_orchestrator", json!({"text": "?"}), Some("t1"));
        assert_eq!(asked["from"], "t1");
        assert_eq!(asked["to"], ORCHESTRATOR);
        let inbox = message_params("inbox_read", json!({}), Some("t1"));
        assert_eq!(inbox, json!({"id": "t1"}));
        // Without a task the sender defaults to the orchestrator.
        let inbox = message_params("inbox_read", json!({}), None);
        assert_eq!(inbox["id"], ORCHESTRATOR);
        let answer = json!({"question": "m1", "text": "yes"});
        let reply = message_params("orchestrator_reply", answer, None);
        assert_eq!(reply["from"], ORCHESTRATOR);
        assert_eq!(reply["replyTo"], "m1");
        assert_eq!(reply["text"], "yes");
    }

    #[test]
    fn unknown_method_is_a_json_rpc_method_not_found_error() {
        let err = dispatch(&orchestrator(), "not/a/method", &json!({})).unwrap_err();
        assert_eq!(err.0, -32601);
    }

    #[test]
    fn tools_call_with_an_unknown_tool_name_is_a_tool_error_not_a_protocol_error() {
        let result = dispatch(
            &orchestrator(),
            "tools/call",
            &json!({"name": "task_delete", "arguments": {}}),
        )
        .unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("unknown tool"));
    }

    #[test]
    fn tools_call_with_malformed_params_is_a_tool_error_not_a_protocol_error() {
        let result = dispatch(&orchestrator(), "tools/call", &json!({"arguments": {}})).unwrap();
        assert_eq!(result["isError"], true);
    }

    #[test]
    fn every_tool_name_maps_to_a_dotted_orchd_method() {
        let b = orchestrator();
        for (name, method, ..) in tool_specs() {
            assert_eq!(orchd_method_for(&b, name), Some(method));
        }
        assert_eq!(orchd_method_for(&b, "task_delete"), None);
        assert_eq!(orchd_method_for(&b, "settings_set"), None);
        assert_eq!(orchd_method_for(&b, "secrets_set"), None);
    }

    #[test]
    fn a_notification_without_an_id_gets_no_response() {
        let out = handle_line(&orchestrator(), r#"{"method":"ping","params":{}}"#);
        assert!(out.is_none());
    }

    #[test]
    fn a_request_with_an_id_gets_a_jsonrpc_shaped_response() {
        let out = handle_line(&orchestrator(), r#"{"id":1,"method":"ping","params":{}}"#).unwrap();
        assert_eq!(out["jsonrpc"], "2.0");
        assert_eq!(out["id"], 1);
        assert_eq!(out["result"], json!({}));
    }

    #[test]
    fn unparseable_json_answers_with_a_null_id_parse_error() {
        let out = handle_line(&orchestrator(), "not json").unwrap();
        assert_eq!(out["id"], Value::Null);
        assert_eq!(out["error"]["code"], -32700);
    }

    const TASK_A: &str = "11111111-1111-4111-8111-111111111111";
    const TASK_B: &str = "22222222-2222-4222-8222-222222222222";
    const UNKNOWN: &str = "99999999-9999-4999-8999-999999999999";

    /// A real daemon (`App` behind `protocol::serve`) on a temp socket with
    /// two tasks in one repo; returns its socket and control token.
    async fn live_daemon(dir: &Path) -> (PathBuf, String) {
        let socket = dir.join("orchd.sock");
        let orchd = "/x/orchd".to_string();
        let app = App::new(dir.into(), socket.clone(), orchd).unwrap();
        for id in [TASK_A, TASK_B] {
            let task = serde_json::from_value(json!({
                "id": id, "title": id, "goal": "g", "criteria": [], "verify": [],
                "repo": "/r", "worktree": "/w", "branch": "b", "baseSha": "s",
                "status": "running", "tier": "standard", "decisions": [],
                "attempts": [], "costUsd": 0.0, "createdAt": 1, "updatedAt": 1,
            }))
            .unwrap();
            app.store.save_task(&task).unwrap();
        }
        let dispatcher: Arc<dyn Dispatcher> = app;
        let (shutdown, rx) = tokio::sync::broadcast::channel(1);
        let path = socket.clone();
        tokio::spawn(async move {
            let _keep_serving = shutdown;
            let _ = crate::protocol::serve(&path, dispatcher, rx).await;
        });
        while !socket.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let token = read_control_token(dir).unwrap();
        (socket, token)
    }

    /// One tool call from inside the runtime; the bridge itself blocks.
    fn call(bridge: &Bridge, name: &str, arguments: Value) -> Value {
        let params = json!({"name": name, "arguments": arguments});
        tokio::task::block_in_place(|| handle_tools_call(bridge, &params))
    }

    fn refused(bridge: &Bridge, name: &str, arguments: Value) {
        let out = call(bridge, name, arguments.clone());
        assert_eq!(out["isError"], true, "{name} {arguments} -> {out}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_message_is_a_tool_error_and_nothing_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let (socket, token) = live_daemon(dir.path()).await;
        let bridge = |task: Option<&str>| Bridge {
            socket: socket.clone(),
            token: token.clone(),
            task: task.map(str::to_string),
        };
        let orch = bridge(None);
        let a = bridge(Some(TASK_A));
        let stranger = bridge(Some(UNKNOWN));
        let a_to_a = json!({"from": TASK_A, "to": TASK_A, "text": "hi"});
        refused(&orch, "peer_send", a_to_a);
        refused(&a, "peer_send", json!({"to": TASK_A, "text": "hi"}));
        refused(&a, "peer_send", json!({"to": UNKNOWN, "text": "hi"}));
        let from_unknown = json!({"from": UNKNOWN, "to": TASK_A, "text": "hi"});
        refused(&orch, "peer_send", from_unknown);
        refused(&orch, "ask_orchestrator", json!({"text": "?"}));
        refused(&stranger, "ask_orchestrator", json!({"text": "?"}));
        assert!(!dir.path().join("messages.json").exists());

        // The same bridge sends a valid one, still waiting for its reader.
        let out = call(&a, "peer_send", json!({"to": TASK_B, "text": "hi"}));
        assert!(out.get("isError").is_none(), "{out}");
        let inbox = call(&bridge(Some(TASK_B)), "inbox_read", json!({}));
        let text = inbox["content"][0]["text"].as_str().unwrap();
        let inbox: Value = serde_json::from_str(text).unwrap();
        assert_eq!(inbox[0]["from"], TASK_A);
        assert_eq!(inbox[0]["delivered"], false);
    }

    #[test]
    fn task_create_gets_the_project_mcp_only_for_its_own_repo() {
        let file = json!({"repo": "/r", "mcp": {"mcpServers": {"a": {}}}});
        let got = with_task_mcp("task.create", json!({"repo": "/r"}), Some(file.clone()));
        assert_eq!(got["mcp"]["mcpServers"]["a"], json!({}));
        let other = with_task_mcp("task.create", json!({"repo": "/x"}), Some(file.clone()));
        assert!(other.get("mcp").is_none());
        let own = with_task_mcp(
            "task.create",
            json!({"repo": "/r", "mcp": {"mcpServers": {}}}),
            Some(file.clone()),
        );
        assert_eq!(own["mcp"], json!({"mcpServers": {}}));
        let start = with_task_mcp("task.start", json!({"repo": "/r"}), Some(file));
        assert!(start.get("mcp").is_none());
    }
}
