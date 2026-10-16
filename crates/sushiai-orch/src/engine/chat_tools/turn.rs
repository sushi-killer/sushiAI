use super::access::{classify, Kind};
use super::catalog::load_cache;
use super::discovery::discover;
use super::server::{claude_tool_name, home, resolve, sanitize, server_key, transport, Transport};
use crate::clock::now_ms;
use crate::engine::state::App;
use crate::harness;
use crate::policy::settings::Settings;
use crate::prompts::engine_text;
use serde_json::json;
use std::collections::BTreeMap;

const RETRY_AFTER_MS: i64 = 10 * 60 * 1000;

/// Tools of a turn: what it may call, which servers to start and what the
/// model is told.
#[derive(Debug, Default, Clone)]
pub struct TurnTools {
    /// Server key -> definition, for the turn's `mcp.json`.
    pub servers: BTreeMap<String, serde_json::Value>,
    /// Read tools, in Claude's `mcp__server__tool` form.
    pub allowed: Vec<String>,
    /// Codex: `(name, stdio definition, read tools)`.
    pub codex: Vec<(String, serde_json::Value, Vec<String>)>,
    /// Appended to the mode's prompt; empty without connected tools.
    pub prompt: String,
    /// `(id, read tools, write tools)` of each server in `servers`.
    pub listed: Vec<(String, Vec<String>, Vec<String>)>,
}

/// The `sushi-action` instructions and the tools each server offers.
pub(in crate::engine) fn tools_prompt(listed: &[(String, Vec<String>, Vec<String>)]) -> String {
    let mut text = format!("\n\n{}", engine_text("chat_tools_intro"));
    for (id, reads, writes) in listed {
        text.push_str(&format!(
            "\n- {id}: read tools {}; write tools (you cannot call them) {}.",
            if reads.is_empty() {
                "none".into()
            } else {
                reads.join(", ")
            },
            if writes.is_empty() {
                "none".into()
            } else {
                writes.join(", ")
            },
        ));
    }
    text.push_str(&format!("\n{} ", engine_text("chat_tools_actions")));
    text.push_str(
        &listed
            .iter()
            .map(|(id, _, _)| id.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    );
    text.push('.');
    text
}

/// Resolves every enabled tool for a turn. One that cannot be resolved, has
/// nothing to offer or fails is left out and remembered with its reason: the
/// turn always runs.
pub async fn for_turn(app: &App, settings: &Settings) -> TurnTools {
    let mut turn = TurnTools::default();
    let mut listed = Vec::new();
    for cfg in settings.chat_tools.iter().filter(|c| c.enabled) {
        let cached = load_cache(app).remove(&cfg.id);
        let known = match cached {
            Some(d) if d.status == "ok" || now_ms() - d.checked_at < RETRY_AFTER_MS => d,
            _ => discover(app, cfg).await,
        };
        let Ok(server) = resolve(cfg, &home()) else {
            continue;
        };
        if known.status != "ok" {
            continue;
        }
        let key = server_key(cfg);
        let (mut reads, mut writes) = (Vec::new(), Vec::new());
        for tool in &known.tools {
            match classify(&tool.name, tool.read_only, &cfg.overrides) {
                Kind::Read => reads.push(tool.name.clone()),
                Kind::Write => writes.push(tool.name.clone()),
            }
        }
        turn.allowed
            .extend(reads.iter().map(|t| claude_tool_name(&key, t)));
        if transport(&server) == Transport::Stdio {
            turn.codex
                .push((sanitize(&cfg.id), server.clone(), reads.clone()));
        }
        turn.servers.insert(key, server);
        listed.push((cfg.id.clone(), reads, writes));
    }
    if !listed.is_empty() {
        turn.prompt = tools_prompt(&listed);
    }
    turn.listed = listed;
    turn
}

/// The connected tools a task run gets: every enabled one that an earlier
/// look found reachable. Only the stored discovery is read -- a run never
/// starts a probe -- so a tool nobody has opened in Settings or used in a
/// chat turn yet is left out until it has been seen once.
pub fn for_task(app: &App, settings: &Settings) -> TurnTools {
    let mut turn = TurnTools::default();
    let cache = load_cache(app);
    for cfg in settings.chat_tools.iter().filter(|c| c.enabled) {
        let Some(known) = cache.get(&cfg.id).filter(|d| d.status == "ok") else {
            continue;
        };
        let Ok(server) = resolve(cfg, &home()) else {
            continue;
        };
        let key = server_key(cfg);
        let (mut reads, mut writes) = (Vec::new(), Vec::new());
        for tool in &known.tools {
            match classify(&tool.name, tool.read_only, &cfg.overrides) {
                Kind::Read => reads.push(tool.name.clone()),
                Kind::Write => writes.push(tool.name.clone()),
            }
        }
        turn.allowed
            .extend(reads.iter().map(|t| claude_tool_name(&key, t)));
        turn.servers.insert(key, server);
        turn.listed.push((cfg.id.clone(), reads, writes));
    }
    turn
}

/// `-c mcp_servers.<name>.*` flags for Codex: only the read tools are enabled.
pub fn codex_flags(turn: &TurnTools) -> Vec<String> {
    let mut flags = Vec::new();
    for (name, server, reads) in &turn.codex {
        flags.extend(harness::codex_mcp_flags(name, server));
        flags.push("-c".to_string());
        flags.push(format!("mcp_servers.{name}.enabled_tools={}", json!(reads)));
    }
    flags
}
