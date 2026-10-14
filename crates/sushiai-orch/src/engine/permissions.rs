//! What a task run may use, and what happens when it reaches for more: the
//! tools a run gets (connected tools, the repo's project MCP servers, the
//! task's own), and a call it is not allowed to make held as one owner
//! question instead of an auto-denial. Writes to `.claude/**` cannot pass
//! Claude Code's own protection, not even with a hook `allow`, so an allowed
//! write goes to a staging dir that is copied into place before verify.

use super::chat_tools::{self, Kind, TurnTools};
use super::*;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub(super) const ALLOW_ONCE: &str = "Allow once";
pub(super) const ALLOW_ALWAYS: &str = "Always for this repo";
pub(super) const DENY: &str = "Deny";
const OPTIONS: [&str; 3] = [ALLOW_ONCE, ALLOW_ALWAYS, DENY];

/// Where an allowed write to a protected path is made; inside the worktree,
/// copied to the real path by [`apply_staged`] and then removed.
pub(super) const STAGING_DIR: &str = ".orchd-staging";

const EDIT_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];
const WEB_TOOLS: [&str; 2] = ["WebFetch", "WebSearch"];
const MAX_DETAIL_CHARS: usize = 400;

// -- grants ------------------------------------------------------------------

/// Rules the owner granted: per repo (`Always for this repo`, inherited by
/// every later task) and per task (`enable:<tool id>` from the brief check, an
/// answer to a denial after the run). A rule is `tool:<claude tool name>`,
/// `path:<repo-relative path>` (a trailing `/**` covers a directory) or
/// `enable:<connected tool id>`.
#[derive(Default, Serialize, Deserialize)]
struct Grants {
    #[serde(default)]
    repos: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    tasks: BTreeMap<String, Vec<String>>,
}

static GRANTS_LOCK: StdMutex<()> = StdMutex::new(());

fn grants_path(app: &App) -> PathBuf {
    app.data_dir.join("permissions.json")
}

