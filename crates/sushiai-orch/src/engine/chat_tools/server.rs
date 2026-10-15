use crate::policy::settings::{ChatToolConfig, Settings};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Stdio,
    Http,
}

pub fn transport(server: &serde_json::Value) -> Transport {
    if server.get("url").is_some() {
        Transport::Http
    } else {
        Transport::Stdio
    }
}

/// Where the owner's MCP config lives: `$HOME`, or `$SUSHIAI_MCP_HOME` in a
/// test profile so nothing of the owner's own config shows up.
pub(super) fn home() -> PathBuf {
    match std::env::var("SUSHIAI_MCP_HOME") {
        Ok(fixture) if !fixture.is_empty() => PathBuf::from(fixture),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_default()),
    }
}

pub(super) fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `mcpServers` of `~/.claude.json`.
fn claude_json_servers(home: &Path) -> BTreeMap<String, serde_json::Value> {
    read_json(&home.join(".claude.json"))
        .and_then(|v| v.get("mcpServers").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Every server of every installed plugin: `(plugin, server, definition)` with
/// `${CLAUDE_PLUGIN_ROOT}` resolved. A plugin's `.mcp.json` is either
/// `{"<server>": {...}}` or wrapped in `{"mcpServers": {...}}`.
fn plugin_servers(home: &Path) -> Vec<(String, String, serde_json::Value)> {
    let Some(installed) = read_json(&home.join(".claude/plugins/installed_plugins.json")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, installs) in installed
        .get("plugins")
        .and_then(|p| p.as_object())
        .into_iter()
        .flatten()
    {
        let plugin = key.split('@').next().unwrap_or(key);
        let Some(root) = installs
            .get(0)
            .and_then(|i| i.get("installPath"))
            .and_then(|p| p.as_str())
        else {
            continue;
        };
        let Some(config) = read_json(&Path::new(root).join(".mcp.json")) else {
            continue;
        };
        let servers = config.get("mcpServers").unwrap_or(&config);
        for (name, def) in servers.as_object().into_iter().flatten() {
            if def.is_object() {
                let text = def.to_string().replace("${CLAUDE_PLUGIN_ROOT}", root);
                if let Ok(def) = serde_json::from_str(&text) {
                    out.push((plugin.to_string(), name.clone(), def));
                }
            }
        }
    }
    out
}

fn ref_of(cfg: &ChatToolConfig) -> Option<&str> {
    cfg.server.get("ref").and_then(|r| r.as_str())
}

/// The server definition a config stands for, read from disk now. Stdio gets
/// an `args` array, an http server a `type`.
pub fn resolve(cfg: &ChatToolConfig, home: &Path) -> Result<serde_json::Value, String> {
    let mut server = match ref_of(cfg) {
        None => cfg.server.clone(),
        Some(r) => match r.split_once(':') {
            Some(("claude-json", name)) => claude_json_servers(home)
                .remove(name)
                .ok_or_else(|| format!("{name} is not in ~/.claude.json any more"))?,
            Some(("plugin", path)) => {
                let (plugin, name) = path.split_once('/').unwrap_or((path, path));
                plugin_servers(home)
                    .into_iter()
                    .find(|(p, n, _)| p == plugin && n == name)
                    .map(|(_, _, def)| def)
                    .ok_or_else(|| format!("the {plugin} plugin is not installed"))?
            }
            _ => return Err(format!("unknown server reference {r}")),
        },
    };
    let Some(object) = server.as_object_mut() else {
        return Err("the server definition is not an object".to_string());
    };
    if object.contains_key("url") {
        object
            .entry("type")
            .or_insert_with(|| serde_json::json!("http"));
    } else if object.contains_key("command") {
        object
            .entry("args")
            .or_insert_with(|| serde_json::json!([]));
    } else {
        return Err("the server has neither a command nor a url".to_string());
    }
    Ok(server)
}

/// The key a server has in the turn's `mcp.json`. A plugin's server keeps the
/// name Claude Code gives it, so the OAuth sign-in stored for it is reused.
pub fn server_key(cfg: &ChatToolConfig) -> String {
    match ref_of(cfg).and_then(|r| r.strip_prefix("plugin:")) {
        Some(path) => format!("plugin:{}", path.replace('/', ":")),
        None => cfg.id.clone(),
    }
}

/// Claude Code's tool-name form of a server key or tool name.
pub(in crate::engine) fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn claude_tool_name(key: &str, tool: &str) -> String {
    format!("mcp__{}__{}", sanitize(key), sanitize(tool))
}

/// Servers of `~/.claude.json` and the installed plugins that are not
/// connected tools yet.
pub fn available_json(settings: &Settings, home: &Path) -> serde_json::Value {
    let added: Vec<&str> = settings
        .chat_tools
        .iter()
        .filter_map(|c| ref_of(c))
        .collect();
    let mut out = Vec::new();
    let mut offer =
        |reference: String, label: String, source: String, server: &serde_json::Value| {
            if added.contains(&reference.as_str()) {
                return;
            }
            out.push(json!({
                "ref": reference,
                "label": label,
                "source": source,
                "transport": if server.get("url").is_some() { "http" } else { "stdio" },
            }));
        };
    for (name, server) in claude_json_servers(home) {
        offer(
            format!("claude-json:{name}"),
            name,
            "~/.claude.json".to_string(),
            &server,
        );
    }
    for (plugin, name, server) in plugin_servers(home) {
        offer(
            format!("plugin:{plugin}/{name}"),
            name,
            format!("{plugin} plugin"),
            &server,
        );
    }
    json!({"servers": out})
}

#[cfg(test)]
mod tests;
