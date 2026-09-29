//! The orchestrator agent's conversations: several named chat sessions per
//! repository, run by the daemon so a reply keeps coming (and stays readable)
//! when the app window closes or restarts. Each turn is one CLI run that only
//! reads the repository and manages tasks through `orchd mcp`; the harness
//! session id carries a session's conversation from turn to turn. A repo has
//! one live turn at most, and it always belongs to the `current` session.

use super::*;
use serde::Serialize;

/// The orchestrator agent's role, handed to Claude as an appended system
/// prompt and to Codex as developer instructions. The tools it gets (read
/// only, plus the orchd MCP bridge) are what actually keep it from doing the
/// work itself; this text explains why.
const ROLE: &str = "You are the owner's task orchestrator in sushiAI. You never change files or run commands yourself: every piece of work becomes an orchd task through the sushiai-orchestrator tools, which run it in an isolated worktree, verify it and report back. Follow that server's instructions. You are not woken up between turns, so never promise to watch or report later: say the task is in the Orchestrator panel. Reply in the owner's language, briefly.";

const SERVER: &str = "sushiai-orchestrator";
const MAX_MESSAGES: usize = 200;
const MAX_TEXT: usize = 20_000;
/// Same length a task title gets when the planner is off (`createTask` in
/// OrchestratorPanel.tsx).
const MAX_TITLE: usize = 60;
const BUSY: &str = "the orchestrator is still answering";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub role: String,
    pub text: String,
    pub ts: i64,
}

/// One conversation with the orchestrator agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub id: String,
    /// Set once from the session's first owner message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    /// True only for the current session while a turn is live in this
    /// daemon; a session saved as busy by a daemon that died reads back idle.
    #[serde(default)]
    pub busy: bool,
    /// The last progress line of a live turn (a tool call, "session started").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_id: Option<String>,
    /// The project's MCP launch config the app resolved; tasks this agent
    /// creates for the same repo get it (`ORCHD_TASK_MCP`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_mcp: Option<serde_json::Value>,
}

impl ChatSession {
    fn new() -> Self {
        ChatSession {
            id: uuid::Uuid::new_v4().to_string(),
            ..Default::default()
        }
    }
}

/// A repo's chat sessions, kept in `chats/<hash>.json`. `chat.get` and
/// `chat.send` act on `current`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStore {
    pub repo: String,
    pub current: String,
    pub sessions: Vec<ChatSession>,
}

/// The one thread per repo daemons kept before sessions, at the same path.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyThread {
    #[allow(dead_code)]
    repo: String,
    #[serde(default)]
    messages: Vec<ChatMessage>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    route_id: Option<String>,
    #[serde(default)]
    task_mcp: Option<serde_json::Value>,
}

/// A session as the sidebar lists it: no message bodies.
#[derive(Debug, Serialize, PartialEq)]
struct SessionSummary {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    busy: bool,
}

impl ChatStore {
    fn fresh(repo: &str) -> Self {
        let session = ChatSession::new();
        ChatStore {
            repo: repo.to_string(),
            current: session.id.clone(),
            sessions: vec![session],
        }
    }

    fn current(&self) -> &ChatSession {
        self.sessions
            .iter()
            .find(|s| s.id == self.current)
            .expect("parse_store keeps current pointing at a session")
    }

    fn current_mut(&mut self) -> &mut ChatSession {
        let current = self.current.clone();
        self.sessions
            .iter_mut()
            .find(|s| s.id == current)
            .expect("parse_store keeps current pointing at a session")
    }

    /// Marks the current session busy while a turn is live; a note only
    /// means something on a live turn.
    fn mark_busy(&mut self, live: bool) {
        for session in &mut self.sessions {
            session.busy = live && session.id == self.current;
            if !session.busy {
                session.note = None;
            }
        }
    }

    fn summaries(&self) -> Vec<SessionSummary> {
        self.sessions
            .iter()
            .map(|s| SessionSummary {
                id: s.id.clone(),
                title: s.title.clone(),
                busy: s.busy,
            })
            .collect()
    }

    /// The current session as `chat.get` returns it: the session plus its repo.
    fn thread_json(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self.current()).unwrap_or_default();
        value["repo"] = json!(self.repo);
        value
    }

    fn list_json(&self) -> serde_json::Value {
        json!({"current": self.current, "sessions": self.summaries()})
    }
}

fn title_for(text: &str) -> String {
    if text.chars().count() > MAX_TITLE {
        format!(
            "{}\u{2026}",
            text.chars().take(MAX_TITLE - 1).collect::<String>()
        )
    } else {
        text.to_string()
    }
}

