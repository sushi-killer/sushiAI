use super::types::ChatTool;
use crate::engine::state::App;
use crate::policy::routing::Harness;
use serde_json::json;
use std::path::Path;

const TOOL_PREFIX: &str = "mcp__sushiai-orchestrator__";

const MAX_TOOLS: usize = 8;

const MAX_SUMMARY: usize = 120;

/// Tools whose input names a task by `id`.
const TASK_ID_TOOLS: [&str; 11] = [
    "task_get",
    "task_backlog",
    "task_start",
    "task_report",
    "task_answer",
    "task_amend",
    "task_archive",
    "task_unarchive",
    "task_stop",
    "task_delete",
    "task_lead_touch",
];

pub(super) fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > MAX_SUMMARY {
        format!(
            "{}…",
            line.chars().take(MAX_SUMMARY - 1).collect::<String>()
        )
    } else {
        line.to_string()
    }
}

fn tool_summary(app: &App, repo: &str, name: &str, input: &serde_json::Value) -> String {
    let field = |key: &str| input.get(key).and_then(|v| v.as_str());
    if TASK_ID_TOOLS.contains(&name) {
        let title = field("id")
            .and_then(|id| app.store.load_task(id).ok().flatten())
            .map(|task| task.title.clone());
        return title.map(|t| one_line(&t)).unwrap_or_default();
    }
    match name {
        "task_create" => field("title").map(one_line).unwrap_or_default(),
        "Read" | "Grep" | "Glob" => {
            let detail = field("file_path")
                .map(|path| {
                    let path = Path::new(path);
                    path.strip_prefix(repo).unwrap_or(path).to_string_lossy()
                })
                .or_else(|| field("pattern").map(Into::into));
            detail.map(|d| one_line(&d)).unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn chat_tool(app: &App, repo: &str, name: &str, input: &serde_json::Value) -> ChatTool {
    let name = name.strip_prefix(TOOL_PREFIX).unwrap_or(name);
    ChatTool {
        name: name.to_string(),
        summary: tool_summary(app, repo, name, input),
    }
}

/// The tool calls one stdout line reports. Codex reports an MCP call twice
/// (started and completed), so `seen` holds the item ids already counted.
pub(super) fn tools_in_line(
    app: &App,
    repo: &str,
    harness: Harness,
    line: &str,
    seen: &mut std::collections::HashSet<String>,
) -> Vec<ChatTool> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let kind = v.get("type").and_then(|t| t.as_str());
    match (harness, kind) {
        (Harness::Claude, Some("assistant")) => {
            if !v.get("parent_tool_use_id").is_none_or(|p| p.is_null()) {
                return Vec::new();
            }
            let blocks = v.pointer("/message/content").and_then(|c| c.as_array());
            blocks
                .into_iter()
                .flatten()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                .filter_map(|b| {
                    let name = b.get("name").and_then(|n| n.as_str())?;
                    Some(chat_tool(
                        app,
                        repo,
                        name,
                        b.get("input").unwrap_or(&json!({})),
                    ))
                })
                .collect()
        }
        (Harness::Codex, Some("item.started" | "item.completed")) => {
            let item = &v["item"];
            let (Some("mcp_tool_call"), Some(id), Some(tool)) = (
                item.get("type").and_then(|t| t.as_str()),
                item.get("id").and_then(|i| i.as_str()),
                item.get("tool").and_then(|t| t.as_str()),
            ) else {
                return Vec::new();
            };
            if !seen.insert(id.to_string()) {
                return Vec::new();
            }
            let empty = json!({});
            vec![chat_tool(
                app,
                repo,
                tool,
                item.get("arguments").unwrap_or(&empty),
            )]
        }
        _ => Vec::new(),
    }
}

/// Appends calls to a turn's list: a call identical to the one before it
/// counts once and only the first `MAX_TOOLS` are kept.
pub(super) fn keep_tools(list: &mut Vec<ChatTool>, calls: Vec<ChatTool>) {
    for call in calls {
        if list.len() < MAX_TOOLS && list.last() != Some(&call) {
            list.push(call);
        }
    }
}

#[cfg(test)]
#[path = "tests/tools.rs"]
mod tests;
