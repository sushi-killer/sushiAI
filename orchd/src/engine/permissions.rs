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
    TaskTools {
        connected: chat_tools::for_task(app, &on),
        off,
        project: servers_of(&worktree.join(".mcp.json")),
        task: servers_of(&app.store.task_dir(&task.id).join("mcp.json")),
        granted: rules
            .iter()
            .filter_map(|r| r.strip_prefix("tool:"))
            .map(str::to_string)
            .collect(),
    }
}

impl TaskTools {
    /// The run's `mcp.json` content: the project's servers, the connected
    /// tools, the task's own (which win over both), then `messages_server`.
    pub fn mcp_config(&self, messages_server: &serde_json::Value) -> serde_json::Value {
        let mut servers = serde_json::Map::new();
        for (key, def) in &self.project {
            servers.insert(key.clone(), def.clone());
        }
        for (key, def) in &self.connected.servers {
            servers.entry(key.clone()).or_insert_with(|| def.clone());
        }
        for (key, def) in &self.task {
            servers.insert(key.clone(), def.clone());
        }
        servers.insert(messages::SERVER.to_string(), messages_server.clone());
        json!({"mcpServers": servers})
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
        for key in self.project.keys() {
            lines.push(format!(
                "{key}: the repository's own MCP server (reads run; a write asks the owner)"
            ));
        }
        for key in self.task.keys() {
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
            return Gate::Pass;
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
        {
            return Gate::Allow;
        }
        return Gate::Ask(subject);
    }
    Gate::Pass
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
    ctx.hook_running.store(true, Ordering::SeqCst);
    let _paused = ClearOnDrop(&ctx.hook_running);
    let mut task = app.store.load_task(&ctx.task_id).ok().flatten()?;
    task.question = Some(question.clone());
    task.status = TaskStatus::Waiting;
    task.updated_at = now_ms();
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);
    let answer = tokio::select! {
        _ = ctx.cancel.cancelled() => None,
        a = rx => a.ok(),
    };
    app.permission_waits.lock().unwrap().remove(&ctx.task_id);
    if let Ok(Some(mut task)) = app.store.load_task(&ctx.task_id) {
        if task
            .question
            .as_ref()
            .is_some_and(|q| q.text == question.text)
        {
            task.question = None;
            if let Some(answer) = &answer {
                task.decisions.push(format!("Owner: {answer}"));
                record_answered_question(&mut task, &question, answer, AnsweredBy::Owner);
                task.status = TaskStatus::Running;
            }
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
        }
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
    if let Ok(Some(mut task)) = app.store.load_task(task_id) {
        task.decisions.push(line);
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);
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
pub(super) fn unhandled_denials(
    handled: &BTreeSet<String>,
    worktree: &Path,
    outcome: &harness::RunOutcome,
) -> Vec<Subject> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for denial in &outcome.permission_denials {
        let tool = denial.tool.as_str();
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
        if handled.contains(subject.name()) || !seen.insert(subject.rule()) {
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
        drop_criterion(task, missing.criterion);
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
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
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
        task.status = TaskStatus::Failed;
        task.updated_at = now_ms();
        let _ = app.store.save_task(task);
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
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    LoopSignal::Continue
}

/// The question for an agent's `No tool: <capability>` report.
pub(super) fn no_tool_question(app: &App, task: &Task, capability: &str) -> Question {
    let settings = app.settings.read().unwrap();
    let lower = capability.to_lowercase();
    let off = settings.chat_tools.iter().find(|c| {
        !c.enabled
            && !rules_for(app, &task.repo, &task.id).contains(&format!("enable:{}", c.id))
            && (lower.contains(&c.id.to_lowercase()) || lower.contains(&c.label.to_lowercase()))
    });
    let mut options = Vec::new();
    if let Some(cfg) = off {
        options.push(format!("Enable {} for this task", cfg.id));
    }
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
        let subjects = unhandled_denials(&handled, Path::new("/wt"), &outcome);
        assert_eq!(
            subjects,
            vec![
                Subject::Tool("mcp__slack__post".into()),
                Subject::Path(".claude/a.md".into())
            ]
        );
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
            task: [("t".to_string(), json!({"command": "t"}))].into(),
            granted: vec!["mcp__a__send".into(), "mcp__a__get".into()],
        };
        assert_eq!(tools.allowed(), vec!["mcp__a__get", "mcp__a__send"]);
        let config = tools.mcp_config(&json!({"command": "messages"}));
        let servers = config["mcpServers"].as_object().unwrap();
        // The repo's own definition of a server wins over the connected one.
        assert_eq!(servers["a"]["command"], "project");
        assert!(servers.contains_key("p") && servers.contains_key("t"));
        assert_eq!(servers[messages::SERVER]["command"], "messages");
        let block = tools_block(&tools, false);
        assert!(
            block.contains("a: read tools get; write tools send"),
            "{block}"
        );
        assert!(block.contains("No tool: "), "{block}");
        assert!(!tools_block(&tools, true).contains("No tool: "));
    }
}
