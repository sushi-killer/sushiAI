use super::schema::{ALL_HOSTS_TOOLS, HOST_TOOLS, READ_ONLY_TOOLS, TASK_TOOLS};
use crate::task::messages::ORCHESTRATOR;
use serde_json::{json, Value};

/// The bridge's scope: the whole orchestrator surface, one task's messaging
/// only (`--task <id>`), where every message is sent as that task, or the
/// read-only tools of a Brainstorm or Plan turn (`--read-only`).
pub(super) struct Bridge {
    /// One request to the orchestration backend: `(method, params)` to its result or message.
    pub(super) call: Caller,
    pub(super) task: Option<String>,
    pub(super) read_only: bool,
}

/// How the bridge reaches the backend; the transport is the caller's choice.
pub type Caller = Box<dyn Fn(&str, Value) -> Result<Value, String>>;

impl Bridge {
    pub(super) fn offers(&self, tool_name: &str) -> bool {
        (self.task.is_none() || TASK_TOOLS.contains(&tool_name))
            && (!self.read_only || READ_ONLY_TOOLS.contains(&tool_name))
    }
}

/// Turns a messaging tool's arguments into its orchd params: the sender is
/// the bridge's task when it has one (an agent can't speak as another), else
/// `from`, else the orchestrator.
pub(super) fn message_params(tool: &str, mut args: Value, task: Option<&str>) -> Value {
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
            });
        }
        _ => {}
    }
    args
}

/// Takes the optional `host` out of a tool's arguments: `local`, `ssh:<id>`, or `all` where
/// the tool lists. A task bridge never names a host, and a tool that cannot run elsewhere
/// refuses one rather than quietly running here.
pub(super) fn take_host(
    bridge: &Bridge,
    tool: &str,
    args: &mut Value,
) -> Result<Option<String>, String> {
    let Some(value) = args.as_object_mut().and_then(|obj| obj.remove("host")) else {
        return Ok(None);
    };
    if bridge.task.is_some() {
        return Err(format!("{tool} takes no host on a task's bridge"));
    }
    if value.is_null() {
        return Ok(None);
    }
    if !HOST_TOOLS.contains(&tool) {
        return Err(format!("{tool} takes no host"));
    }
    let Some(host) = value.as_str().filter(|h| !h.is_empty()) else {
        return Err("host must be \"local\", \"ssh:<id>\" or \"all\"".to_string());
    };
    let valid = host == "local"
        || host.strip_prefix("ssh:").is_some_and(|id| !id.is_empty())
        || (host == "all" && ALL_HOSTS_TOOLS.contains(&tool));
    if !valid {
        let all = if ALL_HOSTS_TOOLS.contains(&tool) {
            ", \"ssh:<id>\" or \"all\""
        } else {
            " or \"ssh:<id>\""
        };
        return Err(format!(
            "host must be \"local\"{all} (see hosts_list), not {host:?}"
        ));
    }
    Ok(Some(host.to_string()))
}

/// A task the agent creates gets the MCP servers the app resolved for its
/// project (`ORCHD_TASK_MCP` names a `{repo, mcp}` file the app writes), the
/// same thing the app adds when the owner creates a task directly. Only for
/// that repo. Only an owner's chat turn may keep caller-set servers.
pub(super) fn with_task_mcp(
    method: &str,
    mut args: Value,
    task_mcp: Option<Value>,
    keep_caller_mcp: bool,
) -> Value {
    if method == "task.create" && !keep_caller_mcp {
        if let Some(obj) = args.as_object_mut() {
            obj.remove("mcp");
        }
    }
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

pub(super) fn bridge_source(source: Option<&str>) -> &'static str {
    match source {
        Some("overseer") => "overseer",
        Some("chat") => "chat",
        _ => "lead",
    }
}

/// Records where a task created through this bridge started: the
/// orchestrator chat agent (no task id) is `chat`, a task's own agent is
/// `handoff`. A source the caller set itself is kept.
/// `source` names the caller outside a task: a lead, chat, or overseer turn.
pub(super) fn with_source(method: &str, mut args: Value, from_task: bool, source: &str) -> Value {
    if matches!(
        method,
        "task.answer"
            | "task.amend"
            | "task.start"
            | "task.stop"
            | "task.delete"
            | "repo.notes.add"
            | "repo.notes.remove"
    ) {
        // Never trusted from the caller: it marks the answer as not the
        // owner's, so orchd refuses an allow on an owner-only question (and an
        // overseer's repo note).
        if let Some(obj) = args.as_object_mut() {
            let source = if from_task { "mcp" } else { source };
            obj.insert("source".to_string(), json!(source));
        }
    }
    if method == "task.pr" {
        // Task branch only: the bridge never forwards a branch, and marks the
        // call so the backend refuses one too.
        if let Some(obj) = args.as_object_mut() {
            obj.remove("branch");
            let source = if from_task { "mcp" } else { source };
            obj.insert("source".to_string(), json!(source));
        }
    }
    if method == "task.create" && args.get("source").is_none() {
        if let Some(obj) = args.as_object_mut() {
            // A task source names who asked for the task, so an overseer
            // turn creates one as the chat does.
            let source = if from_task { "handoff" } else { "chat" };
            obj.insert("source".to_string(), json!(source));
        }
    }
    args
}
