use super::schema::{host_schema, tool_specs, HOST_TOOLS};
use super::scope::{bridge_source, message_params, take_host, with_source, with_task_mcp, Bridge};
use serde::Deserialize;
use serde_json::{json, Value};

/// The relay method on the daemon (`orch.relay.call`).
const RELAY: &str = "relay.call";

pub(super) fn orch_method_for(bridge: &Bridge, tool_name: &str) -> Option<&'static str> {
    tool_specs()
        .into_iter()
        .find(|(name, ..)| *name == tool_name && bridge.offers(name))
        .map(|(_, method, ..)| method)
}

pub(super) fn tools_list_result(bridge: &Bridge) -> Value {
    let tools: Vec<Value> = tool_specs()
        .into_iter()
        .filter(|(name, ..)| bridge.offers(name))
        .map(|(name, _, description, mut input_schema)| {
            if let Some(props) = input_schema["properties"].as_object_mut() {
                if bridge.task.is_some() {
                    props.remove("from");
                } else if HOST_TOOLS.contains(&name) {
                    props.insert("host".to_string(), host_schema(name));
                }
            }
            json!({"name": name, "description": description, "inputSchema": input_schema})
        })
        .collect();
    json!({"tools": tools})
}

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

fn tool_ok(result: Value) -> Value {
    let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
    json!({"content": [{"type": "text", "text": text}]})
}

fn tool_error(message: String) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}

pub(super) fn handle_tools_call(bridge: &Bridge, params: &Value) -> Value {
    let declared_source = std::env::var("ORCHD_BRIDGE_SOURCE").ok();
    call_tool(bridge, params, declared_source.as_deref())
}

pub(super) fn call_tool(bridge: &Bridge, params: &Value, declared_source: Option<&str>) -> Value {
    let p: ToolCallParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return tool_error(format!("invalid tools/call params: {e}")),
    };
    let Some(method) = orch_method_for(bridge, &p.name) else {
        return tool_error(format!("unknown tool: {}", p.name));
    };
    let mut args = if p.arguments.is_null() {
        json!({})
    } else {
        p.arguments
    };
    let host = match take_host(bridge, &p.name, &mut args) {
        Ok(host) => host,
        Err(message) => return tool_error(message),
    };
    if method == "hosts.list" {
        return match (bridge.call)(RELAY, json!({"method": method, "params": {}})) {
            Ok(result) => tool_ok(result),
            Err(message) => tool_error(message),
        };
    }
    // The project servers of a task on another host are resolved by the app for that host.
    let task_mcp = std::env::var("ORCHD_TASK_MCP")
        .ok()
        .filter(|_| host.is_none())
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok());
    let source = bridge_source(declared_source);
    let args = with_task_mcp(
        method,
        args,
        task_mcp,
        bridge.task.is_none() && source == "chat",
    );
    if method == "task.delete" && source == "overseer" {
        return tool_error("an overseer turn never deletes tasks; ask the owner".to_string());
    }
    let args = with_source(method, args, bridge.task.is_some(), source);
    let args = message_params(&p.name, args, bridge.task.as_deref());
    let reply = match host {
        Some(host) => (bridge.call)(
            RELAY,
            json!({"host": host, "method": method, "params": args}),
        ),
        None => (bridge.call)(method, args),
    };
    match reply {
        Ok(result) => tool_ok(result),
        Err(message) => tool_error(message),
    }
}
