use super::call::{handle_tools_call, tools_list_result};
use super::scope::{Bridge, Caller};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

/// Protocol version handed back when the client didn't ask for a specific
/// one. When it did ask, we just echo it back rather than maintaining a
/// compatibility matrix here -- this bridge has no version-specific
/// behavior of its own to gate on.
pub(super) const PROTOCOL_VERSION: &str = "2025-06-18";

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

pub(super) fn handle_initialize(bridge: &Bridge, params: &Value) -> Value {
    let protocol_version = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or(PROTOCOL_VERSION);
    let (name, instructions) = match bridge.task {
        Some(_) => (
            "sushiai-messages",
            crate::prompts::get("mcp_task_instructions"),
        ),
        None => (
            "sushiai-orchestrator",
            crate::prompts::get("mcp_instructions"),
        ),
    };
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": name, "version": env!("CARGO_PKG_VERSION")},
        "instructions": instructions,
    })
}

/// Routes one already-parsed method/params pair to its result, or a
/// JSON-RPC-level `(code, message)` error for anything this bridge doesn't
/// know at all -- a bad tool name or a failed orchd call is *not* one of
/// these, both come back as a normal (non-error) tool result with
/// `isError: true`, same as the MCP spec wants.
pub(super) fn dispatch(
    bridge: &Bridge,
    method: &str,
    params: &Value,
) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(handle_initialize(bridge, params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list_result(bridge)),
        "tools/call" => Ok(handle_tools_call(bridge, params)),
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

/// One line in (a JSON-RPC request), at most one line out. A parse failure
/// can't be tied to a request `id` (there wasn't a valid one to read), so it
/// always answers with `id: null`, matching JSON-RPC's own parse-error
/// convention. Notifications (no `id` field, parsed or not) get no
/// response at all.
pub(super) fn handle_line(bridge: &Bridge, line: &str) -> Option<Value> {
    let req: RpcRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32700, "message": format!("parse error: {e}")},
            }));
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

/// The stdio MCP server loop: one JSON-RPC line in, at most one line out, until `input` ends
/// or `output` closes. `task` / `read_only` pick the tool scope; `call` is the transport.
pub fn serve_stdio(
    task: Option<String>,
    read_only: bool,
    call: Caller,
    input: impl BufRead,
    mut output: impl Write,
) {
    let bridge = Bridge {
        call,
        task,
        read_only,
    };
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(&bridge, &line) {
            let mut out = response.to_string();
            out.push('\n');
            if output.write_all(out.as_bytes()).is_err() || output.flush().is_err() {
                break;
            }
        }
    }
}