/// The repo's store from its file's text: the current shape, else a legacy
/// thread wrapped into one session, else one empty session. The flag says
/// the result differs from what's on disk and should be written back.
fn parse_store(repo: &str, text: Option<&str>) -> (ChatStore, bool) {
    let parsed = text.and_then(|t| serde_json::from_str::<ChatStore>(t).ok());
    let mut changed = parsed.is_none();
    let mut chats = parsed
        .or_else(|| {
            let legacy = serde_json::from_str::<LegacyThread>(text?).ok()?;
            let title = legacy
                .messages
                .iter()
                .find(|m| m.role == "user")
                .map(|m| title_for(&m.text));
            let session = ChatSession {
                title,
                messages: legacy.messages,
                error: legacy.error,
                session_id: legacy.session_id,
                route_id: legacy.route_id,
                task_mcp: legacy.task_mcp,
                ..ChatSession::new()
            };
            Some(ChatStore {
                repo: repo.to_string(),
                current: session.id.clone(),
                sessions: vec![session],
            })
        })
        .unwrap_or_else(|| ChatStore::fresh(repo));
    if chats.sessions.is_empty() {
        chats = ChatStore::fresh(repo);
        changed = true;
    }
    if !chats.sessions.iter().any(|s| s.id == chats.current) {
        chats.current = chats.sessions.last().unwrap().id.clone();
        changed = true;
    }
    chats.repo = repo.to_string();
    (chats, changed)
}

/// Where a repo's turns keep their MCP config, settings and event log.
fn turn_dir(app: &App, repo: &str) -> PathBuf {
    app.data_dir.join("chats").join(simple_hash(repo))
}

fn store_path(app: &App, repo: &str) -> PathBuf {
    app.data_dir
        .join("chats")
        .join(format!("{}.json", simple_hash(repo)))
}

/// Reads the repo's store, writing it straight back when it had to be built
/// or migrated so every caller sees the same session ids. Callers hold
/// `chat_turns`, which serialises all store reads and writes.
fn read_store(app: &App, repo: &str) -> ChatStore {
    let path = store_path(app, repo);
    let text = std::fs::read_to_string(&path).ok();
    let (chats, changed) = parse_store(repo, text.as_deref());
    if changed {
        let _ = store::write_json_atomic(&path, &chats);
    }
    chats
}

fn load(app: &App, repo: &str) -> ChatStore {
    let turns = app.chat_turns.lock().unwrap();
    let mut chats = read_store(app, repo);
    chats.mark_busy(turns.contains_key(repo));
    chats
}

fn broadcast(app: &App, chats: &ChatStore) {
    let _ = app.events_tx.send(Event::Chat {
        thread: Box::new(chats.thread_json()),
        current: chats.current.clone(),
        sessions: Box::new(serde_json::to_value(chats.summaries()).unwrap_or_default()),
    });
}

/// Writes a turn's session back into its repo's store and broadcasts it.
fn save(app: &App, repo: &str, session: &ChatSession) {
    write_session(app, repo, session, false);
}

/// Saves a turn's final session and frees the repo's turn slot under one
/// lock, so no new/switch/clear can slip in between and be overwritten by
/// the reply (or have cleared messages restored by it).
fn finish_turn(app: &App, repo: &str, session: &ChatSession) {
    write_session(app, repo, session, true);
}

fn write_session(app: &App, repo: &str, session: &ChatSession, end_turn: bool) {
    let chats = {
        let mut turns = app.chat_turns.lock().unwrap();
        if end_turn {
            turns.remove(repo);
        }
        let mut chats = read_store(app, repo);
        match chats.sessions.iter_mut().find(|s| s.id == session.id) {
            Some(slot) => *slot = session.clone(),
            None => chats.sessions.push(session.clone()),
        }
        chats.mark_busy(turns.contains_key(repo));
        let _ = store::write_json_atomic(&store_path(app, repo), &chats);
        chats
    };
    broadcast(app, &chats);
}

/// Applies `change` to the repo's store, saves and broadcasts it - refused
/// while a turn is live, since that turn belongs to the current session and
/// moving `current` under it would orphan the reply. Holding `chat_turns`
/// throughout keeps a turn from starting halfway through.
fn change_idle(
    app: &App,
    repo: &str,
    change: impl FnOnce(&mut ChatStore) -> Result<(), String>,
) -> Result<ChatStore, String> {
    let chats = {
        let turns = app.chat_turns.lock().unwrap();
        if turns.contains_key(repo) {
            return Err(BUSY.to_string());
        }
        let mut chats = read_store(app, repo);
        change(&mut chats)?;
        chats.mark_busy(false);
        store::write_json_atomic(&store_path(app, repo), &chats).map_err(|e| e.to_string())?;
        chats
    };
    broadcast(app, &chats);
    Ok(chats)
}