fn load_grants(app: &App) -> Grants {
    std::fs::read_to_string(grants_path(app))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub(super) enum Scope<'a> {
    Repo(&'a str),
    Task(&'a str),
}

pub(super) fn add_rule(app: &App, scope: Scope, rule: &str) {
    let _guard = GRANTS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut grants = load_grants(app);
    let list = match scope {
        Scope::Repo(repo) => grants.repos.entry(repo.to_string()).or_default(),
        Scope::Task(id) => grants.tasks.entry(id.to_string()).or_default(),
    };
    if !list.iter().any(|r| r == rule) {
        list.push(rule.to_string());
        let _ = store::write_json_atomic(&grants_path(app), &grants);
    }
}

/// Every rule that applies to `task`: its repo's, then its own.
pub(super) fn rules_for(app: &App, repo: &str, task_id: &str) -> Vec<String> {
    let grants = load_grants(app);
    grants
        .repos
        .get(repo)
        .into_iter()
        .chain(grants.tasks.get(task_id))
        .flatten()
        .cloned()
        .collect()
}

fn rule_covers(rule: &str, wanted: &str) -> bool {
    rule == wanted
        || rule
            .strip_suffix("/**")
            .is_some_and(|dir| wanted.starts_with(&format!("{dir}/")))
}

fn granted(rules: &[String], wanted: &str) -> bool {
    rules.iter().any(|r| rule_covers(r, wanted))
}

// -- the tools of a run ------------------------------------------------------

/// The MCP servers a task run starts and the tools it may call without
/// asking: the connected tools the owner enabled (their read tools; every
/// write asks), the repo's own project servers and the task's `mcp`.
pub(super) struct TaskTools {
    pub connected: TurnTools,
    /// Connected tools that exist but are off for this task: `(id, label)`.
    pub off: Vec<(String, String)>,
    pub project: BTreeMap<String, serde_json::Value>,
    pub task: BTreeMap<String, serde_json::Value>,
    /// `mcp__server__tool` names the owner allowed for this repo or task.
    pub granted: Vec<String>,
    pub env: HashMap<String, String>,
    pub notes: Vec<String>,
    exe_path: String,
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

/// Reads what is already known about the connected tools; it never starts a
/// server or a probe (that happens on the Settings screen and in chat turns),
/// so a run is not slowed or disturbed by a tool that hangs.
pub(super) fn task_tools(
    app: &App,
    settings: &Settings,
    task: &Task,
    worktree: &Path,
) -> TaskTools {
    let rules = rules_for(app, &task.repo, &task.id);
    let enabled_here: Vec<&str> = rules
        .iter()
        .filter_map(|r| r.strip_prefix("enable:"))
        .collect();
    let mut on = settings.clone();
    let mut off = Vec::new();
    for cfg in on.chat_tools.iter_mut() {
        if enabled_here.contains(&cfg.id.as_str()) {
            cfg.enabled = true;
        }
        if !cfg.enabled {
            off.push((cfg.id.clone(), cfg.label.clone()));
        }
    }
    let secrets = app.secrets.read().unwrap();
    let project_id = secrets::project_for_repo(&secrets, &task.repo, task.project_id.as_deref())
        .ok()
        .flatten();
    let env = project_id.as_ref().map_or_else(HashMap::new, |id| {
        let mut env = secrets.projects.get(id).cloned().unwrap_or_default();
        env.extend(secrets.project_mcp.get(id).cloned().unwrap_or_default());
        env
    });
    drop(secrets);
    let mut project = if project_id.is_some() {
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
            let key = chat_tools::server_key(cfg);
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
            && !disabled.contains(&chat_tools::server_key(cfg));
        if !cfg.enabled {
            off.push((cfg.id.clone(), cfg.label.clone()));
        }
    }
    let mut connected = chat_tools::for_task(app, &on);
    // Plugin server keys can differ from the ids used in brief lines.
    connected.listed.retain(|(id, _, _)| {
        let key = on
            .chat_tools
            .iter()
            .find(|cfg| &cfg.id == id)
            .map(chat_tools::server_key);
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

// -- the gate ----------------------------------------------------------------

/// What a call is about: a repo-relative path or a tool name.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) enum Subject {
    Path(String),
    Tool(String),
}

impl Subject {
    pub fn rule(&self) -> String {
        match self {
            Subject::Path(p) => format!("path:{p}"),
            Subject::Tool(t) => format!("tool:{t}"),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Subject::Path(n) | Subject::Tool(n) => n,
        }
    }
}

enum Gate {
    /// Not orchd's call: Claude Code decides as it would have.
    Pass,
    Allow,
    /// A protected path the owner allowed: the write goes to staging.
    Stage(String),
    Ask(Subject),
    /// Decided by the settings; the call is refused with this reason.
    Deny(String),
}

/// `a/./b/../c` without touching the disk.
fn normalized(rel: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn is_protected(rel: &str) -> bool {
    rel == ".claude" || rel.starts_with(".claude/")
}

fn edit_target(ctx: &HookContext, input: Option<&serde_json::Value>) -> Option<String> {
    let file = ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|k| input.and_then(|i| i.get(k)).and_then(|v| v.as_str()))?;
    relative_to_worktree(&ctx.worktree, file).map(|rel| normalized(&rel))
}

fn escapes_worktree(ctx: &HookContext, input: Option<&serde_json::Value>) -> bool {
    let file = ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|k| input.and_then(|i| i.get(k)).and_then(|v| v.as_str()))
        .unwrap_or_default();
    // Only an absolute path: a relative one may start with `~` or resolve
    // against another directory.
    if !Path::new(file).is_absolute() {
        return true;
    }
    let Some(rel) = relative_to_worktree(&ctx.worktree, file) else {
        return true;
    };
    // No `..` and no symlink on the way: a link may point out of the
    // worktree, also to a file that does not exist yet.
    let mut at = ctx.worktree.clone();
    for part in rel.split('/') {
        if part == ".." {
            return true;
        }
        at.push(part);
        if at
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return true;
        }
    }
    false
}

/// `mcp__server__tool` of a connected tool, read or write; other servers are
/// judged by the tool's name alone.
fn mcp_kind(app: &App, tool_name: &str) -> Kind {
    let Some((server, tool)) = tool_name
        .strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
    else {
        return Kind::Write;
    };
    let settings = app.settings.read().unwrap().clone();
    let cfg = settings
        .chat_tools
        .iter()
        .find(|c| chat_tools::sanitize(&chat_tools::server_key(c)) == server);
    let overrides = cfg.map(|c| c.overrides.clone()).unwrap_or_default();
    let known = cfg
        .and_then(|c| chat_tools::load_cache(app).remove(&c.id))
        .and_then(|d| {
            d.tools
                .into_iter()
                .find(|t| chat_tools::sanitize(&t.name) == tool)
        });
    match known {
        Some(t) => chat_tools::classify(&t.name, t.read_only, &overrides),
        None => chat_tools::classify(tool, None, &overrides),
    }
}

fn gate_for(app: &App, ctx: &HookContext, tool: &str, input: Option<&serde_json::Value>) -> Gate {
    let rules = rules_for(app, &ctx.repo, &ctx.task_id);
    if EDIT_TOOLS.contains(&tool) {
        let Some(rel) = edit_target(ctx, input) else {
            return Gate::Pass;
        };
        if !is_protected(&rel) {
            // Headless Claude Code on Linux refused edits of files in the
            // task's own worktree; an explicit allow keeps that from parking
            // the task on an owner question. A path that climbs out is not
            // the worktree's and stays with Claude's own rules.
            return if escapes_worktree(ctx, input) {
                Gate::Pass
            } else {
                Gate::Allow
            };
        }
        let subject = Subject::Path(rel.clone());
        let ran = ctx.run_grants.lock().unwrap().contains(&subject.rule());
        return if ran || granted(&rules, &subject.rule()) {
            Gate::Stage(rel)
        } else {
            Gate::Ask(subject)
        };
    }
    if tool.starts_with(&format!("mcp__{}__", messages::SERVER)) {
        return Gate::Pass;
    }
    let subject = Subject::Tool(tool.to_string());
    if tool.starts_with("mcp__") {
        if mcp_kind(app, tool) == Kind::Read
            || granted(&rules, &subject.rule())
            || ctx.run_grants.lock().unwrap().contains(&subject.rule())
        {
            return Gate::Allow;
        }
        return Gate::Ask(subject);
    }
    if WEB_TOOLS.contains(&tool) {
        if granted(&rules, &subject.rule())
            || ctx.run_grants.lock().unwrap().contains(&subject.rule())
            || web_allowed(&app.settings.read().unwrap().allowed_domains, tool, input)
        {
            return Gate::Allow;
        }
        return Gate::Deny(format!(
            "{tool} is limited to the allowed domains in the settings and this call is outside them. Meet the task without it, or report `blocked` and say what you could not read."
        ));
    }
    Gate::Pass
}

/// Whether the allowed domains cover a web call: `*` covers everything, a
/// `WebFetch` is covered by its host, and a `WebSearch` has no host.
fn web_allowed(domains: &[String], tool: &str, input: Option<&serde_json::Value>) -> bool {
    if domains.iter().any(|d| d.trim() == "*") {
        return true;
    }
    let host = (tool == "WebFetch")
        .then(|| input.and_then(|i| i.get("url")).and_then(|u| u.as_str()))
        .flatten()
        .and_then(|url| url::Url::parse(url).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
    let Some(host) = host else {
        return false;
    };
    domains.iter().any(|d| {
        let d = d.trim().trim_start_matches("*.").to_ascii_lowercase();
        !d.is_empty() && (host == d || host.ends_with(&format!(".{d}")))
    })
}

fn decision(kind: &str, reason: &str) -> serde_json::Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": kind,
            "permissionDecisionReason": reason,
        }
    })
}

fn staging_reason(ctx: &HookContext, rel: &str) -> String {
    let staged = ctx.worktree.join(STAGING_DIR).join(rel);
    format!(
        "The owner allowed this write, but Claude Code blocks {rel} for agents. Make the same change at {} instead (if {rel} exists, copy it there first, then edit the copy): orchd copies it to {rel} before verification. Do not retry {rel} itself.",
        staged.display()
    )
}

/// The owner's answer to a permission question.
#[derive(Debug, PartialEq)]
pub(super) enum Choice {
    Once,
    Always,
    Deny(Option<String>),
}

pub(super) fn parse_choice(answer: &str) -> Choice {
    if is_option(answer, ALLOW_ONCE) {
        Choice::Once
    } else if is_option(answer, ALLOW_ALWAYS) {
        Choice::Always
    } else if is_option(answer, DENY) {
        Choice::Deny(None)
    } else {
        Choice::Deny(Some(answer.trim().to_string()).filter(|a| !a.is_empty()))
    }
}

pub(super) fn question_text(subjects: &[Subject]) -> String {
    let names: Vec<&str> = subjects.iter().map(Subject::name).collect();
    format!("Allow {} for this task?", names.join(", "))
}

