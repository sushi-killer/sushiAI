use super::access::{classify, Kind};
use super::catalog::load_cache;
use super::server::{claude_tool_name, home, resolve, sanitize, server_key, transport, Transport};
use super::stdio::call_stdio;
use crate::engine::state::App;
use crate::harness::environment::{augmented_path, resolve_binary};
use crate::policy::routing::Harness;
use crate::policy::settings::{ChatToolConfig, Settings};
use crate::prompts::{engine_text, fill};
use crate::store;
use serde_json::json;
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

/// What the one-call turn that sends an approved write tool is told.
pub(in crate::engine) fn send_prompt(tool: &str, args: &serde_json::Value) -> String {
    fill(
        engine_text("chat_tools_send"),
        &[("tool", tool), ("args", &args.to_string())],
    )
}

/// Whether the orchestrator may propose this call: the tool belongs to an
/// enabled connected tool that is reachable, is one of its tools and not a read
/// tool.
pub fn proposable(app: &App, settings: &Settings, id: &str, tool: &str) -> bool {
    let Some(cfg) = settings.chat_tools.iter().find(|c| c.enabled && c.id == id) else {
        return false;
    };
    let Some(known) = load_cache(app).remove(id).filter(|d| d.status == "ok") else {
        return false;
    };
    let found = known.tools.iter().find(|t| t.name == tool);
    let Some(found) = found else {
        return false;
    };
    classify(tool, found.read_only, &cfg.overrides) == Kind::Write
}

/// Runs a write call the owner approved, exactly as stored. A stdio server is
/// called directly. An http server is signed in through Claude Code's own
/// OAuth store, so it gets a one-shot Claude turn that may use only that tool.
pub async fn execute(
    app: &App,
    cfg: &ChatToolConfig,
    tool: &str,
    args: &serde_json::Value,
) -> Result<String, String> {
    let server = resolve(cfg, &home())?;
    if transport(&server) == Transport::Stdio {
        return call_stdio(&server, tool, args).await;
    }
    let key = server_key(cfg);
    let dir = app.data_dir.join("chat-tools");
    let _ = std::fs::create_dir_all(&dir);
    let config = dir.join(format!("send-{}.json", sanitize(&key)));
    store::files::write_json_atomic(&config, &json!({"mcpServers": {key.clone(): server}}))
        .map_err(|e| e.to_string())?;
    let argv = send_argv(&config, &claude_tool_name(&key, tool));
    let prompt = send_prompt(tool, args);
    let mut cmd = tokio::process::Command::new(resolve_binary(Harness::Claude));
    cmd.args(&argv)
        .env("PATH", augmented_path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let outcome = tokio::time::timeout(Duration::from_secs(120), async {
        let mut child = cmd.spawn().map_err(|e| format!("claude: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(prompt.as_bytes()).await;
        }
        child.wait_with_output().await.map_err(|e| e.to_string())
    })
    .await
    .map_err(|_| "the call did not finish in time".to_string())??;
    let _ = std::fs::remove_file(&config);
    let reply: serde_json::Value = serde_json::from_slice(&outcome.stdout)
        .map_err(|_| "claude gave no readable reply".to_string())?;
    let text = reply
        .get("result")
        .and_then(|r| r.as_str())
        .unwrap_or_default()
        .to_string();
    if reply.get("is_error").and_then(|e| e.as_bool()) == Some(true) {
        Err(text)
    } else {
        Ok(text)
    }
}

/// The argv of the one-shot turn that runs one approved call.
fn send_argv(mcp_config: &Path, tool: &str) -> Vec<String> {
    [
        "-p",
        "--output-format",
        "json",
        "--max-turns",
        "3",
        "--model",
        "sonnet",
        "--setting-sources",
        "",
        "--disable-slash-commands",
        "--tools",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        &mcp_config.to_string_lossy(),
        "--allowedTools",
        tool,
        "--permission-prompts",
        "none",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[cfg(test)]
mod tests;