fn push(session: &mut ChatSession, role: &str, text: &str) {
    if role == "user" && session.title.is_none() {
        session.title = Some(title_for(text));
    }
    session.messages.push(ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: role.to_string(),
        text: truncate_chars(text, MAX_TEXT),
        ts: now_ms(),
    });
    let excess = session.messages.len().saturating_sub(MAX_MESSAGES);
    session.messages.drain(..excess);
}

pub(super) fn repo_param(params: &serde_json::Value) -> Result<String, String> {
    let repo = params
        .get("repo")
        .and_then(|v| v.as_str())
        .ok_or("repo is required")?;
    if !Path::new(repo).is_absolute() || !Path::new(repo).is_dir() {
        return Err("repo must be an existing absolute directory".to_string());
    }
    Ok(repo.to_string())
}

pub async fn handle_get(app: &App, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    Ok(load(app, &repo).thread_json())
}

pub async fn handle_list(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    Ok(load(app, &repo).list_json())
}

pub async fn handle_new(app: &App, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    let chats = change_idle(app, &repo, |chats| {
        let session = ChatSession::new();
        chats.current = session.id.clone();
        chats.sessions.push(session);
        Ok(())
    })?;
    Ok(chats.thread_json())
}

pub async fn handle_switch(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("id is required")?
        .to_string();
    let chats = change_idle(app, &repo, |chats| {
        if !chats.sessions.iter().any(|s| s.id == id) {
            return Err("no such chat session".to_string());
        }
        chats.current = id;
        Ok(())
    })?;
    Ok(chats.thread_json())
}

/// Empties the current session and forgets its harness session, so the next
/// reply carries no prior context. The title goes too: it named a message
/// that is gone.
pub async fn handle_clear(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    let chats = change_idle(app, &repo, |chats| {
        let session = chats.current_mut();
        session.messages.clear();
        session.title = None;
        session.session_id = None;
        session.error = None;
        Ok(())
    })?;
    Ok(chats.thread_json())
}

pub async fn handle_cancel(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    if let Some(cancel) = app.chat_turns.lock().unwrap().get(&repo) {
        cancel.cancel();
    }
    Ok(json!({}))
}

/// Who a turn answers: the owner's chat message, or the questions tasks sent
/// meanwhile (their message ids). A question turn leaves the chat's messages
/// alone; its reply goes back to the tasks instead.
enum Turn {
    Owner,
    Questions(Vec<String>),
}

const QUESTIONS_INTRO: &str = "Tasks asked you the questions below while they keep working. Answer each with orchestrator_reply {question, text}; a task reads the answer on its next attempt.";
const QUESTION_TURN_REPLY: &str =
    "Your final message is sent back to each task you did not answer that way.";

/// Claims the repo's one turn slot and loads its current session, starting a
/// fresh harness session when the orchestrator's route changed since the
/// session's last turn.
fn begin_turn(app: &App, repo: &str) -> Result<(ChatSession, Route, CancelToken), String> {
    let settings = app.settings.read().unwrap().clone();
    let route =
        orchestrator_route(&settings).ok_or("no route is configured for the orchestrator")?;
    let cancel = CancelToken::new();
    {
        let mut turns = app.chat_turns.lock().unwrap();
        if turns.contains_key(repo) {
            return Err(BUSY.to_string());
        }
        turns.insert(repo.to_string(), cancel.clone());
    }
    let mut session = load(app, repo).current().clone();
    // A harness session belongs to one harness; switching routes starts fresh.
    if session.route_id.as_deref() != Some(route.id.as_str()) {
        session.session_id = None;
        session.route_id = Some(route.id.clone());
    }
    Ok((session, route, cancel))
}