pub(super) fn permission_question(subjects: &[Subject], detail: &str) -> Question {
    let mut text = question_text(subjects);
    if !detail.is_empty() {
        text.push('\n');
        text.push_str(detail);
    }
    Question::new(
        text,
        OPTIONS.iter().map(|o| o.to_string()).collect(),
        QuestionKind::Permission,
        AskedBy::Implement,
    )
}

/// The `Permission:` decision line for one answered subject.
pub(super) fn answer_line(subject: &Subject, choice: &Choice) -> String {
    let what = match choice {
        Choice::Once => "allowed once",
        Choice::Always => "allowed for this repo",
        Choice::Deny(_) => "denied",
    };
    format!("Permission: {what}: {}", subject.name())
}

/// Holds the call: the task shows one question, the run waits, and the answer
/// comes back through `task.answer`. `None` when the run ended first.
async fn ask(app: &Arc<App>, ctx: &HookContext, subject: &Subject, detail: &str) -> Option<String> {
    let question = permission_question(std::slice::from_ref(subject), detail);
    let (tx, rx) = oneshot::channel();
    app.permission_waits
        .lock()
        .unwrap()
        .insert(ctx.task_id.clone(), tx);
    // The harness is silent while it waits for us: the stall clock pauses.
    let _paused = ctx.hook_running.start();
    if let Err(e) = app.update_task(&ctx.task_id, |task| {
        task.question = Some(question.clone());
        task.status = TaskStatus::Waiting;
    }) {
        eprintln!("orch: updating task {} failed: {e}", ctx.task_id);
        app.permission_waits.lock().unwrap().remove(&ctx.task_id);
        return None;
    }
    let reply = tokio::select! {
        _ = ctx.cancel.cancelled() => None,
        a = rx => a.ok(),
    };
    let evidence = reply.as_ref().and_then(|r| r.default_evidence.as_deref());
    let answer = reply.as_ref().map(|r| r.text.clone());
    app.permission_waits.lock().unwrap().remove(&ctx.task_id);
    // `task.answer` marks an agent's answer (it may only deny here).
    let caller = app
        .take_agent_answer(&ctx.task_id)
        .unwrap_or(AnsweredBy::Owner);
    if let Err(e) = app.update_task(&ctx.task_id, |task| {
        if task
            .question
            .as_ref()
            .is_some_and(|q| q.text == question.text)
        {
            if let Some(answer) = &answer {
                task.question = None;
                record_answer_for(task, Some(&question), answer, caller, evidence, None);
                task.status = TaskStatus::Running;
            } else if app
                .controls
                .lock()
                .unwrap()
                .get(&ctx.task_id)
                .is_none_or(|ctrl| ctrl.cancel.is_cancelled())
            {
                task.status = app.cancelled_status(&task.status);
                if task.status == TaskStatus::Stopped {
                    app.close_stopped_task(task);
                }
            } else {
                // The hook token also fires when a run ends while its task
                // loop continues; only the expired permission question closes.
                task.question = None;
            }
        }
    }) {
        eprintln!("orch: updating task {} failed: {e}", ctx.task_id);
    }
    answer
}

fn call_detail(tool: &str, input: Option<&serde_json::Value>) -> String {
    match (tool, input) {
        (_, None) => String::new(),
        (t, Some(input)) if t.starts_with("mcp__") || WEB_TOOLS.contains(&t) => truncate_chars(
            &format!("Call: {input}"),
            MAX_DETAIL_CHARS,
        ),
        _ => "Claude Code protects this path for agents; the file is written in a staging directory and copied into place once you allow it.".to_string(),
    }
}

/// `hook.edit`'s permission half: the decision for one tool call, `{}` when
/// the call is not orchd's to judge.
pub(super) async fn decide_call(
    app: &Arc<App>,
    ctx: &HookContext,
    payload: &serde_json::Value,
) -> serde_json::Value {
    let Some(tool) = payload.get("tool_name").and_then(|t| t.as_str()) else {
        return json!({});
    };
    let input = payload.get("tool_input");
    let first = gate_for(app, ctx, tool, input);
    let subject = match first {
        Gate::Pass => return json!({}),
        Gate::Allow => return decision("allow", "allowed for this run"),
        Gate::Stage(rel) => {
            ctx.staged.lock().unwrap().insert(rel.clone());
            return decision("deny", &staging_reason(ctx, &rel));
        }
        Gate::Deny(reason) => return decision("deny", &reason),
        Gate::Ask(subject) => subject,
    };
    // One question at a time; a call that waited behind another may find its
    // subject already answered.
    let _one = ctx.ask_lock.lock().await;
    match gate_for(app, ctx, tool, input) {
        Gate::Allow => return decision("allow", "allowed for this run"),
        Gate::Stage(rel) => {
            ctx.staged.lock().unwrap().insert(rel.clone());
            return decision("deny", &staging_reason(ctx, &rel));
        }
        Gate::Deny(reason) => return decision("deny", &reason),
        _ => {}
    }
    if ctx.run_denied.lock().unwrap().contains(&subject.rule()) {
        return decision("deny", &denied_reason(&subject, None));
    }
    let detail = call_detail(tool, input);
    let Some(answer) = ask(app, ctx, &subject, &detail).await else {
        return decision("deny", "The run was stopped before the owner answered.");
    };
    let choice = parse_choice(&answer);
    ctx.handled
        .lock()
        .unwrap()
        .insert(subject.name().to_string());
    let line = answer_line(&subject, &choice);
    append_decision(app, &ctx.task_id, line);
    match choice {
        Choice::Deny(note) => {
            ctx.run_denied.lock().unwrap().insert(subject.rule());
            decision("deny", &denied_reason(&subject, note.as_deref()))
        }
        Choice::Once | Choice::Always => {
            if choice == Choice::Always {
                add_rule(app, Scope::Repo(&ctx.repo), &subject.rule());
            }
            // A write to an outside service asks again on the next call
            // unless the owner chose Always; a path or a read-like tool stays
            // allowed for the rest of the run.
            let single_call = matches!(&subject, Subject::Tool(t) if t.starts_with("mcp__"))
                && choice == Choice::Once;
            match &subject {
                Subject::Path(rel) => {
                    ctx.run_grants.lock().unwrap().insert(subject.rule());
                    ctx.staged.lock().unwrap().insert(rel.clone());
                    decision("deny", &staging_reason(ctx, rel))
                }
                Subject::Tool(_) => {
                    if !single_call {
                        ctx.run_grants.lock().unwrap().insert(subject.rule());
                    }
                    decision("allow", "the owner allowed it")
                }
            }
        }
    }
}

