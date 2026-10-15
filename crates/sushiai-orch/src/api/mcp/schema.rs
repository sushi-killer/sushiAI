use serde_json::{json, Value};

/// What a task agent gets (`--task`): only messaging, always as itself.
pub const TASK_TOOLS: [&str; 4] = [
    // In `tool_specs` order, so a task's tools/list reads the same way.
    "peer_list",
    "peer_send",
    "inbox_read",
    "ask_orchestrator",
];

/// What a Brainstorm or Plan turn gets (`--read-only`): looking, never changing.
pub const READ_ONLY_TOOLS: [&str; 5] = [
    "task_list",
    "task_get",
    "settings_get",
    "repo_notes_list",
    "costs_summary",
];

/// What the orchestrator agent gets: every tool below.
pub const ORCHESTRATOR_TOOLS: [&str; 27] = [
    "task_list",
    "task_get",
    "task_create",
    "task_backlog",
    "task_start",
    "task_report",
    "task_pr",
    "task_lead_touch",
    "task_answer",
    "task_amend",
    "settings_get",
    "task_archive",
    "task_unarchive",
    "task_stop",
    "task_delete",
    "peer_list",
    "peer_send",
    "inbox_read",
    "ask_orchestrator",
    "orchestrator_reply",
    "repo_audit",
    "evolution_run",
    "repo_notes_list",
    "repo_notes_add",
    "repo_notes_remove",
    "costs_summary",
    "hosts_list",
];

/// The tools an orchestrator bridge may point at another saved host with `host`; the call
/// then goes to the daemon's `relay.call`, which the desktop app answers.
pub(super) const HOST_TOOLS: [&str; 13] = [
    "task_list",
    "task_get",
    "task_create",
    "task_backlog",
    "task_start",
    "task_report",
    "task_pr",
    "task_lead_touch",
    "task_answer",
    "task_amend",
    "task_archive",
    "task_unarchive",
    "inbox_read",
];

/// The tools whose `host` may be `all`: every saved host at once.
pub(super) const ALL_HOSTS_TOOLS: [&str; 2] = ["task_list", "inbox_read"];

pub(super) fn host_schema(tool: &str) -> Value {
    let all = if ALL_HOSTS_TOOLS.contains(&tool) {
        ", or \"all\" for every saved host (results labelled with their host, unreachable hosts listed)"
    } else {
        ""
    };
    json!({
        "type": "string",
        "description": format!(
            "Run this on another saved host: \"local\" (This Mac) or \"ssh:<id>\" from hosts_list{all}. Omit for this machine's own orchestrator."
        ),
    })
}

fn from_schema() -> Value {
    json!({
        "type": "string",
        "description": "Sender: a task id, or \"orchestrator\" (the default).",
    })
}

/// name, orchd method, description, JSON Schema for `inputSchema`. No
/// `settings_set` (would let an agent silently change parallelism/planner
/// config the owner didn't ask to change), no `secrets_*` (an agent has no
/// business reading or writing API keys) -- both deliberately left off this
/// surface, not just forgotten. `task_stop` and `task_delete` are the
/// owner's orchestrator only (never `--task` or `--read-only`); the daemon
/// refuses a bridge delete of a live task, and an overseer turn never deletes.
pub(super) fn tool_specs() -> Vec<ToolSpec> {
    let mut tools = tasks::reads();
    tools.extend(tasks::creation());
    tools.extend(tasks::queue());
    tools.extend(tasks::delivery());
    tools.extend(tasks::answers());
    tools.extend(repository::settings());
    tools.extend(tasks::controls());
    tools.extend(messages::specs());
    tools.extend(repository::analysis());
    tools.extend(repository::notes());
    tools.extend(repository::costs());
    tools.extend(repository::hosts());
    tools
}

type ToolSpec = (&'static str, &'static str, &'static str, Value);

mod messages;
mod repository;
mod tasks;