pub async fn handle_send(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or("text is required")?
        .to_string();
    let (mut session, route, cancel) = begin_turn(app, &repo)?;
    if let Some(mcp) = params.get("mcp") {
        session.task_mcp = Some(mcp.clone());
    }
    push(&mut session, "user", &text);
    session.error = None;
    save(app, &repo, &session);
    // Questions waiting for the orchestrator ride along with the owner's
    // message; the chat shows only what the owner wrote.
    let prompt = match messages::take_for_orchestrator(app, &repo) {
        Some((_, questions)) => format!("{text}\n\n---\n\n{QUESTIONS_INTRO}\n\n{questions}"),
        None => text,
    };
    let app = app.arc();
    tokio::spawn(async move {
        run_turn(&app, repo, session, route, prompt, cancel, Turn::Owner).await;
    });
    Ok(json!({}))
}

/// Starts a turn for the questions tasks sent the orchestrator in `repo`,
/// in the owner's current chat session. A turn already running picks
/// them up when it ends; with no route configured they wait for one.
pub(super) fn wake(app: &App, repo: &str) {
    if !Path::new(repo).is_dir() || !messages::has_pending_for_orchestrator(app, repo) {
        return;
    }
    let Ok((session, route, cancel)) = begin_turn(app, repo) else {
        return;
    };
    let Some((ids, questions)) = messages::take_for_orchestrator(app, repo) else {
        app.chat_turns.lock().unwrap().remove(repo);
        return;
    };
    save(app, repo, &session);
    let prompt = format!("{QUESTIONS_INTRO} {QUESTION_TURN_REPLY}\n\n{questions}");
    let app = app.arc();
    let repo = repo.to_string();
    tokio::spawn(async move {
        run_turn(
            &app,
            repo,
            session,
            route,
            prompt,
            cancel,
            Turn::Questions(ids),
        )
        .await;
    });
}

/// The orchestrator agent's route: the one the owner picked, else the route
/// that handles standard tasks.
pub(super) fn orchestrator_route(settings: &Settings) -> Option<Route> {
    let find = |id: &str| settings.routes.iter().find(|r| r.id == id).cloned();
    find(&settings.orchestrator).or_else(|| find(settings.tiers.get(&Tier::Standard)?))
}

fn mcp_server(app: &App, task_mcp_file: Option<&Path>) -> serde_json::Value {
    let mut server = json!({
        "command": app.orchd_path,
        "args": [
            "mcp",
            "--data",
            app.data_dir.to_string_lossy(),
            "--socket",
            app.socket_path.to_string_lossy(),
        ],
    });
    if let Some(file) = task_mcp_file {
        server["env"] = json!({"ORCHD_TASK_MCP": file.to_string_lossy()});
    }
    server
}

