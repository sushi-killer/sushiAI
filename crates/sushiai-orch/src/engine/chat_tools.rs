//! Connected tools of the orchestrator chat: MCP servers the owner toggles in
//! Settings. A server definition is read at turn time from where the owner
//! already keeps it (`~/.claude.json`, an installed Claude Code plugin), so no
//! secret is copied into orchd's settings. Read tools are handed to the turn;
//! a write tool is never callable by the model: it proposes the call in a
//! `sushi-action` block and the owner's OK runs exactly that call.

use super::*;
use serde::Serialize;
use std::collections::BTreeMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

/// How long a server gets to start and answer one request.
const MCP_TIMEOUT: Duration = Duration::from_secs(30);
/// A failed discovery is not retried on every turn, only after this long or
/// when the owner refreshes the list in Settings.
const RETRY_AFTER_MS: i64 = 10 * 60 * 1000;
const READ_VERBS: [&str; 7] = ["get", "list", "search", "read", "query", "find", "fetch"];
const WRITE_VERBS: [&str; 18] = [
    "add", "create", "update", "delete", "remove", "send", "write", "post", "edit", "set",
    "rename", "copy", "move", "insert", "append", "clear", "forget", "upload",
];

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
fn home() -> PathBuf {
    match std::env::var("SUSHIAI_MCP_HOME") {
        Ok(fixture) if !fixture.is_empty() => PathBuf::from(fixture),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_default()),
    }
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
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
pub(super) fn sanitize(name: &str) -> String {
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

fn claude_tool_name(key: &str, tool: &str) -> String {
    format!("mcp__{}__{}", sanitize(key), sanitize(tool))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Read,
    Write,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Read => "read",
            Kind::Write => "write",
        }
    }
}

/// Read or write: the owner's override first, then the server's
/// `readOnlyHint`, then the tool's name.
pub fn classify(
    name: &str,
    read_only_hint: Option<bool>,
    overrides: &BTreeMap<String, String>,
) -> Kind {
    match overrides.get(name).map(String::as_str) {
        Some("read") => return Kind::Read,
        Some("write") => return Kind::Write,
        _ => {}
    }
    if let Some(read_only) = read_only_hint {
        return if read_only { Kind::Read } else { Kind::Write };
    }
    let lower = name.to_lowercase();
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let reads = tokens.iter().any(|t| READ_VERBS.contains(t));
    let writes = tokens.iter().any(|t| WRITE_VERBS.contains(t));
    if reads && !writes {
        Kind::Read
    } else {
        Kind::Write
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
}

/// What the last look at a server found. `status` is `ok`, `needs-auth`,
/// `failed` or `unavailable` (could not be resolved).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub tools: Vec<KnownTool>,
    pub checked_at: i64,
}

static CACHE_LOCK: StdMutex<()> = StdMutex::new(());

fn cache_path(app: &App) -> PathBuf {
    app.data_dir.join("chat-tools.json")
}

pub(super) fn load_cache(app: &App) -> BTreeMap<String, Discovery> {
    read_json(&cache_path(app))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn update_cache(app: &App, change: impl FnOnce(&mut BTreeMap<String, Discovery>)) {
    let _guard = CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut cache = load_cache(app);
    change(&mut cache);
    let _ = store::write_json_atomic(&cache_path(app), &cache);
}

fn failed(status: &str, reason: impl Into<String>) -> Discovery {
    Discovery {
        status: status.to_string(),
        reason: Some(reason.into()),
        tools: Vec::new(),
        checked_at: now_ms(),
    }
}

/// A newline-delimited JSON-RPC session with a stdio MCP server.
struct StdioSession {
    _child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    lines: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    next_id: u64,
}

impl StdioSession {
    async fn start(server: &serde_json::Value) -> Result<Self, String> {
        let command = server
            .get("command")
            .and_then(|c| c.as_str())
            .ok_or("the server has no command")?;
        let mut cmd = tokio::process::Command::new(command);
        for arg in server
            .get("args")
            .and_then(|a| a.as_array())
            .into_iter()
            .flatten()
        {
            cmd.arg(arg.as_str().unwrap_or_default());
        }
        for (key, value) in server
            .get("env")
            .and_then(|e| e.as_object())
            .into_iter()
            .flatten()
        {
            cmd.env(key, value.as_str().unwrap_or_default());
        }
        cmd.env("PATH", augmented_path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start {command}: {e}"))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut session = StdioSession {
            _child: child,
            stdin,
            lines: tokio::io::BufReader::new(stdout).lines(),
            next_id: 1,
        };
        session
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": {"name": "orchd", "version": "0"},
                }),
            )
            .await?;
        session
            .send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await?;
        Ok(session)
    }

    async fn send(&mut self, message: &serde_json::Value) -> Result<(), String> {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())
    }

    async fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        loop {
            let line = self
                .lines
                .next_line()
                .await
                .map_err(|e| e.to_string())?
                .ok_or("the server closed its output")?;
            let Ok(reply) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if reply.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                let message = error.get("message").and_then(|m| m.as_str());
                return Err(message
                    .unwrap_or("the server returned an error")
                    .to_string());
            }
            return Ok(reply.get("result").cloned().unwrap_or_default());
        }
    }
}

async fn within<T>(
    work: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(MCP_TIMEOUT, work)
        .await
        .map_err(|_| "the server did not answer in time".to_string())?
}

async fn list_stdio(server: &serde_json::Value) -> Result<Vec<KnownTool>, String> {
    within(async {
        let mut session = StdioSession::start(server).await?;
        let listed = session.request("tools/list", json!({})).await?;
        Ok(listed
            .get("tools")
            .and_then(|t| t.as_array())
            .into_iter()
            .flatten()
            .filter_map(|tool| {
                Some(KnownTool {
                    name: tool.get("name")?.as_str()?.to_string(),
                    read_only: tool
                        .pointer("/annotations/readOnlyHint")
                        .and_then(|h| h.as_bool()),
                })
            })
            .collect())
    })
    .await
}

