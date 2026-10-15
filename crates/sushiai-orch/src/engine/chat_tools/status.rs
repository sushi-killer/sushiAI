use super::access::classify;
use super::catalog::load_cache;
use super::discovery::discover;
use super::server::{available_json, home, resolve, transport, Transport};
use crate::engine::state::App;
use crate::policy::settings::Settings;
use serde_json::json;
use std::collections::BTreeMap;

/// One row of the Settings list.
pub async fn status_json(
    app: &App,
    settings: &Settings,
    refresh: bool,
    cached_only: bool,
    only: Option<&str>,
) -> Result<serde_json::Value, String> {
    let cache = load_cache(app);
    let mut rows = Vec::new();
    for cfg in settings
        .chat_tools
        .iter()
        .filter(|cfg| only.is_none_or(|id| cfg.id == id))
    {
        let resolved = resolve(cfg, &home());
        let mut found = cache.get(&cfg.id).cloned();
        if cfg.enabled && !cached_only && (refresh || found.is_none()) {
            found = Some(discover(app, cfg).await);
        }
        let (kind, codex) = match &resolved {
            Ok(server) => match transport(server) {
                Transport::Stdio => ("stdio", None),
                Transport::Http => (
                    "http",
                    Some("Codex chats cannot use http servers; they run without it."),
                ),
            },
            Err(_) => ("unknown", None),
        };
        let (status, reason) = match (&resolved, &found) {
            (Err(reason), _) => ("unavailable", Some(reason.clone())),
            (_, Some(d)) => (d.status.as_str(), d.reason.clone()),
            _ => ("unchecked", None),
        };
        let tools: Vec<serde_json::Value> = found
            .iter()
            .flat_map(|d| d.tools.iter())
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "kind": classify(&tool.name, tool.read_only, &cfg.overrides).as_str(),
                    "guess": classify(&tool.name, tool.read_only, &BTreeMap::new()).as_str(),
                })
            })
            .collect();
        rows.push(json!({
            "id": cfg.id,
            "transport": kind,
            "status": status,
            "reason": reason,
            "codex": codex,
            "tools": tools,
        }));
    }
    Ok(json!({"tools": rows}))
}

pub async fn handle_tools(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let settings = app.settings.read().unwrap().clone();
    let refresh = params.get("refresh").and_then(|r| r.as_bool()) == Some(true);
    let cached_only = params.get("cached").and_then(|r| r.as_bool()) == Some(true);
    let only = params.get("id").and_then(|r| r.as_str());
    status_json(app, &settings, refresh, cached_only, only).await
}

pub async fn handle_tool_servers(
    app: &App,
    _params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let settings = app.settings.read().unwrap().clone();
    Ok(available_json(&settings, &home()))
}
