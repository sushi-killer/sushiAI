use super::catalog::{failed, update_cache, Discovery, KnownTool};
use super::server::{home, resolve, sanitize, server_key, transport, Transport};
use super::stdio::{list_stdio, within};
use crate::clock::now_ms;
use crate::engine::state::App;
use crate::harness::environment::{augmented_path, resolve_binary};
use crate::policy::routing::Harness;
use crate::policy::settings::ChatToolConfig;
use crate::store;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// A Claude turn over one server, stopped as soon as it reports which servers
/// it connected: what an http server offers, without a model call.
async fn probe_claude(app: &App, key: &str, server: &serde_json::Value) -> Discovery {
    let dir = app.data_dir.join("chat-tools");
    let _ = std::fs::create_dir_all(&dir);
    let config = dir.join(format!("probe-{}.json", sanitize(key)));
    let _ = store::files::write_json_atomic(&config, &json!({"mcpServers": {key: server}}));
    let mut cmd = tokio::process::Command::new(resolve_binary(Harness::Claude));
    cmd.args([
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--max-turns",
        "1",
        "--strict-mcp-config",
        "--tools",
        "",
        "--mcp-config",
    ])
    .arg(&config)
    .env("PATH", augmented_path())
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .stderr(std::process::Stdio::null())
    .kill_on_drop(true);
    let outcome = within(async {
        let mut child = cmd.spawn().map_err(|e| format!("claude: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(b"hi").await;
        }
        let stdout = child.stdout.take().expect("piped stdout");
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
            if let Some(found) = discovery_from_init(&line, key) {
                return Ok(found);
            }
        }
        Err("claude ended without listing its servers".to_string())
    })
    .await;
    let _ = std::fs::remove_file(&config);
    outcome.unwrap_or_else(|reason| failed("failed", reason))
}

/// What a stream line says about server `key`, when it is the init event.
fn discovery_from_init(line: &str, key: &str) -> Option<Discovery> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v.get("type")?.as_str()? != "system" || v.get("subtype")?.as_str()? != "init" {
        return None;
    }
    let status = v
        .get("mcp_servers")?
        .as_array()?
        .iter()
        .find(|s| s.get("name").and_then(|n| n.as_str()) == Some(key))
        .and_then(|s| s.get("status")?.as_str())
        .unwrap_or("failed");
    let prefix = format!("mcp__{}__", sanitize(key));
    let tools = v
        .get("tools")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str()?.strip_prefix(&prefix))
        .map(|name| KnownTool {
            name: name.to_string(),
            read_only: None,
        })
        .collect();
    Some(match status {
        "connected" => Discovery {
            status: "ok".to_string(),
            reason: None,
            tools,
            checked_at: now_ms(),
        },
        "needs-auth" => failed(
            "needs-auth",
            "Needs sign-in: open Claude Code and authorise it with /mcp",
        ),
        other => failed("failed", format!("The server did not connect ({other})")),
    })
}

/// Looks at one configured server now and remembers what it found. A tool
/// whose name is already known keeps the `readOnlyHint` it had.
pub(super) async fn discover(app: &App, cfg: &ChatToolConfig) -> Discovery {
    let found = match resolve(cfg, &home()) {
        Err(reason) => failed("unavailable", reason),
        Ok(server) => match transport(&server) {
            Transport::Stdio => match list_stdio(&server).await {
                Ok(tools) => Discovery {
                    status: "ok".to_string(),
                    reason: None,
                    tools,
                    checked_at: now_ms(),
                },
                Err(reason) => failed("failed", reason),
            },
            Transport::Http => probe_claude(app, &server_key(cfg), &server).await,
        },
    };
    let stored = found.clone();
    update_cache(app, |cache| {
        cache.insert(cfg.id.clone(), stored);
    });
    found
}

/// Notes a turn's init event in the status cache: a server that connected,
/// needs sign-in or failed, as Claude saw it.
pub fn note_init(app: &App, line: &str) {
    if !line.contains("\"mcp_servers\"") {
        return;
    }
    let settings = app.settings.read().unwrap().clone();
    for cfg in settings.chat_tools.iter().filter(|c| c.enabled) {
        let Ok(server) = resolve(cfg, &home()) else {
            continue;
        };
        if transport(&server) != Transport::Http && !line.contains(&server_key(cfg)) {
            continue;
        }
        if let Some(found) = discovery_from_init(line, &server_key(cfg)) {
            let id = cfg.id.clone();
            update_cache(app, |cache| match cache.get_mut(&id) {
                Some(old) if found.status == "ok" && old.status == "ok" => {}
                _ => {
                    cache.insert(id, found);
                }
            });
        }
    }
}

#[cfg(test)]
mod tests;