/// The text of a `tools/call` result; an `isError` result is an error.
fn call_text(result: &serde_json::Value) -> Result<String, String> {
    let text = result
        .get("content")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    if result.get("isError").and_then(|e| e.as_bool()) == Some(true) {
        Err(if text.is_empty() {
            "the tool reported an error".to_string()
        } else {
            text
        })
    } else {
        Ok(text)
    }
}

async fn call_stdio(
    server: &serde_json::Value,
    tool: &str,
    args: &serde_json::Value,
) -> Result<String, String> {
    within(async {
        let mut session = StdioSession::start(server).await?;
        let result = session
            .request("tools/call", json!({"name": tool, "arguments": args}))
            .await?;
        call_text(&result)
    })
    .await
}

/// A Claude turn over one server, stopped as soon as it reports which servers
/// it connected: what an http server offers, without a model call.
async fn probe_claude(app: &App, key: &str, server: &serde_json::Value) -> Discovery {
    let dir = app.data_dir.join("chat-tools");
    let _ = std::fs::create_dir_all(&dir);
    let config = dir.join(format!("probe-{}.json", sanitize(key)));
    let _ = store::write_json_atomic(&config, &json!({"mcpServers": {key: server}}));
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
async fn discover(app: &App, cfg: &ChatToolConfig) -> Discovery {
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
fn tools_prompt(listed: &[(String, Vec<String>, Vec<String>)]) -> String {
    let mut text = String::from(
        "\n\nConnected tools: the owner connected these services. Their read tools are available to you; use them to look things up before you answer.",
    );
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
    text.push_str("\nYou can never call a write tool. When the owner asks for one, say in one line what it will do and end your reply with one ```sushi-action block holding JSON: {\"server\": \"the service's id\", \"tool\": \"the write tool's name\", \"args\": {...}, \"summary\": \"what it does, in a few words\", \"target\": \"where: the channel, sheet or project\"}. The app shows the owner an OK card; nothing happens until the owner sends it. If the owner declines, do not propose the same call again. The services' ids, as they appear above: ");
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
    store::write_json_atomic(&config, &json!({"mcpServers": {key.clone(): server}}))
        .map_err(|e| e.to_string())?;
    let argv = send_argv(&config, &claude_tool_name(&key, tool));
    let prompt = format!(
        "Call the tool {tool} exactly once with exactly these JSON arguments and change nothing in them:\n{args}\nThen reply with the tool's result in one or two lines. If it fails, reply with the error."
    );
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
mod tests {
    use super::*;

    #[test]
    fn the_test_profile_fixture_lists_only_example_servers() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../electron/test-fixtures/mcp-home");
        let listed = available_json(&Settings::default(), &fixture);
        let refs: Vec<&str> = listed["servers"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|s| s["ref"].as_str())
            .collect();
        assert_eq!(
            refs,
            ["claude-json:example-notes", "claude-json:example-tracker"]
        );
    }

    #[test]
    fn names_decide_read_or_write_when_nothing_else_does() {
        let none = BTreeMap::new();
        for read in [
            "google_sheets_read_data",
            "find",
            "slack_search_public",
            "list_watches",
            "asana_get_task",
        ] {
            assert_eq!(classify(read, None, &none), Kind::Read, "{read}");
        }
        for write in [
            "google_sheets_update_cells",
            "remember",
            "slack_send_message",
            "get_or_create_task",
            "tree",
        ] {
            assert_eq!(classify(write, None, &none), Kind::Write, "{write}");
        }
    }

    #[test]
    fn a_hint_beats_the_name_and_an_override_beats_both() {
        let mut overrides = BTreeMap::new();
        assert_eq!(classify("send_report", Some(true), &overrides), Kind::Read);
        assert_eq!(classify("read_all", Some(false), &overrides), Kind::Write);
        overrides.insert("read_all".to_string(), "read".to_string());
        assert_eq!(classify("read_all", Some(false), &overrides), Kind::Read);
    }

    #[test]
    fn a_plugin_server_keeps_claude_codes_name() {
        let cfg = &ChatToolConfig {
            id: "slack".into(),
            label: "Slack".into(),
            enabled: true,
            server: serde_json::json!({"ref": "plugin:slack/slack"}),
            overrides: Default::default(),
        };
        assert_eq!(server_key(cfg), "plugin:slack:slack");
        assert_eq!(
            claude_tool_name(&server_key(cfg), "search"),
            "mcp__plugin_slack_slack__search"
        );
    }

    #[test]
    fn the_init_event_tells_what_a_server_offers() {
        let line = r#"{"type":"system","subtype":"init","mcp_servers":[{"name":"asana","status":"connected"}],"tools":["Read","mcp__asana__get_task"]}"#;
        let found = discovery_from_init(line, "asana").unwrap();
        assert_eq!(found.status, "ok");
        assert_eq!(found.tools[0].name, "get_task");
        let line = line.replace("connected", "needs-auth");
        assert_eq!(
            discovery_from_init(&line, "asana").unwrap().status,
            "needs-auth"
        );
    }

    #[test]
    fn the_send_turn_may_use_only_the_one_tool() {
        let argv = send_argv(Path::new("/tmp/c.json"), "mcp__x__y");
        let at = argv.iter().position(|a| a == "--allowedTools").unwrap();
        assert_eq!(argv[at + 1], "mcp__x__y");
        assert_eq!(
            argv[argv.iter().position(|a| a == "--tools").unwrap() + 1],
            ""
        );
    }
}
