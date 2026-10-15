use crate::engine::chat_tools;
use crate::engine::chat_tools::turn::TurnTools;
use crate::engine::messages;
use crate::engine::secrets;
use crate::engine::state::App;
use crate::policy::settings::Settings;
use crate::store::permissions::rules_for;
use crate::task::Task;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

/// The MCP servers a task run starts and the tools it may call without
/// asking: the connected tools the owner enabled (their read tools; every
/// write asks), the repo's own project servers and the task's `mcp`.
pub(in crate::engine) struct TaskTools {
    pub connected: TurnTools,
    /// Connected tools that exist but are off for this task: `(id, label)`.
    pub off: Vec<(String, String)>,
    pub project: BTreeMap<String, serde_json::Value>,
    pub task: BTreeMap<String, serde_json::Value>,
    /// `mcp__server__tool` names the owner allowed for this repo or task.
    pub granted: Vec<String>,
    pub env: HashMap<String, String>,
    pub notes: Vec<String>,
    pub(super) exe_path: String,
}

fn is_full_bridge(def: &serde_json::Value, exe_path: &str) -> bool {
    let Some(command) = def.get("command").and_then(serde_json::Value::as_str) else {
        return false;
    };
    let Some(args) = def.get("args").and_then(serde_json::Value::as_array) else {
        return false;
    };
    (command == exe_path
        || Path::new(command)
            .file_name()
            .is_some_and(|name| name == "sushiai"))
        && args.first().and_then(serde_json::Value::as_str) == Some("mcp")
        && !args.iter().any(|arg| arg.as_str() == Some("--task"))
}

fn servers_of(file: &Path) -> BTreeMap<String, serde_json::Value> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("mcpServers").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn project_environment(app: &App, task: &Task) -> (bool, HashMap<String, String>) {
    let secrets = app.secrets.read().unwrap();
    let project_id = secrets::project_for_repo(&secrets, &task.repo, task.project_id.as_deref())
        .ok()
        .flatten();
    let env = project_id.as_ref().map_or_else(HashMap::new, |id| {
        let mut env = secrets.projects.get(id).cloned().unwrap_or_default();
        env.extend(secrets.project_mcp.get(id).cloned().unwrap_or_default());
        env
    });
    (project_id.is_some(), env)
}

/// Reads what is already known about the connected tools; it never starts a
/// server or a probe (that happens on the Settings screen and in chat turns),
/// so a run is not slowed or disturbed by a tool that hangs.
pub(in crate::engine) fn task_tools(
    app: &App,
    settings: &Settings,
    task: &Task,
    worktree: &Path,
) -> TaskTools {
    let rules = rules_for(&app.data_dir, &task.repo, &task.id);
    let enabled_here: Vec<&str> = rules
        .iter()
        .filter_map(|r| r.strip_prefix("enable:"))
        .collect();
    let (project_known, env) = project_environment(app, task);
    let mut project = if project_known {
        BTreeMap::new()
    } else {
        servers_of(&worktree.join(".mcp.json"))
    };
    let mut task_servers = servers_of(&app.store.task_dir(&task.id).join("mcp.json"));
    let enable = settings
        .mcp_enable
        .get(&task.repo)
        .cloned()
        .unwrap_or_default();
    let disable = settings
        .mcp_disable
        .get(&task.repo)
        .cloned()
        .unwrap_or_default();
    let mut notes = Vec::new();
    for name in &enable {
        if !settings.chat_tools.iter().any(|cfg| cfg.id == *name) {
            notes.push(format!(
                "MCP: ignored unknown name {name:?} in mcpEnable for {}",
                task.repo
            ));
        }
    }
    let mut disabled = HashSet::new();
    for name in &disable {
        if name == messages::SERVER {
            continue;
        }
        let mut known = project.contains_key(name) || task_servers.contains_key(name);
        disabled.insert(name.clone());
        for cfg in &settings.chat_tools {
            let key = chat_tools::server::server_key(cfg);
            if cfg.id == *name || key == *name {
                known = true;
                disabled.insert(cfg.id.clone());
                disabled.insert(key);
            }
        }
        if !known {
            notes.push(format!(
                "MCP: ignored unknown name {name:?} in mcpDisable for {}",
                task.repo
            ));
        }
    }
    disabled.remove(messages::SERVER);
    project.retain(|key, _| !disabled.contains(key));
    task_servers.retain(|key, _| !disabled.contains(key));
    let mut on = settings.clone();
    let mut off = Vec::new();
    for cfg in &mut on.chat_tools {
        cfg.enabled = (enable.contains(&cfg.id) || enabled_here.contains(&cfg.id.as_str()))
            && !disabled.contains(&cfg.id)
            && !disabled.contains(&chat_tools::server::server_key(cfg));
        if !cfg.enabled {
            off.push((cfg.id.clone(), cfg.label.clone()));
        }
    }
    let mut connected = chat_tools::turn::for_task(app, &on);
    // Plugin server keys can differ from the ids used in brief lines.
    connected.listed.retain(|(id, _, _)| {
        let key = on
            .chat_tools
            .iter()
            .find(|cfg| &cfg.id == id)
            .map(chat_tools::server::server_key);
        !key.and_then(|key| connected.servers.get(&key))
            .is_some_and(|def| is_full_bridge(def, &app.exe_path))
    });
    TaskTools {
        connected,
        off,
        project,
        task: task_servers,
        granted: rules
            .iter()
            .filter_map(|r| r.strip_prefix("tool:"))
            .map(str::to_string)
            .collect(),
        env,
        exe_path: app.exe_path.clone(),
        notes,
    }
}