fn claude_argv(
    route: &Route,
    mcp_config: &Path,
    settings_path: &Path,
    session: Option<&str>,
) -> Vec<String> {
    let tools = crate::mcp::ORCHESTRATOR_TOOLS
        .iter()
        .map(|t| format!("mcp__{SERVER}__{t}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut argv: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--setting-sources",
        "project,local",
        "--disable-slash-commands",
        "--tools",
        "Read,Grep,Glob",
        "--append-system-prompt",
        ROLE,
        "--strict-mcp-config",
        "--mcp-config",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    argv.push(mcp_config.to_string_lossy().to_string());
    argv.push("--settings".to_string());
    argv.push(settings_path.to_string_lossy().to_string());
    argv.push("--allowedTools".to_string());
    argv.push(tools);
    argv.push("--permission-prompts".to_string());
    argv.push("none".to_string());
    if let Some(model) = &route.model {
        argv.push("--model".to_string());
        argv.push(model.clone());
    }
    if let Some(effort) = &route.effort {
        argv.push("--effort".to_string());
        argv.push(effort.clone());
    }
    if let Some(session) = session {
        argv.push("--resume".to_string());
        argv.push(session.to_string());
    }
    argv
}

fn codex_argv(
    route: &Route,
    repo: &str,
    server: &serde_json::Value,
    session: Option<&str>,
) -> Vec<String> {
    let mut argv: Vec<String> = match session {
        // `resume` takes neither `-C` nor `--sandbox`; the cwd is set on the
        // process and the sandbox through config.
        Some(session) => vec![
            "exec".into(),
            "resume".into(),
            session.into(),
            "--json".into(),
            "--skip-git-repo-check".into(),
            "-c".into(),
            "sandbox_mode=\"read-only\"".into(),
        ],
        None => vec![
            "exec".into(),
            "--json".into(),
            "--skip-git-repo-check".into(),
            "-C".into(),
            repo.into(),
            "--sandbox".into(),
            "read-only".into(),
        ],
    };
    let toml = |v: &serde_json::Value| v.to_string();
    argv.push("-c".into());
    argv.push(format!("developer_instructions={}", toml(&json!(ROLE))));
    argv.extend(harness::codex_mcp_flags(SERVER, server));
    if let Some(model) = &route.model {
        argv.push("-m".into());
        argv.push(model.clone());
    }
    if let Some(effort) = &route.effort {
        argv.push("-c".into());
        argv.push(format!("model_reasoning_effort=\"{effort}\""));
    }
    argv.push("-".into());
    argv
}

async fn run_turn(
    app: &Arc<App>,
    repo: String,
    mut thread: ChatSession,
    route: Route,
    text: String,
    cancel: CancelToken,
    turn: Turn,
) {
    let dir = turn_dir(app, &repo);
    let _ = std::fs::create_dir_all(&dir);
    let task_mcp_file = thread.task_mcp.as_ref().and_then(|mcp| {
        let file = dir.join("task-mcp.json");
        store::write_json_atomic(&file, &json!({"repo": repo, "mcp": mcp}))
            .ok()
            .map(|_| file)
    });
    let server = mcp_server(app, task_mcp_file.as_deref());
    let session = thread.session_id.clone();
    let key_path = dir.join("key");
    let argv = match route.harness {
        Harness::Claude => {
            let mcp_config = dir.join("mcp.json");
            let _ = store::write_json_atomic(&mcp_config, &json!({"mcpServers": {SERVER: server}}));
            let settings_path = dir.join("settings.json");
            let settings = app.settings.read().unwrap().clone();
            write_readonly_claude_settings_with_profile(
                &route,
                app,
                &key_path,
                &settings,
                &[app.data_dir.to_string_lossy().to_string()],
                &settings_path,
            );
            claude_argv(&route, &mcp_config, &settings_path, session.as_deref())
        }
        Harness::Codex => codex_argv(&route, &repo, &server, session.as_deref()),
    };

    let result = run(app, &repo, &mut thread, &route, &argv, &text, &cancel).await;
    let _ = std::fs::remove_file(&key_path);

    // Messages sent meanwhile are impossible (the turn slot stays held until
    // `finish_turn`, so the session can't be switched or cleared under it),
    // but the owner may have been shown a fresher note: keep this turn's
    // session.
    thread.note = None;
    match (result, &turn) {
        (Ok(outcome), _) => {
            if outcome.session_id.is_some() {
                thread.session_id = outcome.session_id.clone();
            }
            match (
                outcome.final_text.filter(|t| !t.trim().is_empty()),
                outcome.error,
                &turn,
            ) {
                (Some(reply), _, Turn::Owner) => push(&mut thread, "assistant", &reply),
                (Some(reply), _, Turn::Questions(ids)) => {
                    messages::reply_where_unanswered(app, ids, &reply)
                }
                (None, Some(error), _) => {
                    // A session the CLI no longer knows fails every resume.
                    thread.session_id = None;
                    if matches!(turn, Turn::Owner) {
                        thread.error = Some(truncate_chars(&error, 2000));
                    }
                }
                (None, None, Turn::Owner) => {
                    thread.error = Some("The orchestrator gave no reply.".into())
                }
                (None, None, Turn::Questions(_)) => {}
            }
        }
        (Err(RunError::Cancelled), Turn::Owner) => thread.error = Some("Stopped.".into()),
        (Err(RunError::Io(error)), Turn::Owner) => thread.error = Some(error),
        (Err(_), Turn::Questions(_)) => {}
    }
    finish_turn(app, &repo, &thread);
    // Questions that arrived during this turn get the next one.
    wake(app, &repo);
}

async fn run(
    app: &Arc<App>,
    repo: &str,
    thread: &mut ChatSession,
    route: &Route,
    argv: &[String],
    text: &str,
    cancel: &CancelToken,
) -> Result<harness::RunOutcome, RunError> {
    let harness_kind = route.harness;
    let started_at = now_ms();
    let thread_resumed = thread.session_id.is_some();
    let record = |outcome: &mut harness::RunOutcome| {
        finalize_cost(app, harness_kind, route.model.as_deref(), outcome);
        outcome.fingerprint = Some(outcome.build_fingerprint(harness_kind, None, String::new()));
        record_run(
            app,
            "",
            0,
            &CostTag::repo("chat", &route.id, repo),
            (harness_kind, route.model.as_deref(), thread_resumed),
            outcome,
            started_at,
        );
    };
    let bin = resolve_binary(harness_kind);
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(argv)
        .current_dir(repo)
        .env("PATH", augmented_path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| RunError::Io(format!("{bin}: {e}")))?;
    let pgid = child.id().map(|p| p as i32);
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(text.as_bytes()).await;
    }
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let mut out = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stdout));
    let mut err = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stderr));
    let events = turn_dir(app, repo).join("events.jsonl");
    let mut outcome = harness::RunOutcome::default();
    let mut stderr_tail = String::new();
    let (mut out_done, mut err_done) = (false, false);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                kill_group(pgid, &mut child).await;
                record(&mut outcome.clone());
                return Err(RunError::Cancelled);
            }
            line = out.next_line(), if !out_done => match line {
                Ok(Some(l)) => {
                    append_line(&events, &l);
                    if let Some(note) = harness::feed_stream_line(harness_kind, &l, &mut outcome) {
                        thread.note = Some(note);
                        save(app, repo, thread);
                    }
                }
                _ => out_done = true,
            },
            line = err.next_line(), if !err_done => match line {
                Ok(Some(l)) => {
                    stderr_tail = tail_chars(&format!("{stderr_tail}{l}\n"), 2000);
                }
                _ => err_done = true,
            },
            status = child.wait(), if out_done && err_done => {
                if let Ok(status) = status {
                    if !status.success() && outcome.error.is_none() {
                        let tail = stderr_tail.trim();
                        outcome.error = Some(if tail.is_empty() {
                            format!("{harness_kind:?} exited with {status}.")
                        } else {
                            tail.to_string()
                        });
                    }
                }
                break;
            }
        }
    }
    record(&mut outcome);
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(harness: Harness) -> Route {
        Route {
            id: "r".into(),
            label: "R".into(),
            harness,
            model: Some("m".into()),
            effort: None,
            profile_id: None,
        }
    }

    #[test]
    fn claude_chat_reads_only_and_reaches_orchd_through_one_allowed_tools_value() {
        let argv = claude_argv(
            &route(Harness::Claude),
            Path::new("/d/mcp.json"),
            Path::new("/d/settings.json"),
            Some("sess"),
        );
        let at = |flag: &str| argv[argv.iter().position(|a| a == flag).unwrap() + 1].clone();
        assert_eq!(at("--tools"), "Read,Grep,Glob");
        assert_eq!(at("--mcp-config"), "/d/mcp.json");
        assert_eq!(at("--resume"), "sess");
        assert_eq!(at("--allowedTools").split(',').count(), 19);
        assert!(at("--allowedTools").contains("mcp__sushiai-orchestrator__orchestrator_reply"));
        assert!(!at("--allowedTools").contains("task_delete"));
        assert!(!argv.iter().any(|a| a == "acceptEdits"));
    }

    #[test]
    fn codex_chat_is_read_only_fresh_or_resumed() {
        let server = json!({"command": "/o", "args": ["mcp"], "env": {"ORCHD_TASK_MCP": "/f"}});
        let fresh = codex_argv(&route(Harness::Codex), "/repo", &server, None);
        assert_eq!(&fresh[..2], &["exec".to_string(), "--json".to_string()]);
        assert!(fresh
            .windows(2)
            .any(|w| w[0] == "--sandbox" && w[1] == "read-only"));
        assert!(fresh
            .contains(&"mcp_servers.sushiai-orchestrator.env.ORCHD_TASK_MCP=\"/f\"".to_string()));
        let resumed = codex_argv(&route(Harness::Codex), "/repo", &server, Some("t1"));
        assert_eq!(
            &resumed[..3],
            &["exec".to_string(), "resume".to_string(), "t1".to_string()]
        );
        assert!(resumed.contains(&"sandbox_mode=\"read-only\"".to_string()));
        assert!(!resumed.contains(&"-C".to_string()));
        assert_eq!(resumed.last().unwrap(), "-");
    }

    #[test]
    fn the_orchestrator_uses_its_own_route_else_the_standard_one() {
        let mut settings = Settings::default();
        let standard = settings.tiers.get(&Tier::Standard).unwrap().clone();
        assert_eq!(orchestrator_route(&settings).unwrap().id, standard);
        settings.orchestrator = "claude-opus".into();
        assert_eq!(orchestrator_route(&settings).unwrap().id, "claude-opus");
        settings.orchestrator = "gone".into();
        assert_eq!(orchestrator_route(&settings).unwrap().id, standard);
    }

    fn test_app() -> (Arc<App>, tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            dir.path().join("data"),
            dir.path().join("orchd.sock"),
            "orchd".to_string(),
        )
        .unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        (app, dir, repo.to_string_lossy().to_string())
    }

    /// Writes `session` into the repo's store as a finished turn would.
    fn converse(app: &App, repo: &str, text: &str, harness_session: &str) {
        let mut session = load(app, repo).current().clone();
        push(&mut session, "user", text);
        push(&mut session, "assistant", "ok");
        session.session_id = Some(harness_session.to_string());
        save(app, repo, &session);
    }

    #[tokio::test]
    async fn a_new_repo_starts_with_one_empty_current_session() {
        let (app, _dir, repo) = test_app();
        let list = handle_list(&app, json!({"repo": repo})).await.unwrap();
        let thread = handle_get(&app, json!({"repo": repo})).await.unwrap();
        assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(list["current"], thread["id"]);
        assert_eq!(thread["messages"], json!([]));
        assert!(thread.get("sessionId").is_none());
        assert_eq!(thread["repo"], json!(repo));
    }

    #[tokio::test]
    async fn a_new_session_is_empty_current_and_titled_by_its_first_owner_message() {
        let (app, _dir, repo) = test_app();
        converse(&app, &repo, "Fix the login page", "harness-1");
        let first = load(&app, &repo).current.clone();

        let created = handle_new(&app, json!({"repo": repo})).await.unwrap();
        assert_ne!(created["id"], json!(first));
        assert_eq!(created["messages"], json!([]));
        assert!(created.get("title").is_none());
        assert!(created.get("sessionId").is_none());

        let long = "x".repeat(80);
        converse(&app, &repo, &long, "harness-2");
        let list = handle_list(&app, json!({"repo": repo})).await.unwrap();
        assert_eq!(list["current"], created["id"]);
        let titles: Vec<_> = list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["title"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(titles[0], "Fix the login page");
        assert_eq!(titles[1], format!("{}\u{2026}", "x".repeat(59)));
        assert!(list["sessions"][0].get("messages").is_none());
    }

    #[tokio::test]
    async fn switching_brings_back_that_sessions_own_messages_and_harness_session() {
        let (app, _dir, repo) = test_app();
        converse(&app, &repo, "first chat", "harness-1");
        let first = load(&app, &repo).current.clone();
        handle_new(&app, json!({"repo": repo})).await.unwrap();
        converse(&app, &repo, "second chat", "harness-2");

        let back = handle_switch(&app, json!({"repo": repo, "id": first}))
            .await
            .unwrap();
        assert_eq!(back["id"], json!(first));
        assert_eq!(back["messages"][0]["text"], "first chat");
        assert_eq!(back["sessionId"], "harness-1");
        assert_eq!(load(&app, &repo).current, first);

        let unknown = handle_switch(&app, json!({"repo": repo, "id": "nope"})).await;
        assert!(unknown.is_err());
        assert_eq!(load(&app, &repo).current, first);
    }

    #[tokio::test]
    async fn clearing_empties_the_current_session_and_forgets_its_harness_session() {
        let (app, _dir, repo) = test_app();
        converse(&app, &repo, "keep me", "harness-1");
        let kept = load(&app, &repo).current.clone();
        handle_new(&app, json!({"repo": repo})).await.unwrap();
        converse(&app, &repo, "clear me", "harness-2");

        let cleared = handle_clear(&app, json!({"repo": repo})).await.unwrap();
        assert_eq!(cleared["messages"], json!([]));
        assert!(cleared.get("sessionId").is_none());
        assert!(cleared.get("title").is_none());
        let chats = load(&app, &repo);
        assert_eq!(chats.sessions.len(), 2);
        let other = chats.sessions.iter().find(|s| s.id == kept).unwrap();
        assert_eq!(other.messages.len(), 2);
        assert_eq!(other.session_id.as_deref(), Some("harness-1"));
    }

    #[tokio::test]
    async fn new_switch_and_clear_are_refused_while_a_turn_is_running() {
        let (app, _dir, repo) = test_app();
        converse(&app, &repo, "first chat", "harness-1");
        let first = load(&app, &repo).current.clone();
        handle_new(&app, json!({"repo": repo})).await.unwrap();
        converse(&app, &repo, "second chat", "harness-2");
        let second = load(&app, &repo).current.clone();

        app.chat_turns
            .lock()
            .unwrap()
            .insert(repo.clone(), CancelToken::new());
        let list = handle_list(&app, json!({"repo": repo})).await.unwrap();
        let busy: Vec<_> = list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["busy"].as_bool().unwrap())
            .collect();
        assert_eq!(busy, vec![false, true]);
        for result in [
            handle_new(&app, json!({"repo": repo})).await,
            handle_switch(&app, json!({"repo": repo, "id": first})).await,
            handle_clear(&app, json!({"repo": repo})).await,
        ] {
            assert_eq!(result.unwrap_err(), BUSY);
        }
        let chats = load(&app, &repo);
        assert_eq!(chats.sessions.len(), 2);
        assert_eq!(chats.current, second);
        assert_eq!(chats.current().messages.len(), 2);

        // The turn's final save frees the slot in the same step, so the reply
        // lands in the session it belongs to and the same requests go through.
        let mut reply = chats.current().clone();
        push(&mut reply, "assistant", "done");
        finish_turn(&app, &repo, &reply);
        assert!(!app.chat_turns.lock().unwrap().contains_key(&repo));
        let chats = load(&app, &repo);
        assert_eq!(chats.current().messages.len(), 3);
        assert!(!chats.current().busy);
        handle_switch(&app, json!({"repo": repo, "id": first}))
            .await
            .unwrap();
        let cleared = handle_clear(&app, json!({"repo": repo})).await.unwrap();
        assert_eq!(cleared["id"], json!(first));
        assert_eq!(cleared["messages"], json!([]));
    }

    #[tokio::test]
    async fn a_legacy_single_thread_file_becomes_a_one_session_store() {
        let (app, _dir, repo) = test_app();
        let legacy = json!({
            "repo": repo,
            "messages": [
                {"id": "a", "role": "user", "text": "Old question", "ts": 1},
                {"id": "b", "role": "assistant", "text": "Old answer", "ts": 2},
                {"id": "c", "role": "user", "text": "Follow-up", "ts": 3},
            ],
            "busy": true,
            "sessionId": "legacy-harness",
            "routeId": "claude-opus",
            "taskMcp": {"servers": {}},
        });
        let path = store_path(&app, &repo);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, legacy.to_string()).unwrap();

        let thread = handle_get(&app, json!({"repo": repo})).await.unwrap();
        let list = handle_list(&app, json!({"repo": repo})).await.unwrap();
        assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(list["current"], thread["id"]);
        assert_eq!(list["sessions"][0]["title"], "Old question");
        let texts: Vec<_> = thread["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["text"].as_str().unwrap())
            .collect();
        assert_eq!(texts, vec!["Old question", "Old answer", "Follow-up"]);
        assert_eq!(thread["sessionId"], "legacy-harness");
        assert_eq!(thread["routeId"], "claude-opus");
        assert_eq!(thread["taskMcp"], json!({"servers": {}}));
        assert_eq!(thread["busy"], false);

        // The migrated shape is on disk, so the ids hold across reads.
        let on_disk: ChatStore =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json!(on_disk.current), thread["id"]);
        assert_eq!(on_disk.sessions[0].messages.len(), 3);
    }

    #[tokio::test]
    async fn a_turn_in_a_new_session_does_not_resume_the_previous_harness_session() {
        let (app, _dir, repo) = test_app();
        converse(&app, &repo, "first chat", "harness-1");
        let created = handle_new(&app, json!({"repo": repo})).await.unwrap();
        let (session, route, _cancel) = begin_turn(&app, &repo).unwrap();
        assert_eq!(json!(session.id), created["id"]);
        assert_eq!(session.session_id, None);
        assert_eq!(session.route_id.as_deref(), Some(route.id.as_str()));
        assert_eq!(begin_turn(&app, &repo).err(), Some(BUSY.to_string()));
    }

    #[test]
    fn a_store_pointing_at_a_missing_session_falls_back_to_the_last_one() {
        let text = json!({"repo": "/r", "current": "gone", "sessions": [{"id": "a"}, {"id": "b"}]})
            .to_string();
        let (chats, changed) = parse_store("/r", Some(&text));
        assert!(changed);
        assert_eq!(chats.current, "b");
        let (fresh, changed) =
            parse_store("/r", Some(r#"{"repo":"/r","current":"x","sessions":[]}"#));
        assert!(changed);
        assert_eq!(fresh.sessions.len(), 1);
        assert_eq!(fresh.current, fresh.sessions[0].id);
    }

    #[test]
    fn a_session_keeps_its_latest_messages_only() {
        let mut thread = ChatSession::default();
        for i in 0..(MAX_MESSAGES + 5) {
            push(&mut thread, "user", &i.to_string());
        }
        assert_eq!(thread.messages.len(), MAX_MESSAGES);
        assert_eq!(thread.messages[0].text, "5");
    }
}