fn denied_reason(subject: &Subject, note: Option<&str>) -> String {
    let mut text = format!(
        "The owner denied {}. Do not try it again; meet the task another way, or report `blocked` and say what you could not do.",
        subject.name()
    );
    if let Some(note) = note {
        text.push_str(&format!(" The owner said: {note}"));
    }
    text
}

fn append_decision(app: &App, task_id: &str, line: String) {
    if let Err(e) = app.update_task(task_id, |task| task.decisions.push(line)) {
        eprintln!("orch: updating task {task_id} failed: {e}");
    }
}

// -- staging -----------------------------------------------------------------

/// Copies what the agent staged at an approved path to its real place, and
/// clears the staging dir. Returns the paths it applied.
pub(super) fn apply_staged(ctx: &HookContext) -> Vec<String> {
    let staging = ctx.worktree.join(STAGING_DIR);
    let mut applied = Vec::new();
    let staged: BTreeSet<String> = ctx.staged.lock().unwrap().clone();
    for rel in staged {
        let from = staging.join(&rel);
        let to = ctx.worktree.join(&rel);
        if !from.is_file() {
            continue;
        }
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::copy(&from, &to).is_ok() {
            applied.push(rel);
        }
    }
    applied
}

/// Final apply at the end of a run: what was staged lands, the dir goes.
pub(super) fn finish_staging(ctx: &HookContext) -> Vec<String> {
    let applied = apply_staged(ctx);
    let _ = std::fs::remove_dir_all(ctx.worktree.join(STAGING_DIR));
    applied
}

pub(super) fn staging_lines(applied: &[String]) -> Vec<String> {
    applied
        .iter()
        .map(|rel| format!("Permission: copied the staged {rel} into place"))
        .collect()
}

// -- after the run -----------------------------------------------------------

/// The calls a finished run was refused that orchd never decided: the tool or
/// path, so they can be asked about once instead of retried.
/// A write the run was refused outside its worktree, into a temp directory:
/// scratch work that needs no owner and loses nothing.
fn scratch_file<'a>(worktree: &Path, denial: &'a harness::PermissionDenial) -> Option<&'a str> {
    let file = ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|k| denial.input.get(k).and_then(|v| v.as_str()))?;
    // The system temp dir and the POSIX ones, each also by its resolved path
    // (a symlinked /tmp resolves elsewhere on some systems).
    let roots = [std::env::temp_dir(), "/tmp".into(), "/var/tmp".into()];
    let in_temp = roots.iter().any(|root| {
        Path::new(file).starts_with(root)
            || root
                .canonicalize()
                .is_ok_and(|real| Path::new(file).starts_with(real))
    });
    (EDIT_TOOLS.contains(&denial.tool.as_str())
        && Path::new(file).is_absolute()
        && relative_to_worktree(worktree, file).is_none()
        && in_temp)
        .then_some(file)
}

/// The decision lines for scratch writes the run was refused: recorded for
/// the next attempt, which works inside its worktree instead.
pub(super) fn scratch_lines(worktree: &Path, outcome: &harness::RunOutcome) -> Vec<String> {
    let files: BTreeSet<&str> = outcome
        .permission_denials
        .iter()
        .filter_map(|d| scratch_file(worktree, d))
        .collect();
    files
        .into_iter()
        .map(|file| {
            format!(
                "Orchestrator: a write to {file} outside the worktree was refused; keep scratch files inside the worktree and out of the commit"
            )
        })
        .collect()
}

pub(super) fn unhandled_denials(
    handled: &BTreeSet<String>,
    worktree: &Path,
    outcome: &harness::RunOutcome,
    protected: &[String],
) -> Vec<Subject> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for denial in &outcome.permission_denials {
        let tool = denial.tool.as_str();
        if scratch_file(worktree, denial).is_some() {
            continue;
        }
        let subject = if EDIT_TOOLS.contains(&tool) {
            let input = Some(&denial.input);
            let file = ["file_path", "notebook_path", "path"]
                .iter()
                .find_map(|k| input.and_then(|i| i.get(k)).and_then(|v| v.as_str()));
            match file {
                Some(f) => Subject::Path(
                    relative_to_worktree(worktree, f)
                        .map(|rel| normalized(&rel))
                        .unwrap_or_else(|| f.to_string()),
                ),
                None => continue,
            }
        } else if (tool.starts_with("mcp__") && !tool.starts_with("mcp__sushiai-messages__"))
            || WEB_TOOLS.contains(&tool)
        {
            Subject::Tool(tool.to_string())
        } else {
            continue;
        };
        // A planner test or a check-run file is restored by orchd, never asked
        // about: the owner has nothing to allow.
        let is_protected = matches!(&subject, Subject::Path(rel) if protected.contains(rel));
        if is_protected || handled.contains(subject.name()) || !seen.insert(subject.rule()) {
            continue;
        }
        out.push(subject);
    }
    out
}

/// Whether every subject already has an answer recorded on the task: the
/// same cause again after the owner has spoken.
pub(super) fn already_answered(task: &Task, subjects: &[Subject]) -> bool {
    subjects.iter().all(|s| {
        [
            answer_line(s, &Choice::Once),
            answer_line(s, &Choice::Always),
            answer_line(s, &Choice::Deny(None)),
        ]
        .iter()
        .any(|line| task.decisions.contains(line))
    })
}

/// Applies the owner's answer to the subjects a finished run was refused.
pub(super) fn apply_answer(app: &App, task: &mut Task, subjects: &[Subject], answer: &str) {
    let choice = parse_choice(answer);
    for subject in subjects {
        task.decisions.push(answer_line(subject, &choice));
        match &choice {
            Choice::Once => add_rule(app, Scope::Task(&task.id), &subject.rule()),
            Choice::Always => add_rule(app, Scope::Repo(&task.repo), &subject.rule()),
            Choice::Deny(_) => task
                .decisions
                .push(format!("Orchestrator: {}", denied_reason(subject, None))),
        }
    }
}

// -- preflight ----------------------------------------------------------------

const OWNER_ACTION: &str = "I will do it: drop the criterion";
const CONTINUE_ANYWAY: &str = "Continue anyway";

pub(super) fn missing_tool_question(task: &Task, missing: &MissingTool) -> Question {
    let criterion = task
        .criteria
        .get(missing.criterion)
        .map(String::as_str)
        .unwrap_or("");
    let mut options = Vec::new();
    if let Some(id) = &missing.tool_id {
        options.push(format!("Enable {id} for this task"));
    }
    options.push(OWNER_ACTION.to_string());
    options.push(CONTINUE_ANYWAY.to_string());
    Question::new(
        format!(
            "Criterion {} needs {}, which this task will not have: {criterion}",
            missing.criterion, missing.capability
        ),
        options,
        QuestionKind::Permission,
        AskedBy::Brief,
    )
}