/// A key in a task's server definition that also hands the server to Codex.
const CODEX_MARK: &str = "codex";

fn without_mark(def: &serde_json::Value) -> serde_json::Value {
    let mut def = def.clone();
    if let Some(obj) = def.as_object_mut() {
        obj.remove(CODEX_MARK);
    }
    def
}

impl TaskTools {
    /// The run's `mcp.json` content: the project's servers, the connected
    /// tools, the task's own (which win over both), then `messages_server`.
    pub fn mcp_config(&self, messages_server: &serde_json::Value) -> serde_json::Value {
        let mut servers = serde_json::Map::new();
        for (key, def) in &self.project {
            if !is_full_bridge(def, &self.exe_path) {
                servers.insert(key.clone(), def.clone());
            }
        }
        for (key, def) in &self.connected.servers {
            if !is_full_bridge(def, &self.exe_path) {
                servers.entry(key.clone()).or_insert_with(|| def.clone());
            }
        }
        for (key, def) in &self.task {
            if !is_full_bridge(def, &self.exe_path) {
                servers.insert(key.clone(), without_mark(def));
            }
        }
        servers.insert(messages::SERVER.to_string(), messages_server.clone());
        fn resolve(value: &mut serde_json::Value, env: &HashMap<String, String>) {
            match value {
                serde_json::Value::String(text) => {
                    let mut result = text.clone();
                    for (name, secret) in env {
                        result = result.replace(&format!("${{{name}}}"), secret);
                    }
                    *text = result;
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        resolve(item, env);
                    }
                }
                serde_json::Value::Object(items) => {
                    for item in items.values_mut() {
                        resolve(item, env);
                    }
                }
                _ => {}
            }
        }
        let mut config = json!({"mcpServers": servers});
        resolve(&mut config, &self.env);
        config
    }

    /// The servers a Codex run gets: the task's own that carry
    /// `"codex": true` (the task's `mcp.json` also holds the app's snapshot
    /// of every configured server, so only a marked one was meant for
    /// Codex) and, for host runs only, `messages_server`. Codex has no hook
    /// that asks the owner before a write, so nothing reaches it unmarked, and never the
    /// orchestrator's own bridge.
    pub fn codex_servers(
        &self,
        messages_server: Option<&serde_json::Value>,
    ) -> Vec<(String, serde_json::Value)> {
        let mut servers: Vec<(String, serde_json::Value)> = self
            .task
            .iter()
            .filter(|(key, def)| {
                def.get(CODEX_MARK) == Some(&json!(true))
                    && key.as_str() != messages::SERVER
                    && key.as_str() != "sushiai-orchestrator"
                    && !is_full_bridge(def, &self.exe_path)
            })
            .map(|(key, def)| (key.clone(), without_mark(def)))
            .collect();
        if let Some(messages_server) = messages_server {
            servers.push((messages::SERVER.to_string(), messages_server.clone()));
        }
        servers
    }

    /// `permissions.allow` entries for the run: read tools of the connected
    /// tools and tools the owner allowed. Never a write tool of its own
    /// accord.
    pub fn allowed(&self) -> Vec<String> {
        let mut allow = self.connected.allowed.clone();
        for tool in &self.granted {
            if !allow.contains(tool) {
                allow.push(tool.clone());
            }
        }
        allow
    }

    /// One line per server, for the briefs.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (id, reads, writes) in &self.connected.listed {
            if self
                .connected
                .servers
                .get(id)
                .is_some_and(|def| is_full_bridge(def, &self.exe_path))
            {
                continue;
            }
            let list = |tools: &[String]| {
                if tools.is_empty() {
                    "none".to_string()
                } else {
                    tools.join(", ")
                }
            };
            lines.push(format!(
                "{id}: read tools {}; write tools {} (each write asks the owner before it runs)",
                list(reads),
                list(writes)
            ));
        }
        for (key, def) in &self.project {
            if is_full_bridge(def, &self.exe_path) {
                continue;
            }
            lines.push(format!(
                "{key}: the repository's own MCP server (reads run; a write asks the owner)"
            ));
        }
        for (key, def) in &self.task {
            if is_full_bridge(def, &self.exe_path) {
                continue;
            }
            lines.push(format!(
                "{key}: this task's own MCP server (reads run; a write asks the owner)"
            ));
        }
        lines
    }

    /// What the brief check is told the agent will have and could have.
    pub fn capabilities(&self) -> serde_json::Value {
        json!({
            "builtIn": "read, search and edit files in the worktree, run shell commands, the orchd messaging tools",
            "mcp": self.lines(),
            "connectedButOffForThisTask": self
                .off
                .iter()
                .map(|(id, label)| json!({"id": id, "label": label}))
                .collect::<Vec<_>>(),
        })
    }
}