/// Applies the answer to the preflight question.
pub(super) fn apply_missing_answer(
    app: &App,
    task: &mut Task,
    missing: &MissingTool,
    answer: &str,
) {
    let enable = missing
        .tool_id
        .as_deref()
        .filter(|id| is_option(answer, &format!("Enable {id} for this task")));
    if let Some(id) = enable {
        add_rule(app, Scope::Task(&task.id), &format!("enable:{id}"));
        task.decisions
            .push(format!("Brief check: {id} enabled for this task"));
    } else if is_option(answer, OWNER_ACTION) {
        let text = task
            .criteria
            .get(missing.criterion)
            .cloned()
            .unwrap_or_default();
        let settings = app.settings.read().unwrap().clone();
        let origin = origin_of_last_answer(task);
        let _ = drop_criterion(&settings, task, missing.criterion, origin);
        task.decisions.push(format!(
            "Brief check: \"{text}\" needs {}; it is an owner action, not this task's",
            missing.capability
        ));
    } else {
        task.decisions.push(format!(
            "Brief check: the owner let the task run without {}",
            missing.capability
        ));
    }
}

/// `No tool: <capability>`: how an agent reports a criterion it has no tool
/// for (the brief asks for it as the start of the blocked report's question).
pub(super) fn missing_tool_claim(question: &str) -> Option<String> {
    let question = question.trim();
    question
        .get(..8)
        .filter(|prefix| prefix.eq_ignore_ascii_case("no tool:"))?;
    let text = question[8..].trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The block the implement and plan briefs carry.
pub(super) fn tools_block(tools: &TaskTools, planning: bool) -> String {
    let lines = tools.lines();
    let mut text = String::from(
        "## Tools for this task\n\nBesides the file and shell tools, the task agent has these MCP servers:\n",
    );
    if lines.is_empty() {
        text.push_str("- none\n");
    }
    for line in &lines {
        text.push_str(&format!("- {line}\n"));
    }
    for (id, label) in &tools.off {
        text.push_str(&format!(
            "- {id} ({label}): connected but off for this repo\n"
        ));
    }
    if planning {
        text.push_str("\nCriteria may only ask for what these tools can do. A step that needs a service with no tool here belongs to the owner: say so in the criterion instead of asking the agent to do it.\n\n");
    } else {
        text.push_str("\nA call the run may not make (a write to an outside service, a path Claude Code protects) waits for the owner's answer and then goes on or is denied: do not retry it or work around it. If a criterion needs a tool that is not listed here, do not improvise: end with outcome `blocked` and start the report's `question` with `No tool: ` and the capability you lack.\n\n");
    }
    text
}

/// A finished run was refused calls orchd never decided: the attempt ends
/// here, one owner question is raised and no attempt starts on the same
/// cause -- a retry cannot fix a refusal. After the owner's answer the next
/// attempt has what was allowed; the same refusal again stops the task.
#[allow(clippy::too_many_arguments)]
pub(super) async fn stop_for_denials(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    subjects: &[Subject],
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
    permit: &mut Option<SlotPermit>,
) -> LoopSignal {
    let names = question_text(subjects);
    let names = names
        .trim_start_matches("Allow ")
        .trim_end_matches(" for this task?")
        .to_string();
    record_failure(
        task,
        idx,
        FailureKind::Blocked,
        format!("The run was refused {names}; no retry can fix that."),
    );
    if already_answered(task, subjects) {
        task.decisions.push(format!(
            "Permission: {names} was refused again after the owner's answer; stopping instead of another attempt on the same cause"
        ));
        task.mark_failed();
        task.updated_at = now_ms();
        app.save_task_logged(task);
        app.broadcast_task(task);
        return LoopSignal::Stop;
    }
    task.question = Some(permission_question(
        subjects,
        "The run tried this and was refused; nothing was done. Asking once instead of starting another attempt.",
    ));
    task.status = TaskStatus::Waiting;
    task.updated_at = now_ms();
    let Some(answer) = wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await
    else {
        return LoopSignal::Stop;
    };
    apply_answer(app, task, subjects, &answer);
    task.updated_at = now_ms();
    app.save_task_logged(task);
    app.broadcast_task(task);
    LoopSignal::Continue
}

/// The connected tool the owner could switch on for an agent's
/// `No tool: <capability>` report, when one matches.
pub(super) fn tool_to_enable(app: &App, task: &Task, capability: &str) -> Option<String> {
    let settings = app.settings.read().unwrap();
    let lower = capability.to_lowercase();
    settings
        .chat_tools
        .iter()
        .find(|c| {
            !c.enabled
                && !rules_for(app, &task.repo, &task.id).contains(&format!("enable:{}", c.id))
                && (lower.contains(&c.id.to_lowercase()) || lower.contains(&c.label.to_lowercase()))
        })
        .map(|c| c.id.clone())
}

/// The question for an agent's `No tool: <capability>` report; `enable` is
/// the connected tool the owner could switch on, when one matches.
pub(super) fn no_tool_question(capability: &str, enable: Option<&str>) -> Question {
    let mut options: Vec<String> = enable
        .map(|id| format!("Enable {id} for this task"))
        .into_iter()
        .collect();
    options.push(OWNER_ACTION.to_string());
    options.push("stop".to_string());
    Question::new(
        format!("The agent has no tool for {capability}; a retry cannot give it one."),
        options,
        QuestionKind::Permission,
        AskedBy::Implement,
    )
}

/// Applies the owner's answer to a [`no_tool_question`].
pub(super) fn apply_no_tool_answer(app: &App, task: &mut Task, capability: &str, answer: &str) {
    if let Some(id) = answer
        .trim()
        .strip_prefix("Enable ")
        .and_then(|rest| rest.strip_suffix(" for this task"))
    {
        add_rule(app, Scope::Task(&task.id), &format!("enable:{id}"));
        task.decisions
            .push(format!("Permission: {id} enabled for this task"));
    } else {
        task.decisions.push(format!(
            "Orchestrator: the owner will do the part that needs {capability}; do not attempt it, finish the rest and name what is left in your handoff"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_rule_covers_what_is_under_it_and_nothing_else() {
        assert!(rule_covers("path:.claude/**", "path:.claude/a/b.md"));
        assert!(rule_covers("tool:x", "tool:x"));
        assert!(!rule_covers("path:.claude/**", "path:.claudex/a"));
        assert!(!rule_covers("path:.claude/a.md", "path:.claude/b.md"));
    }

    #[test]
    fn paths_are_normalized_before_the_protected_check() {
        assert!(is_protected(&normalized("./a/../.claude/x.md")));
        assert!(is_protected(&normalized(".claude")));
        assert!(!is_protected(&normalized("docs/.claude/x.md")));
        assert!(!is_protected(&normalized(".claudeish/x")));
    }

    #[test]
    fn answers_are_read_as_the_three_options_or_a_denial_with_a_note() {
        assert_eq!(parse_choice("Allow once"), Choice::Once);
        assert_eq!(parse_choice(" always for this repo. "), Choice::Always);
        assert_eq!(parse_choice("Deny"), Choice::Deny(None));
        assert_eq!(
            parse_choice("not that one"),
            Choice::Deny(Some("not that one".to_string()))
        );
    }

    #[test]
    fn the_question_names_the_tool_or_path() {
        let q = permission_question(&[Subject::Path(".claude/x.md".into())], "");
        assert_eq!(q.text, "Allow .claude/x.md for this task?");
        assert_eq!(q.options, vec![ALLOW_ONCE, ALLOW_ALWAYS, DENY]);
        assert_eq!(q.kind, QuestionKind::Permission);
    }

    #[test]
    fn only_a_connected_tool_that_is_off_can_be_offered_for_a_no_tool_claim() {
        let (app, _dir) = crate::engine::test_support::test_app();
        let task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        assert_eq!(tool_to_enable(&app, &task, "move an Asana task"), None);
        app.settings.write().unwrap().chat_tools = vec![serde_json::from_value(json!({
            "id": "asana", "label": "Asana", "enabled": false,
            "server": {"type": "http", "url": "https://example.invalid/mcp"}
        }))
        .unwrap()];
        assert_eq!(
            tool_to_enable(&app, &task, "move an Asana task").as_deref(),
            Some("asana")
        );
        assert_eq!(tool_to_enable(&app, &task, "send an email"), None);
    }

    #[test]
    fn a_no_tool_claim_needs_its_prefix_and_a_capability() {
        assert_eq!(
            missing_tool_claim("No tool: move an Asana task").as_deref(),
            Some("move an Asana task")
        );
        assert_eq!(missing_tool_claim("no tool:  x ").as_deref(), Some("x"));
        assert_eq!(missing_tool_claim("No tool:"), None);
        assert_eq!(missing_tool_claim("Which database?"), None);
        assert_eq!(missing_tool_claim("é"), None);
    }

    fn denial(tool: &str, input: serde_json::Value) -> harness::PermissionDenial {
        harness::PermissionDenial {
            tool: tool.to_string(),
            input,
        }
    }

    #[test]
    fn only_refusals_orchd_did_not_decide_are_asked_about() {
        let outcome = harness::RunOutcome {
            permission_denials: vec![
                denial("mcp__slack__post", json!({})),
                denial("mcp__slack__post", json!({"again": 1})),
                denial("Write", json!({"file_path": "/wt/.claude/a.md"})),
                denial("Write", json!({"file_path": "/wt/.claude/done.md"})),
                denial("Bash", json!({"command": "git push"})),
                denial("mcp__sushiai-messages__peer_send", json!({})),
                denial("Task", json!({})),
            ],
            ..Default::default()
        };
        let handled: BTreeSet<String> = [".claude/done.md".to_string()].into();
        let subjects = unhandled_denials(&handled, Path::new("/wt"), &outcome, &[]);
        assert_eq!(
            subjects,
            vec![
                Subject::Tool("mcp__slack__post".into()),
                Subject::Path(".claude/a.md".into())
            ]
        );
    }

    #[test]
    fn web_calls_are_decided_by_the_allowed_domains() {
        let fetch = |url: &str| json!({"url": url});
        let open = vec!["*".to_string()];
        let some = vec!["docs.rs".to_string(), "*.example.org".to_string()];
        assert!(web_allowed(&open, "WebSearch", None));
        assert!(web_allowed(
            &some,
            "WebFetch",
            Some(&fetch("https://docs.rs/serde/latest"))
        ));
        assert!(web_allowed(
            &some,
            "WebFetch",
            Some(&fetch("https://api.Example.org:8443/x?y#z"))
        ));
        assert!(!web_allowed(
            &some,
            "WebFetch",
            Some(&fetch("https://evil.test/docs.rs"))
        ));
        assert!(!web_allowed(
            &some,
            "WebFetch",
            Some(&fetch("https://notdocs.rs/"))
        ));
        assert!(!web_allowed(
            &some,
            "WebFetch",
            Some(&fetch("https://u@evil.test/"))
        ));
        for url in [
            "https://evil.test\\@docs.rs/",
            "https://evil.test\\.docs.rs/",
            "not a url",
            "ftp://docs.rs/x",
        ] {
            assert!(!web_allowed(&some, "WebFetch", Some(&fetch(url))), "{url}");
        }
        assert!(web_allowed(
            &open,
            "WebFetch",
            Some(&fetch("https://anything.example/x"))
        ));
        assert!(!web_allowed(&some, "WebSearch", None));
        assert!(!web_allowed(
            &[],
            "WebFetch",
            Some(&fetch("https://docs.rs/"))
        ));
    }

    #[test]
    fn a_refused_write_to_a_temp_dir_is_recorded_and_raises_no_question() {
        let outcome = harness::RunOutcome {
            permission_denials: vec![
                denial("Write", json!({"file_path": "/tmp/scratch/notes.txt"})),
                denial("Write", json!({"file_path": "/tmp/scratch/notes.txt"})),
                denial("Write", json!({"file_path": "/etc/hosts"})),
                denial("Write", json!({"file_path": "/wt/docs/b.md"})),
                denial("Bash", json!({"file_path": "/tmp/x"})),
            ],
            ..Default::default()
        };
        let none = BTreeSet::new();
        assert_eq!(
            unhandled_denials(&none, Path::new("/wt"), &outcome, &[]),
            vec![
                Subject::Path("/etc/hosts".into()),
                Subject::Path("docs/b.md".into())
            ]
        );
        let lines = scratch_lines(Path::new("/wt"), &outcome);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("/tmp/scratch/notes.txt"));
    }

    #[test]
    fn a_refused_write_to_a_protected_path_raises_no_question() {
        let outcome = harness::RunOutcome {
            permission_denials: vec![
                denial("Write", json!({"file_path": "/wt/tests/a.rs"})),
                denial("Edit", json!({"file_path": "/wt/docs/b.md"})),
            ],
            ..Default::default()
        };
        let none = BTreeSet::new();
        let protected = vec!["tests/a.rs".to_string()];
        assert_eq!(
            unhandled_denials(&none, Path::new("/wt"), &outcome, &protected),
            vec![Subject::Path("docs/b.md".into())]
        );
        assert_eq!(
            unhandled_denials(&none, Path::new("/wt"), &outcome, &[]).len(),
            2
        );
    }

    #[test]
    fn task_tools_drop_full_bridges_from_every_source() {
        let exe = "/opt/bin/renamed-daemon";
        let full = json!({"command": exe, "args": ["mcp"], "codex": true});
        let readonly =
            json!({"command": "/opt/bin/sushiai", "args": ["mcp", "--read-only"], "codex": true});
        let scoped =
            json!({"command": "sushiai", "args": ["mcp", "--task", "other-task"], "codex": true});
        let unrelated = json!({"command": "sushiai", "args": ["orch", "status"], "codex": true});
        let tools = TaskTools {
            connected: TurnTools {
                servers: [("connected-full".into(), full.clone())].into(),
                listed: vec![("connected-full".into(), vec!["get".into()], vec![])],
                ..Default::default()
            },
            project: [
                ("project-full".into(), full.clone()),
                ("project-readonly".into(), readonly.clone()),
            ]
            .into(),
            task: [
                ("task-full".into(), full),
                ("task-readonly".into(), readonly),
                ("scoped".into(), scoped),
                ("unrelated".into(), unrelated),
                (
                    messages::SERVER.into(),
                    json!({"command": exe, "args": ["mcp"]}),
                ),
            ]
            .into(),
            off: vec![],
            granted: vec![],
            env: HashMap::new(),
            notes: vec![],
            exe_path: exe.into(),
        };
        let messages = json!({"command": exe, "args": ["mcp", "--task", "this-task"]});
        let config = tools.mcp_config(&messages);
        let servers = config["mcpServers"].as_object().unwrap();
        let codex = tools.codex_servers(Some(&messages));
        let lines = tools.lines().join("\n");
        for name in [
            "connected-full",
            "project-full",
            "project-readonly",
            "task-full",
            "task-readonly",
        ] {
            assert!(!servers.contains_key(name), "{config}");
            assert!(!codex.iter().any(|(key, _)| key == name));
            assert!(!lines.contains(name), "{lines}");
        }
        for name in ["scoped", "unrelated"] {
            assert!(servers.contains_key(name), "{config}");
            assert!(codex.iter().any(|(key, _)| key == name));
            assert!(lines.contains(name), "{lines}");
        }
        assert_eq!(servers[messages::SERVER], messages);
        assert_eq!(codex.last().unwrap(), &(messages::SERVER.into(), messages));
    }

    #[test]
    fn a_grant_reaches_the_run_but_a_write_tool_is_never_pre_allowed() {
        let tools = TaskTools {
            connected: TurnTools {
                allowed: vec!["mcp__a__get".into()],
                servers: [("a".to_string(), json!({"command": "x"}))].into(),
                listed: vec![("a".into(), vec!["get".into()], vec!["send".into()])],
                ..Default::default()
            },
            off: vec![("asana".into(), "Asana".into())],
            project: [
                ("a".to_string(), json!({"command": "project"})),
                ("p".to_string(), json!({"command": "p"})),
            ]
            .into(),
            task: [
                ("t".to_string(), json!({"command": "t", "codex": true})),
                // The app's snapshot of a configured server: Claude-only.
                ("p".to_string(), json!({"command": "p"})),
                (
                    "sushiai-orchestrator".to_string(),
                    json!({"command": "o", "codex": true}),
                ),
            ]
            .into(),
            granted: vec!["mcp__a__send".into(), "mcp__a__get".into()],
            env: HashMap::new(),
            exe_path: "sushiai".into(),
            notes: vec![],
        };
        assert_eq!(tools.allowed(), vec!["mcp__a__get", "mcp__a__send"]);
        let config = tools.mcp_config(&json!({"command": "messages"}));
        let servers = config["mcpServers"].as_object().unwrap();
        // The repo's own definition of a server wins over the connected one.
        assert_eq!(servers["a"]["command"], "project");
        assert!(servers.contains_key("p") && servers.contains_key("t"));
        assert!(servers["t"].get("codex").is_none());
        assert_eq!(servers[messages::SERVER]["command"], "messages");
        // Codex gets only the task's own servers and the messages server:
        // it has no hook to ask the owner before a connected tool writes.
        let codex: Vec<String> = tools
            .codex_servers(Some(&json!({"command": "messages"})))
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(codex, vec!["t".to_string(), messages::SERVER.to_string()]);
        let sandboxed: Vec<String> = tools
            .codex_servers(None)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(sandboxed, vec!["t".to_string()]);
        let t = &tools.codex_servers(Some(&json!({})))[0].1;
        assert!(t.get("codex").is_none());
        let block = tools_block(&tools, false);
        assert!(
            block.contains("a: read tools get; write tools send"),
            "{block}"
        );
        assert!(block.contains("No tool: "), "{block}");
        assert!(!tools_block(&tools, true).contains("No tool: "));
    }

    #[tokio::test]
    async fn task_tools_project_id_uses_only_snapshot_and_no_id_falls_back_to_worktree() {
        let (app, dir) = test_app();
        let mut task = task_with_status(TaskStatus::Queued);
        app.store.save_task(&task).unwrap();
        let worktree = dir.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".mcp.json"),
            json!({"mcpServers": {
                "fallback": {"command": "fallback"}
            }})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            app.store.task_dir(&task.id).join("mcp.json"),
            json!({"mcpServers": {
                "snapshot": {"command": "snapshot", "codex": true},
                "remove": {"command": "remove", "codex": true},
                "sushiai-messages": {"command": "fake", "codex": true}
            }})
            .to_string(),
        )
        .unwrap();
        let mut settings = Settings::default();
        let tools = task_tools(&app, &settings, &task, &worktree);
        assert!(tools.project.contains_key("fallback"));
        settings.mcp_disable.insert(
            task.repo.clone(),
            vec!["remove".into(), messages::SERVER.into(), "missing".into()],
        );
        settings
            .mcp_enable
            .insert(task.repo.clone(), vec!["not-a-tool".into()]);
        let tools = task_tools(&app, &settings, &task, &worktree);
        let config = tools.mcp_config(&json!({"command": "messages"}));
        assert_eq!(
            config["mcpServers"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["fallback", "snapshot", messages::SERVER]
        );
        let codex = tools.codex_servers(Some(&json!({"command": "messages"})));
        assert_eq!(
            codex
                .iter()
                .map(|(key, _)| key.as_str())
                .collect::<Vec<_>>(),
            vec!["snapshot", messages::SERVER]
        );
        assert_eq!(codex.last().unwrap().1["command"], "messages");
        assert_eq!(
            config["mcpServers"][messages::SERVER]["command"],
            "messages"
        );
        assert_eq!(
            tools.notes,
            vec![
                format!(
                    "MCP: ignored unknown name \"not-a-tool\" in mcpEnable for {}",
                    task.repo
                ),
                format!(
                    "MCP: ignored unknown name \"missing\" in mcpDisable for {}",
                    task.repo
                ),
            ]
        );
        app.secrets
            .write()
            .unwrap()
            .repo_projects
            .insert(task.repo.clone(), "project".into());
        assert!(task_tools(&app, &settings, &task, &worktree)
            .project
            .is_empty());
        task.project_id = Some("project".into());
        assert!(task_tools(&app, &settings, &task, &worktree)
            .project
            .is_empty());
        // A stored project id that does not match the repo is no project.
        app.secrets.write().unwrap().repo_projects.clear();
        assert!(task_tools(&app, &settings, &task, &worktree)
            .project
            .contains_key("fallback"));
        settings
            .mcp_disable
            .insert(task.repo.clone(), vec!["fallback".into()]);
        task.project_id = None;
        assert!(task_tools(&app, &settings, &task, &worktree)
            .project
            .is_empty());
    }

    #[tokio::test]
    async fn task_tools_enable_and_grants_are_repo_scoped_and_disable_matches_id_or_server_key() {
        let (app, dir) = test_app();
        let task = task_with_status(TaskStatus::Queued);
        app.store.save_task(&task).unwrap();
        let mut settings = Settings {
            chat_tools: serde_json::from_value(json!([
                {"id": "global", "label": "Global tool", "enabled": true, "server": {"command": "true"}},
                {"id": "local", "label": "Local tool", "enabled": false, "server": {"command": "true"}},
                {"id": "plug", "label": "Plugin tool", "enabled": true, "server": {"ref": "plugin:demo/server"}}
            ])).unwrap(),
            ..Settings::default()
        };
        std::fs::write(app.data_dir.join("chat-tools.json"), json!({
            "global": {"status": "ok", "tools": [{"name": "get_item", "readOnly": true}], "checkedAt": 0},
            "local": {"status": "ok", "tools": [{"name": "get_item", "readOnly": true}], "checkedAt": 0}
        }).to_string()).unwrap();
        settings
            .mcp_enable
            .insert(format!("{}/", task.repo), vec!["global".into()]);
        let tools = task_tools(&app, &settings, &task, dir.path());
        assert!(tools.connected.servers.is_empty());
        assert!(tools_block(&tools, false)
            .contains("global (Global tool): connected but off for this repo"));
        assert_eq!(
            tools.capabilities()["connectedButOffForThisTask"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        settings
            .mcp_enable
            .insert(task.repo.clone(), vec!["local".into()]);
        add_rule(&app, Scope::Repo(&task.repo), "enable:global");
        let tools = task_tools(&app, &settings, &task, dir.path());
        assert_eq!(
            tools
                .connected
                .servers
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["global", "local"]
        );
        assert_eq!(tools.off, vec![("plug".into(), "Plugin tool".into())]);
        add_rule(&app, Scope::Task(&task.id), "enable:local");
        settings.mcp_enable.clear();
        assert!(task_tools(&app, &settings, &task, dir.path())
            .connected
            .servers
            .contains_key("local"));

        std::fs::write(
            app.store.task_dir(&task.id).join("mcp.json"),
            json!({"mcpServers": {
                "global": {"command": "task-copy", "codex": true},
                "plugin:demo:server": {"command": "plugin-copy", "codex": true}
            }})
            .to_string(),
        )
        .unwrap();
        for name in ["plug", "plugin:demo:server"] {
            settings
                .mcp_disable
                .insert(task.repo.clone(), vec!["global".into(), name.into()]);
            let tools = task_tools(&app, &settings, &task, dir.path());
            assert_eq!(
                tools
                    .connected
                    .servers
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
                vec!["local"]
            );
            assert!(tools.task.is_empty());
            assert!(tools.notes.is_empty());
            assert!(tools.allowed().iter().all(|tool| !tool.contains("global")));
            assert_eq!(tools.codex_servers(Some(&json!({}))).len(), 1);
        }
    }
}

#[cfg(test)]
mod answer_caller_tests {
    use super::*;

    #[tokio::test]
    async fn a_held_permission_refusal_keeps_the_caller_and_policy_evidence() {
        for (source, caller) in [
            (None, AnsweredBy::Owner),
            (Some("lead"), AnsweredBy::Lead),
            (Some("overseer"), AnsweredBy::Overseer),
            (Some("chat"), AnsweredBy::Orchestrator),
            (Some("policy"), AnsweredBy::Policy),
        ] {
            let (app, _dir) = test_app();
            let task = task_with_status(TaskStatus::Running);
            app.store.save_task(&task).unwrap();
            let ctx = HookContext::new(&task, 1, Path::new(&task.worktree), &task.base_sha);
            let loop_app = app.clone();
            let mut events = app.subscribe();
            let waiter = tokio::spawn(async move {
                ask(
                    &loop_app,
                    &ctx,
                    &Subject::Tool("mcp__example__write".into()),
                    "",
                )
                .await
            });
            tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .unwrap()
                .unwrap();
            let mut params = json!({"id": task.id, "answer": "Deny"});
            if let Some(source) = source {
                params["source"] = json!(source);
            }
            if caller == AnsweredBy::Policy {
                params["evidence"] = json!("waited");
            }
            app.handle_task_answer(params).await.unwrap();
            assert_eq!(waiter.await.unwrap().as_deref(), Some("Deny"));
            let task = app.store.load_task(&task.id).unwrap().unwrap();
            assert_eq!(task.status, TaskStatus::Running);
            assert!(task.question.is_none());
            assert_eq!(task.question_history[0].answered_by, caller);
            if caller == AnsweredBy::Policy {
                assert_eq!(task.assumptions[0].evidence, "waited");
                assert!(task.decisions[0].starts_with("Policy:"));
            }
            assert!(app.agent_answers.lock().unwrap().is_empty());
            app.shutdown();
        }
    }
}
