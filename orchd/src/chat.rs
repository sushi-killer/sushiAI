//! The orchestrator agent's conversations: several named chat sessions per
//! repository, run by the daemon so a reply keeps coming (and stays readable)
//! when the app window closes or restarts. Each turn is one CLI run that only
//! reads the repository and manages tasks through `orchd mcp`; the harness
//! session id carries a session's conversation from turn to turn. A repo has
//! one live turn at most, and it always belongs to the `current` session.

use super::chat_tools;
use super::*;
use serde::Serialize;

const SERVER: &str = "sushiai-orchestrator";
const MAX_MESSAGES: usize = 200;
const MAX_TEXT: usize = 20_000;
/// Same length a task title gets when the planner is off (`createTask` in
/// OrchestratorPanel.tsx).
const MAX_TITLE: usize = 60;
const BUSY: &str = "the orchestrator is still answering";

/// How one turn behaves: which prompt and which tools it gets. The owner picks
/// it per message inside one conversation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatMode {
    #[default]
    Chat,
    Brainstorm,
    Plan,
}

impl ChatMode {
    /// The optional `mode` parameter: absent means the plain chat.
    fn param(params: &serde_json::Value) -> Result<Self, String> {
        match params.get("mode") {
            None | Some(serde_json::Value::Null) => Ok(ChatMode::Chat),
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|_| "mode must be \"chat\", \"brainstorm\" or \"plan\"".to_string()),
        }
    }

    fn is_chat(&self) -> bool {
        *self == ChatMode::Chat
    }

    /// This mode's prompt (`prompts/orchestrator.yaml`, overridable).
    fn prompt(self) -> String {
        crate::prompts::get(match self {
            ChatMode::Chat => "chat",
            ChatMode::Brainstorm => "brainstorm",
            ChatMode::Plan => "plan",
        })
    }

    /// Brainstorm and Plan only look: they hand the owner a proposal instead
    /// of creating tasks.
    fn read_only(self) -> bool {
        !self.is_chat()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub role: String,
    pub text: String,
    pub ts: i64,
    /// The mode the turn ran in (on the owner's message and on the reply to
    /// it); absent means the plain chat.
    #[serde(default, skip_serializing_if = "ChatMode::is_chat")]
    pub mode: ChatMode,
    /// The task the orchestrator proposed in this reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<ChatDraft>,
    /// The list of tasks the orchestrator proposed in this reply, and what the
    /// owner did with each row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<ChatProposal>,
    /// Choices the orchestrator asked the owner to make in this reply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<ChatQuestion>,
    /// The tool calls the orchestrator made while writing this reply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ChatTool>,
    /// The write call the orchestrator proposed in this reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<ChatAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionState {
    Pending,
    Sending,
    Sent,
    Failed,
    Declined,
}

/// A write call on a connected tool that waits for the owner's OK, read from a
/// `sushi-action` block. It runs at most once; `noted` says the orchestrator
/// was told how it ended.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatAction {
    pub id: String,
    /// The connected tool's id.
    pub server: String,
    pub tool: String,
    pub args: serde_json::Value,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub target: String,
    pub state: ActionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub noted: bool,
}

/// One tool call of a reply: the tool's name without the orchd server prefix
/// and a one-line detail (empty when there is none worth showing).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChatTool {
    pub name: String,
    pub summary: String,
}

/// A task the orchestrator proposed for the owner to confirm, read from a
/// `sushi-draft` block.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatDraft {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default, deserialize_with = "crate::brief::lenient_criteria")]
    pub criteria: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default, deserialize_with = "crate::brief::lenient_tier")]
    pub tier: Option<Tier>,
}

/// What a proposed task waits for: the 1-based position of another task in the
/// same list, or the id of an existing task.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ProposalDep {
    Index(usize),
    Id(String),
}

/// One row of a proposal. `task_id` is set once the row became a task,
/// `skipped` once the owner passed on it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProposedTask {
    pub title: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default, deserialize_with = "crate::brief::lenient_criteria")]
    pub criteria: Vec<String>,
    #[serde(default, deserialize_with = "lenient_deps")]
    pub depends_on: Vec<ProposalDep>,
    #[serde(default, deserialize_with = "crate::brief::lenient_tier")]
    pub tier: Option<Tier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skipped: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChatProposal {
    /// Names the parent task of a proposal of two or more rows.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub feature: String,
    /// That parent, once the first rows were created.
    #[serde(
        default,
        rename = "featureTaskId",
        skip_serializing_if = "Option::is_none"
    )]
    pub feature_task_id: Option<String>,
    pub tasks: Vec<ProposedTask>,
}

impl<'de> Deserialize<'de> for ProposalDep {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Number(n) => n
                .as_u64()
                .map(|n| ProposalDep::Index(n as usize))
                .ok_or_else(|| D::Error::custom("bad index")),
            // A model may quote the number; task ids are never all digits.
            serde_json::Value::String(s) => {
                let s = s.trim();
                match s.parse::<usize>() {
                    Ok(n) => Ok(ProposalDep::Index(n)),
                    Err(_) if !s.is_empty() => Ok(ProposalDep::Id(s.to_string())),
                    Err(_) => Err(D::Error::custom("empty dependency")),
                }
            }
            _ => Err(D::Error::custom("bad dependency")),
        }
    }
}

fn lenient_deps<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<ProposalDep>, D::Error> {
    let v: Option<Vec<serde_json::Value>> = Deserialize::deserialize(d).unwrap_or(None);
    let mut deps: Vec<ProposalDep> = Vec::new();
    for dep in v
        .unwrap_or_default()
        .into_iter()
        .filter_map(|d| serde_json::from_value::<ProposalDep>(d).ok())
    {
        if !deps.contains(&dep) {
            deps.push(dep);
        }
    }
    Ok(deps)
}

impl ChatProposal {
    /// The `tasks` list of a `sushi-draft` block. Any row that is not a task
    /// with a title, an index dependency that points outside the list or at
    /// itself, or a dependency cycle drops the whole list.
    fn parse(value: &serde_json::Value, feature: Option<&str>) -> Option<ChatProposal> {
        let rows = value.as_array().filter(|rows| !rows.is_empty())?;
        let mut tasks = Vec::with_capacity(rows.len());
        for row in rows {
            let mut task = serde_json::from_value::<ProposedTask>(row.clone()).ok()?;
            task.title = task.title.trim().to_string();
            if task.title.is_empty() {
                return None;
            }
            task.goal = task.goal.trim().to_string();
            task.task_id = None;
            task.skipped = false;
            tasks.push(task);
        }
        let proposal = ChatProposal {
            feature: feature.unwrap_or_default().trim().to_string(),
            feature_task_id: None,
            tasks,
        };
        proposal.acyclic().then_some(proposal)
    }

    /// The 0-based rows a row depends on, `None` for an index out of range or
    /// pointing at the row itself.
    fn row_deps(&self, row: usize) -> Option<Vec<usize>> {
        let mut deps = Vec::new();
        for dep in &self.tasks[row].depends_on {
            if let ProposalDep::Index(n) = dep {
                if *n == 0 || *n > self.tasks.len() || *n - 1 == row {
                    return None;
                }
                deps.push(*n - 1);
            }
        }
        Some(deps)
    }

    fn acyclic(&self) -> bool {
        let all: Vec<usize> = (0..self.tasks.len()).collect();
        self.order(&all).is_some()
    }

    /// The given rows so that every row follows the rows it depends on (among
    /// the given ones); `None` on a bad index or a cycle.
    fn order(&self, rows: &[usize]) -> Option<Vec<usize>> {
        let mut ordered = Vec::new();
        let mut left = rows.to_vec();
        while !left.is_empty() {
            let ready = left.iter().position(|&row| {
                self.row_deps(row)
                    .is_some_and(|deps| deps.iter().all(|d| !left.contains(d)))
            })?;
            ordered.push(left.remove(ready));
        }
        Some(ordered)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ChatQuestion {
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Deserialize)]
struct DraftBlock {
    #[serde(flatten)]
    draft: ChatDraft,
    #[serde(default, deserialize_with = "lenient_questions")]
    questions: Vec<ChatQuestion>,
    #[serde(default)]
    tasks: Option<serde_json::Value>,
    #[serde(default)]
    feature: Option<serde_json::Value>,
}

fn lenient_questions<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<ChatQuestion>, D::Error> {
    let v: Option<Vec<serde_json::Value>> = Deserialize::deserialize(d).unwrap_or(None);
    Ok(v.unwrap_or_default()
        .into_iter()
        .filter_map(|q| serde_json::from_value::<ChatQuestion>(q).ok())
        .filter(|q| !q.text.trim().is_empty())
        .collect())
}

/// The reply with its `sushi-draft` block removed, plus what the block held.
struct ParsedReply {
    text: String,
    draft: Option<ChatDraft>,
    proposal: Option<ChatProposal>,
    questions: Vec<ChatQuestion>,
}

/// Byte span of the block [`crate::brief::last_fenced_block`] reads: from its
/// opening marker through its closing fence or tag.
fn block_span(text: &str, tag: &str) -> Option<(usize, usize)> {
    let fence = format!("```{tag}");
    let open_tag = format!("<{tag}>");
    let fence_at = text.rfind(&fence);
    let tag_at = text.rfind(&open_tag);
    if tag_at.is_some() && (fence_at.is_none() || tag_at > fence_at) {
        let start = tag_at?;
        let content = start + open_tag.len();
        let close = format!("</{tag}>");
        return Some((start, content + text[content..].find(&close)? + close.len()));
    }
    let start = fence_at?;
    let after_open = start + fence.len();
    let content = after_open + text[after_open..].find('\n')? + 1;
    let mut offset = content;
    for line in text[content..].split_inclusive('\n') {
        offset += line.len();
        if line.trim() == "```" {
            return Some((start, offset));
        }
    }
    let close = text[content..].rfind("```")?;
    Some((start, content + close + 3))
}

/// Reads the reply's last `sushi-draft` block. A block that does not parse,
/// or holds no title, no question and no valid task list, is no block: the
/// text stays as is. A malformed task list alone is dropped, the rest of the
/// block still counts.
fn parse_reply(reply: &str) -> ParsedReply {
    let verbatim = || ParsedReply {
        text: reply.to_string(),
        draft: None,
        proposal: None,
        questions: Vec::new(),
    };
    let Some(body) = crate::brief::last_fenced_block(reply, "sushi-draft") else {
        return verbatim();
    };
    let Ok(block) = serde_json::from_str::<DraftBlock>(&body) else {
        return verbatim();
    };
    let Some((start, end)) = block_span(reply, "sushi-draft") else {
        return verbatim();
    };
    let mut draft = block.draft;
    draft.title = draft.title.trim().to_string();
    let proposal = block.tasks.as_ref().and_then(|tasks| {
        ChatProposal::parse(tasks, block.feature.as_ref().and_then(|f| f.as_str()))
    });
    if draft.title.is_empty() && block.questions.is_empty() && proposal.is_none() {
        return verbatim();
    }
    let before = reply[..start].trim_end();
    let after = reply[end..].trim_start();
    let mut text = match (before.is_empty(), after.is_empty()) {
        (false, false) => format!("{before}\n\n{after}"),
        (false, true) => before.to_string(),
        _ => after.to_string(),
    };
    if text.trim().is_empty() {
        text = if !draft.title.is_empty() {
            draft.title.clone()
        } else if let Some(question) = block.questions.first() {
            question.text.clone()
        } else {
            let count = proposal.as_ref().map_or(0, |p| p.tasks.len());
            format!("{count} proposed tasks")
        };
    }
    ParsedReply {
        text,
        draft: (!draft.title.is_empty()).then_some(draft),
        proposal,
        questions: block.questions,
    }
}

/// One conversation with the orchestrator agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub id: String,
    /// When the session was created, in ms.
    #[serde(default)]
    pub created_at: i64,
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
    /// The mode of the last message the owner sent; the composer restores it.
    #[serde(default, skip_serializing_if = "ChatMode::is_chat")]
    pub mode: ChatMode,
}

impl ChatSession {
    fn new() -> Self {
        ChatSession {
            id: uuid::Uuid::new_v4().to_string(),
            created_at: now_ms(),
            ..Default::default()
        }
    }

    fn updated_at(&self) -> i64 {
        self.messages.last().map_or(self.created_at, |m| m.ts)
    }
}

/// A repo's chat sessions, kept in `chats/<hash>.json`. `chat.get` and
/// `chat.send` act on `current`, which always names a session. A store written
/// before modes may carry a `brainstormCurrent` pointer and `kind` fields; they
/// are ignored, so its brainstorm sessions read as ordinary chat sessions.
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
#[serde(rename_all = "camelCase")]
struct SessionSummary {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    busy: bool,
    updated_at: i64,
    message_count: usize,
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
            .expect("parse_store keeps the pointer at a session")
    }

    fn current_mut(&mut self) -> &mut ChatSession {
        let id = self.current.clone();
        self.sessions
            .iter_mut()
            .find(|s| s.id == id)
            .expect("parse_store keeps the pointer at a session")
    }

    /// Marks the current session busy while a turn is live; a note only means
    /// something on a live turn.
    fn mark_busy(&mut self, repo: &str, turns: &HashMap<String, CancelToken>) {
        let live = turns.contains_key(repo);
        for session in &mut self.sessions {
            // A send that no turn is running any more was cut short by a
            // restart: whether the call went out is unknown, so never redo it.
            if !live {
                for action in session
                    .messages
                    .iter_mut()
                    .filter_map(|m| m.action.as_mut())
                    .filter(|a| a.state == ActionState::Sending)
                {
                    action.state = ActionState::Failed;
                    action.error = Some("Interrupted: it is unknown whether this was sent.".into());
                }
            }
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
                updated_at: s.updated_at(),
                message_count: s.messages.len(),
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
/// thread wrapped into one chat session, else one empty session; either way
/// the store ends up with a session and a pointer to one. The flag says
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
            let created_at = legacy.messages.first().map_or_else(now_ms, |m| m.ts);
            let session = ChatSession {
                title,
                messages: legacy.messages,
                created_at,
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
    for session in &mut chats.sessions {
        if session.created_at == 0 {
            session.created_at = session.messages.first().map_or_else(now_ms, |m| m.ts);
            changed = true;
        }
    }
    if !chats.sessions.iter().any(|s| s.id == chats.current) {
        chats.current = chats
            .sessions
            .last()
            .expect("checked non-empty above")
            .id
            .clone();
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
    chats.mark_busy(repo, &turns);
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
        chats.mark_busy(repo, &turns);
        let _ = store::write_json_atomic(&store_path(app, repo), &chats);
        chats
    };
    broadcast(app, &chats);
}

/// Applies `change` to the repo's store, saves and broadcasts it - refused
/// while a turn is live, since that turn belongs to the current session and
/// moving its pointer under it would orphan the reply. Holding `chat_turns`
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
        chats.mark_busy(repo, &turns);
        store::write_json_atomic(&store_path(app, repo), &chats).map_err(|e| e.to_string())?;
        chats
    };
    broadcast(app, &chats);
    Ok(chats)
}

fn push(session: &mut ChatSession, role: &str, text: &str, mode: ChatMode) {
    if role == "user" && session.title.is_none() {
        session.title = Some(title_for(text));
    }
    session.messages.push(ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: role.to_string(),
        text: truncate_chars(text, MAX_TEXT),
        ts: now_ms(),
        mode,
        draft: None,
        proposal: None,
        questions: Vec::new(),
        tools: Vec::new(),
        action: None,
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

/// Starts an empty session and makes it the current one.
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

/// Empties the current session and forgets its harness session, so the
/// next reply carries no prior context. The title goes too: it named a message
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
        session.mode = ChatMode::Chat;
        Ok(())
    })?;
    Ok(chats.thread_json())
}

/// Serialises `chat.createProposal`, so a double click cannot create a row twice.
static PROPOSAL_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A list of 1-based row numbers from a request, deduplicated.
fn row_numbers(value: &serde_json::Value, len: usize) -> Result<Vec<usize>, String> {
    let list = value
        .as_array()
        .ok_or("indices must be a list of row numbers")?;
    let mut rows: Vec<usize> = Vec::new();
    for n in list {
        let n = n
            .as_u64()
            .map(|n| n as usize)
            .filter(|n| (1..=len).contains(n))
            .ok_or_else(|| format!("row numbers run from 1 to {len}"))?;
        if !rows.contains(&(n - 1)) {
            rows.push(n - 1);
        }
    }
    Ok(rows)
}

/// The proposal row `messageId` carries, with the session it lives in.
fn find_proposal(chats: &ChatStore, message_id: &str) -> Result<(String, ChatProposal), String> {
    chats
        .sessions
        .iter()
        .find_map(|s| {
            let message = s.messages.iter().find(|m| m.id == message_id)?;
            Some((s.id.clone(), message.proposal.clone()))
        })
        .ok_or_else(|| "no such chat message".to_string())
        .and_then(|(session, proposal)| {
            Ok((
                session,
                proposal.ok_or_else(|| "that message proposes no tasks".to_string())?,
            ))
        })
}

/// Creates `rows` (0-based, dependencies first) as tasks, recording each new
/// task's id on its row as soon as it exists. A proposal of two or more rows
/// is one feature: the first rows created make its parent task, recorded on
/// the proposal, and every row created from it, now or later, is a part that
/// branches from and lands on that parent's branch. A one-row proposal stays a
/// task of its own.
async fn create_rows(
    app: &App,
    repo: &str,
    proposal: &mut ChatProposal,
    rows: &[usize],
    backlog: bool,
    task_mcp: Option<&serde_json::Value>,
    created: &mut Vec<serde_json::Value>,
) -> Result<(), String> {
    let mut fresh = None;
    let parent = if proposal.tasks.len() < 2 {
        None
    } else if let Some(id) = live_feature(app, proposal) {
        Some(id)
    } else {
        let id = create_feature(app, repo, proposal, backlog, task_mcp).await?;
        created.push(json!({"feature": true, "taskId": id}));
        proposal.feature_task_id = Some(id.clone());
        fresh = Some(id.clone());
        Some(id)
    };
    let made = create_parts(
        app,
        repo,
        proposal,
        rows,
        backlog,
        parent.as_deref(),
        task_mcp,
        created,
    )
    .await;
    if let Some(id) = fresh {
        if made.is_err() && !created.iter().any(|c| c.get("index").is_some()) {
            // No part exists: a childless parent would implement its own
            // placeholder goal.
            let _ = app.handle_task_delete(json!({"id": id})).await;
            proposal.feature_task_id = None;
            created.retain(|c| c["taskId"] != json!(id));
        } else if backlog {
            // Out of `later` even when a part failed, so the parts created
            // are not stranded.
            app.handle_task_backlog(json!({"id": id, "bucket": "next"}))
                .await?;
        } else {
            app.handle_task_start(json!({"id": id})).await?;
        }
    }
    made
}

/// The proposal's parent while new parts can still join it.
fn live_feature(app: &App, proposal: &ChatProposal) -> Option<String> {
    let id = proposal.feature_task_id.as_deref()?;
    let task = app.store.load_task(id).ok().flatten()?;
    (!task.archived && !matches!(task.status, TaskStatus::Done | TaskStatus::Landing))
        .then_some(task.id)
}

/// The parent of a proposal's rows. Backlogged, it waits in `later` until its
/// parts exist, so the autopilot cannot start it half-built.
async fn create_feature(
    app: &App,
    repo: &str,
    proposal: &ChatProposal,
    backlog: bool,
    task_mcp: Option<&serde_json::Value>,
) -> Result<String, String> {
    let titles: Vec<&str> = proposal.tasks.iter().map(|t| t.title.as_str()).collect();
    let title = if proposal.feature.is_empty() {
        format!("{} and {} more", titles[0], titles.len() - 1)
    } else {
        proposal.feature.clone()
    };
    let mut params = json!({
        "repo": repo,
        "title": title,
        "goal": format!("Every part lands on this task's branch: {}", titles.join("; ")),
        "criteria": titles.iter().map(|t| format!("\"{t}\" landed")).collect::<Vec<_>>(),
        "source": "chat",
        "start": false,
    });
    if backlog {
        params["backlog"] = json!({"bucket": "later"});
    }
    if let Some(mcp) = task_mcp {
        params["mcp"] = mcp.clone();
    }
    let made = app.handle_task_create(params).await?;
    made["id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "task.create returned no id".to_string())
}

#[allow(clippy::too_many_arguments)]
async fn create_parts(
    app: &App,
    repo: &str,
    proposal: &mut ChatProposal,
    rows: &[usize],
    backlog: bool,
    parent: Option<&str>,
    task_mcp: Option<&serde_json::Value>,
    created: &mut Vec<serde_json::Value>,
) -> Result<(), String> {
    for &row in rows {
        let task = &proposal.tasks[row];
        let depends_on: Vec<String> = task
            .depends_on
            .iter()
            .filter_map(|dep| match dep {
                ProposalDep::Index(n) => proposal.tasks[*n - 1].task_id.clone(),
                ProposalDep::Id(id) => Some(id.clone()),
            })
            .collect();
        let mut params = json!({
            "repo": repo,
            "title": task.title,
            "goal": if task.goal.is_empty() { &task.title } else { &task.goal },
            "criteria": task.criteria,
            "dependsOn": depends_on,
            "source": "chat",
            "start": !backlog,
        });
        if let Some(parent) = parent {
            params["parent"] = json!(parent);
        } else if backlog {
            params["backlog"] = json!({"bucket": "next"});
        }
        if let Some(mcp) = task_mcp {
            params["mcp"] = mcp.clone();
        }
        let made = app.handle_task_create(params).await?;
        let id = made["id"]
            .as_str()
            .ok_or("task.create returned no id")?
            .to_string();
        created.push(json!({"index": row + 1, "taskId": id}));
        proposal.tasks[row].task_id = Some(id);
        proposal.tasks[row].skipped = false;
    }
    Ok(())
}

/// Turns the chosen rows of a reply's proposal into tasks, dependencies
/// first, and records what happened on the message: each created row keeps its
/// task id, the rows the owner passed on read as skipped. Rows that already
/// became tasks are left alone, so asking twice creates nothing twice.
///
/// Params: `repo`, `messageId`, `indices` (1-based rows to create),
/// `backlog` (park them in the plan backlog instead of starting them) and
/// `skip` (1-based rows to record as skipped; every other unresolved row
/// without it).
pub async fn handle_create_proposal(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    let message_id = params
        .get("messageId")
        .and_then(|v| v.as_str())
        .ok_or("messageId is required")?
        .to_string();
    let backlog = params
        .get("backlog")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let _one_at_a_time = PROPOSAL_LOCK.lock().await;
    // Holding the turn slot keeps a turn from starting and writing its own
    // copy of the session over the rows recorded here.
    {
        let mut turns = app.chat_turns.lock().unwrap();
        if turns.contains_key(&repo) {
            return Err(BUSY.to_string());
        }
        turns.insert(repo.clone(), CancelToken::new());
    }
    let outcome = create_proposal(app, &repo, &message_id, &params, backlog).await;
    app.chat_turns.lock().unwrap().remove(&repo);
    let (chats, created) = outcome?;
    broadcast(app, &chats);
    let (_, proposal) = find_proposal(&chats, &message_id)?;
    Ok(json!({"proposal": proposal, "created": created}))
}

async fn create_proposal(
    app: &App,
    repo: &str,
    message_id: &str,
    params: &serde_json::Value,
    backlog: bool,
) -> Result<(ChatStore, Vec<serde_json::Value>), String> {
    let chats = load(app, repo);
    let (session_id, mut proposal) = find_proposal(&chats, message_id)?;
    let len = proposal.tasks.len();
    let chosen = row_numbers(params.get("indices").ok_or("indices is required")?, len)?;
    let passed = match params.get("skip") {
        Some(skip) => row_numbers(skip, len)?,
        None => (0..len).filter(|row| !chosen.contains(row)).collect(),
    };
    let todo: Vec<usize> = chosen
        .iter()
        .copied()
        .filter(|&row| proposal.tasks[row].task_id.is_none())
        .collect();
    let ordered = proposal
        .order(&todo)
        .ok_or("the proposed tasks depend on each other in a loop")?;
    for &row in &ordered {
        for dep in proposal.row_deps(row).unwrap_or_default() {
            if proposal.tasks[dep].task_id.is_none() && !todo.contains(&dep) {
                return Err(format!(
                    "\"{}\" depends on \"{}\", which is not created or selected",
                    proposal.tasks[row].title, proposal.tasks[dep].title
                ));
            }
        }
    }
    let task_mcp = chats
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .and_then(|s| s.task_mcp.clone());
    let mut created = Vec::new();
    let made = create_rows(
        app,
        repo,
        &mut proposal,
        &ordered,
        backlog,
        task_mcp.as_ref(),
        &mut created,
    )
    .await;
    if made.is_ok() {
        for (row, task) in proposal.tasks.iter_mut().enumerate() {
            if task.task_id.is_none() {
                task.skipped = passed.contains(&row) && !chosen.contains(&row);
            }
        }
    }
    // Whatever was created is recorded, even when a later row failed.
    let chats = {
        let _turns = app.chat_turns.lock().unwrap();
        let mut chats = read_store(app, repo);
        if let Some(message) = chats
            .sessions
            .iter_mut()
            .find(|s| s.id == session_id)
            .and_then(|s| s.messages.iter_mut().find(|m| m.id == message_id))
        {
            message.proposal = Some(proposal);
        }
        // The turn slot is only a placeholder that serialises writers; no
        // turn runs, so nothing stored or broadcast may say busy.
        chats.mark_busy(repo, &HashMap::new());
        store::write_json_atomic(&store_path(app, repo), &chats).map_err(|e| e.to_string())?;
        chats
    };
    match made {
        Ok(()) => Ok((chats, created)),
        Err(error) => {
            broadcast(app, &chats);
            Err(error)
        }
    }
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
    Owner(ChatMode),
    Questions(Vec<String>),
}

const QUESTIONS_INTRO: &str = "Tasks asked you the questions below while they keep working. Answer each with orchestrator_reply {question, text}; a task reads the answer on its next attempt.";
const QUESTION_TURN_REPLY: &str =
    "Your final message is sent back to each task you did not answer that way.";

/// Claims the repo's turn slot and loads the current session, starting a fresh harness session when the orchestrator's route
/// changed since the session's last turn.
fn begin_turn(app: &App, repo: &str) -> Result<(ChatSession, Route, CancelToken), String> {
    let settings = app.settings.read().unwrap().clone();
    let route = app
        .chat_route(&settings)
        .ok_or("no route is configured for the orchestrator")?;
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
    let mode = ChatMode::param(&params)?;
    let (mut session, route, cancel) = begin_turn(app, &repo)?;
    if let Some(mcp) = params.get("mcp") {
        session.task_mcp = Some(mcp.clone());
    }
    let notes = take_action_notes(&mut session);
    push(&mut session, "user", &text, mode);
    session.mode = mode;
    session.error = None;
    save(app, &repo, &session);
    // Questions waiting for the orchestrator ride along with the owner's
    // message; the chat shows only what the owner wrote. Brainstorm and Plan
    // turns never answer task questions.
    let waiting = match mode {
        ChatMode::Chat => messages::take_for_orchestrator(app, &repo),
        _ => None,
    };
    let prompt = match waiting {
        Some((_, questions)) => format!("{text}\n\n---\n\n{QUESTIONS_INTRO}\n\n{questions}"),
        None => text,
    };
    let prompt = format!("{notes}{prompt}");
    let app = app.arc();
    tokio::spawn(async move {
        run_turn(
            &app,
            repo,
            session,
            route,
            prompt,
            cancel,
            Turn::Owner(mode),
        )
        .await;
    });
    Ok(json!({}))
}

const HISTORY_INTRO: &str = "This conversation was edited: the owner changed an earlier message, so the replies that followed it are gone. Here is what was said before it, for context only (do not answer it again):";
const MAX_HISTORY_TEXT: usize = 1500;

/// The kept part of an edited conversation, compact, for a fresh harness
/// session that has no memory of it.
fn history_prompt(kept: &[ChatMessage]) -> String {
    if kept.is_empty() {
        return String::new();
    }
    let lines = kept
        .iter()
        .map(|m| {
            let who = if m.role == "user" {
                "Owner"
            } else {
                "Orchestrator"
            };
            format!("{who}: {}", truncate_chars(m.text.trim(), MAX_HISTORY_TEXT))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{HISTORY_INTRO}\n\n{lines}\n\n---\n\nThe owner's edited message:\n\n")
}

/// Replaces one of the owner's messages, drops everything after it (with its
/// proposals, pending actions, draft and questions) and runs a new turn from
/// there on a fresh harness session that is given the kept history. Tasks
/// already created from a dropped proposal stay. Refused while a turn runs.
pub async fn handle_edit(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    let message_id = message_id_param(&params)?;
    let text = params
        .get("text")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or("text is required")?
        .to_string();
    let requested_mode = match params.get("mode") {
        None | Some(serde_json::Value::Null) => None,
        Some(_) => Some(ChatMode::param(&params)?),
    };
    let (mut session, route, cancel) = begin_turn(app, &repo)?;
    let Some(index) = session
        .messages
        .iter()
        .position(|m| m.id == message_id && m.role == "user")
    else {
        app.chat_turns.lock().unwrap().remove(&repo);
        return Err("that message is not one of the owner's messages in this chat".to_string());
    };
    let mode = requested_mode.unwrap_or(session.messages[index].mode);
    if let Some(mcp) = params.get("mcp") {
        session.task_mcp = Some(mcp.clone());
    }
    session.messages.truncate(index + 1);
    {
        let message = &mut session.messages[index];
        message.text = truncate_chars(&text, MAX_TEXT);
        message.mode = mode;
        message.ts = now_ms();
    }
    if index == 0 {
        session.title = Some(title_for(&text));
    }
    let history = history_prompt(&session.messages[..index]);
    // The old harness session remembers the dropped part: start a new one.
    session.session_id = None;
    let notes = take_action_notes(&mut session);
    session.mode = mode;
    session.error = None;
    session.note = None;
    save(app, &repo, &session);
    let waiting = match mode {
        ChatMode::Chat => messages::take_for_orchestrator(app, &repo),
        _ => None,
    };
    let prompt = match waiting {
        Some((_, questions)) => format!("{text}\n\n---\n\n{QUESTIONS_INTRO}\n\n{questions}"),
        None => text,
    };
    let prompt = format!("{notes}{history}{prompt}");
    let app = app.arc();
    tokio::spawn(async move {
        run_turn(
            &app,
            repo,
            session,
            route,
            prompt,
            cancel,
            Turn::Owner(mode),
        )
        .await;
    });
    Ok(json!({}))
}

/// What became of the write calls the orchestrator proposed, for its next
/// turn: it cannot see the owner's OK card. Each call is reported once.
fn take_action_notes(session: &mut ChatSession) -> String {
    let mut notes = Vec::new();
    for action in session
        .messages
        .iter_mut()
        .filter_map(|m| m.action.as_mut())
        .filter(|a| !a.noted)
    {
        let what = format!("{} ({}: {})", action.summary, action.server, action.tool);
        let note = match action.state {
            ActionState::Sent => format!(
                "The owner approved {what} and it ran. Result: {}",
                action.result.as_deref().unwrap_or("none")
            ),
            ActionState::Failed => format!(
                "The owner approved {what} but it failed: {}",
                action.error.as_deref().unwrap_or("unknown error")
            ),
            ActionState::Declined => {
                format!("The owner declined {what}. Do not propose it again unless asked.")
            }
            _ => continue,
        };
        action.noted = true;
        notes.push(note);
    }
    if notes.is_empty() {
        String::new()
    } else {
        format!("[Connected tools]\n{}\n\n---\n\n", notes.join("\n"))
    }
}

/// Starts a turn for the questions tasks sent the orchestrator in `repo`,
/// in the owner's current chat session, in the plain chat mode. A turn already running picks
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

fn mcp_server(app: &App, task_mcp_file: Option<&Path>, mode: ChatMode) -> serde_json::Value {
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
    if mode.read_only() {
        server["args"]
            .as_array_mut()
            .expect("args is an array")
            .push(json!("--read-only"));
    }
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
    mode: ChatMode,
    connected: &chat_tools::TurnTools,
) -> Vec<String> {
    let offered: &[&str] = if mode.read_only() {
        &crate::mcp::READ_ONLY_TOOLS
    } else {
        &crate::mcp::ORCHESTRATOR_TOOLS
    };
    let tools = offered
        .iter()
        .map(|t| format!("mcp__{SERVER}__{t}"))
        .chain(connected.allowed.iter().cloned())
        .collect::<Vec<_>>()
        .join(",");
    let prompt = format!("{}{}", mode.prompt(), connected.prompt);
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
        &prompt,
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
    mode: ChatMode,
    connected: &chat_tools::TurnTools,
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
    argv.push(format!(
        "developer_instructions={}",
        toml(&json!(format!("{}{}", mode.prompt(), connected.prompt)))
    ));
    argv.extend(harness::codex_mcp_flags(SERVER, server));
    argv.extend(chat_tools::codex_flags(connected));
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

/// Adds the agent's reply, in the mode of the turn, to the session.
fn apply_reply(
    thread: &mut ChatSession,
    reply: &str,
    tools: Vec<ChatTool>,
    mode: ChatMode,
    proposable: &dyn Fn(&str, &str) -> bool,
) {
    let (reply, action) = take_action(reply, proposable);
    let parsed = parse_reply(&reply);
    push(thread, "assistant", &parsed.text, mode);
    let message = thread.messages.last_mut().expect("just pushed");
    message.draft = parsed.draft;
    message.proposal = parsed.proposal;
    message.questions = parsed.questions;
    message.tools = tools;
    message.action = action;
}

#[derive(Deserialize)]
struct ActionBlock {
    server: String,
    tool: String,
    #[serde(default)]
    args: serde_json::Value,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    target: String,
}

/// The reply without its `sushi-action` block, and the pending action the
/// block proposed. A block that does not parse, or names a call
/// `proposable` refuses (an unknown service, a read tool), is no action: the
/// reply stays as it is.
fn take_action(
    reply: &str,
    proposable: &dyn Fn(&str, &str) -> bool,
) -> (String, Option<ChatAction>) {
    let untouched = || (reply.to_string(), None);
    let Some(body) = crate::brief::last_fenced_block(reply, "sushi-action") else {
        return untouched();
    };
    let Ok(block) = serde_json::from_str::<ActionBlock>(&body) else {
        return untouched();
    };
    let Some((start, end)) = block_span(reply, "sushi-action") else {
        return untouched();
    };
    let args = match block.args {
        serde_json::Value::Null => json!({}),
        args if args.is_object() => args,
        _ => return untouched(),
    };
    if !proposable(&block.server, &block.tool) {
        return untouched();
    }
    let summary = one_line(block.summary.trim());
    let before = reply[..start].trim_end();
    let after = reply[end..].trim_start();
    let mut text = [before, after]
        .iter()
        .filter(|part| !part.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() {
        text = if summary.is_empty() {
            format!("{}: {}", block.server, block.tool)
        } else {
            summary.clone()
        };
    }
    let action = ChatAction {
        id: uuid::Uuid::new_v4().to_string(),
        server: block.server,
        tool: block.tool,
        args,
        summary,
        target: one_line(block.target.trim()),
        state: ActionState::Pending,
        error: None,
        result: None,
        noted: false,
    };
    (text, Some(action))
}

const TOOL_PREFIX: &str = "mcp__sushiai-orchestrator__";
const MAX_TOOLS: usize = 8;
const MAX_SUMMARY: usize = 120;

/// Tools whose input names a task by `id`.
const TASK_ID_TOOLS: [&str; 10] = [
    "task_get",
    "task_backlog",
    "task_start",
    "task_stop",
    "task_report",
    "task_answer",
    "task_amend",
    "task_archive",
    "task_unarchive",
    "task_lead_touch",
];

fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() > MAX_SUMMARY {
        format!(
            "{}…",
            line.chars().take(MAX_SUMMARY - 1).collect::<String>()
        )
    } else {
        line.to_string()
    }
}

fn tool_summary(app: &App, repo: &str, name: &str, input: &serde_json::Value) -> String {
    let field = |key: &str| input.get(key).and_then(|v| v.as_str());
    if TASK_ID_TOOLS.contains(&name) {
        let title = field("id")
            .and_then(|id| app.store.load_task(id).ok().flatten())
            .map(|task| task.title);
        return title.map(|t| one_line(&t)).unwrap_or_default();
    }
    match name {
        "task_create" => field("title").map(one_line).unwrap_or_default(),
        "Read" | "Grep" | "Glob" => {
            let detail = field("file_path")
                .map(|path| {
                    let path = Path::new(path);
                    path.strip_prefix(repo).unwrap_or(path).to_string_lossy()
                })
                .or_else(|| field("pattern").map(Into::into));
            detail.map(|d| one_line(&d)).unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn chat_tool(app: &App, repo: &str, name: &str, input: &serde_json::Value) -> ChatTool {
    let name = name.strip_prefix(TOOL_PREFIX).unwrap_or(name);
    ChatTool {
        name: name.to_string(),
        summary: tool_summary(app, repo, name, input),
    }
}

/// The tool calls one stdout line reports. Codex reports an MCP call twice
/// (started and completed), so `seen` holds the item ids already counted.
fn tools_in_line(
    app: &App,
    repo: &str,
    harness: Harness,
    line: &str,
    seen: &mut std::collections::HashSet<String>,
) -> Vec<ChatTool> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let kind = v.get("type").and_then(|t| t.as_str());
    match (harness, kind) {
        (Harness::Claude, Some("assistant")) => {
            if !v.get("parent_tool_use_id").is_none_or(|p| p.is_null()) {
                return Vec::new();
            }
            let blocks = v.pointer("/message/content").and_then(|c| c.as_array());
            blocks
                .into_iter()
                .flatten()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                .filter_map(|b| {
                    let name = b.get("name").and_then(|n| n.as_str())?;
                    Some(chat_tool(
                        app,
                        repo,
                        name,
                        b.get("input").unwrap_or(&json!({})),
                    ))
                })
                .collect()
        }
        (Harness::Codex, Some("item.started" | "item.completed")) => {
            let item = &v["item"];
            let (Some("mcp_tool_call"), Some(id), Some(tool)) = (
                item.get("type").and_then(|t| t.as_str()),
                item.get("id").and_then(|i| i.as_str()),
                item.get("tool").and_then(|t| t.as_str()),
            ) else {
                return Vec::new();
            };
            if !seen.insert(id.to_string()) {
                return Vec::new();
            }
            let empty = json!({});
            vec![chat_tool(
                app,
                repo,
                tool,
                item.get("arguments").unwrap_or(&empty),
            )]
        }
        _ => Vec::new(),
    }
}

/// Appends calls to a turn's list: a call identical to the one before it
/// counts once and only the first `MAX_TOOLS` are kept.
fn keep_tools(list: &mut Vec<ChatTool>, calls: Vec<ChatTool>) {
    for call in calls {
        if list.len() < MAX_TOOLS && list.last() != Some(&call) {
            list.push(call);
        }
    }
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
    let mode = match &turn {
        Turn::Owner(mode) => *mode,
        Turn::Questions(_) => ChatMode::Chat,
    };
    let dir = turn_dir(app, &repo);
    let _ = std::fs::create_dir_all(&dir);
    // Brainstorm and Plan never create tasks, so they have no task MCP to hand on.
    let task_mcp = thread.task_mcp.as_ref().filter(|_| !mode.read_only());
    let task_mcp_file = task_mcp.and_then(|mcp| {
        let file = dir.join("task-mcp.json");
        store::write_json_atomic(&file, &json!({"repo": repo, "mcp": mcp}))
            .ok()
            .map(|_| file)
    });
    let server = mcp_server(app, task_mcp_file.as_deref(), mode);
    let settings = app.settings.read().unwrap().clone();
    // A question turn answers a task, not the owner: it gets no connected tools.
    let connected = match &turn {
        Turn::Owner(_) => chat_tools::for_turn(app, &settings).await,
        Turn::Questions(_) => chat_tools::TurnTools::default(),
    };
    let session = thread.session_id.clone();
    let key_path = dir.join("key");
    let argv = match route.harness {
        Harness::Claude => {
            let mcp_config = dir.join("mcp.json");
            let mut servers = serde_json::Map::new();
            for (key, def) in &connected.servers {
                servers.insert(key.clone(), def.clone());
            }
            servers.insert(SERVER.to_string(), server);
            let _ = store::write_json_atomic(&mcp_config, &json!({"mcpServers": servers}));
            let settings_path = dir.join("settings.json");
            write_readonly_claude_settings_with_profile(
                &route,
                app,
                &key_path,
                &settings,
                &[app.data_dir.to_string_lossy().to_string()],
                &settings_path,
            );
            claude_argv(
                &route,
                &mcp_config,
                &settings_path,
                session.as_deref(),
                mode,
                &connected,
            )
        }
        Harness::Codex => codex_argv(&route, &repo, &server, session.as_deref(), mode, &connected),
    };

    let (result, tools) = match run(app, &repo, &mut thread, &route, &argv, &text, &cancel).await {
        Ok((outcome, tools)) => (Ok(outcome), tools),
        Err(error) => (Err(error), Vec::new()),
    };
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
                (Some(reply), _, Turn::Owner(mode)) => {
                    apply_reply(&mut thread, &reply, tools, *mode, &|server, tool| {
                        chat_tools::proposable(app, &settings, server, tool)
                    });
                }
                (Some(reply), _, Turn::Questions(ids)) => {
                    messages::reply_where_unanswered(app, ids, &reply)
                }
                (None, Some(error), _) => {
                    // A session the CLI no longer knows fails every resume.
                    thread.session_id = None;
                    if matches!(turn, Turn::Owner(_)) {
                        thread.error = Some(truncate_chars(&error, 2000));
                    }
                }
                (None, None, Turn::Owner(_)) => {
                    thread.error = Some("The orchestrator gave no reply.".into())
                }
                (None, None, Turn::Questions(_)) => {}
            }
        }
        (Err(RunError::Cancelled), Turn::Owner(_)) => thread.error = Some("Stopped.".into()),
        (Err(RunError::Io(error) | RunError::NotFound(error)), Turn::Owner(_)) => {
            thread.error = Some(error)
        }
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
) -> Result<(harness::RunOutcome, Vec<ChatTool>), RunError> {
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
    let mut tools = Vec::new();
    let mut seen_items = std::collections::HashSet::new();
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
                    chat_tools::note_init(app, &l);
                    keep_tools(
                        &mut tools,
                        tools_in_line(app, repo, harness_kind, &l, &mut seen_items),
                    );
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
                            crate::harness::exit_error(harness_kind, status)
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
    Ok((outcome, tools))
}

fn message_id_param(params: &serde_json::Value) -> Result<String, String> {
    params
        .get("messageId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "messageId is required".to_string())
}

fn pending_action<'a>(
    session: &'a mut ChatSession,
    message_id: &str,
) -> Result<&'a mut ChatAction, String> {
    let action = session
        .messages
        .iter_mut()
        .find(|m| m.id == message_id)
        .and_then(|m| m.action.as_mut())
        .ok_or("no such action")?;
    if action.state != ActionState::Pending {
        return Err(format!("the action is already {:?}", action.state).to_lowercase());
    }
    Ok(action)
}

/// Runs the write call the orchestrator proposed, once, with the owner's OK.
/// The call takes the repo's turn slot, so nothing else can rewrite the
/// session while it runs; `args` replaces the proposed arguments (Edit).
pub async fn handle_action_send(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    let message_id = message_id_param(&params)?;
    let edited = match params.get("args") {
        None | Some(serde_json::Value::Null) => None,
        Some(args) if args.is_object() => Some(args.clone()),
        Some(_) => return Err("args must be an object".to_string()),
    };
    let (mut session, action) = {
        let mut turns = app.chat_turns.lock().unwrap();
        if turns.contains_key(&repo) {
            return Err(BUSY.to_string());
        }
        let mut session = read_store(app, &repo).current().clone();
        let action = pending_action(&mut session, &message_id)?;
        if let Some(args) = edited {
            action.args = args;
        }
        action.state = ActionState::Sending;
        let action = action.clone();
        turns.insert(repo.clone(), CancelToken::new());
        (session, action)
    };
    save(app, &repo, &session);
    let app = app.arc();
    tokio::spawn(async move {
        let settings = app.settings.read().unwrap().clone();
        let outcome = match settings.chat_tools.iter().find(|c| c.id == action.server) {
            Some(cfg) => chat_tools::execute(&app, cfg, &action.tool, &action.args).await,
            None => Err("the connected tool was removed from Settings".to_string()),
        };
        let slot = session
            .messages
            .iter_mut()
            .find(|m| m.id == message_id)
            .and_then(|m| m.action.as_mut())
            .expect("the message was in the session a moment ago");
        let text = match outcome {
            Ok(result) => {
                slot.state = ActionState::Sent;
                let done = format!("Sent: {}.", slot.summary);
                let result = result.trim().to_string();
                slot.result = Some(truncate_chars(&result, 2000));
                if result.is_empty() {
                    done
                } else {
                    format!("{done}\n\n{}", truncate_chars(&result, 2000))
                }
            }
            Err(error) => {
                slot.state = ActionState::Failed;
                slot.error = Some(truncate_chars(&error, 2000));
                format!(
                    "Could not send: {}.\n\n{}",
                    slot.summary,
                    truncate_chars(&error, 2000)
                )
            }
        };
        push(&mut session, "assistant", &text, ChatMode::Chat);
        finish_turn(&app, &repo, &session);
    });
    Ok(json!({}))
}

/// The owner's "Don't send": recorded, and told to the orchestrator on its
/// next turn.
pub async fn handle_action_decline(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    let message_id = message_id_param(&params)?;
    let chats = change_idle(app, &repo, |chats| {
        let action = pending_action(chats.current_mut(), &message_id)?;
        action.state = ActionState::Declined;
        Ok(())
    })?;
    Ok(chats.thread_json())
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
            account_id: None,
            strength: None,
        }
    }

    #[test]
    fn claude_chat_reads_only_and_reaches_orchd_through_one_allowed_tools_value() {
        let argv = claude_argv(
            &route(Harness::Claude),
            Path::new("/d/mcp.json"),
            Path::new("/d/settings.json"),
            Some("sess"),
            ChatMode::Chat,
            &chat_tools::TurnTools::default(),
        );
        let at = |flag: &str| argv[argv.iter().position(|a| a == flag).unwrap() + 1].clone();
        assert_eq!(at("--tools"), "Read,Grep,Glob");
        assert_eq!(at("--mcp-config"), "/d/mcp.json");
        assert_eq!(at("--resume"), "sess");
        assert_eq!(
            at("--allowedTools").split(',').count(),
            crate::mcp::ORCHESTRATOR_TOOLS.len()
        );
        assert!(at("--allowedTools").contains("mcp__sushiai-orchestrator__orchestrator_reply"));
        assert!(!at("--allowedTools").contains("task_delete"));
        assert_eq!(
            at("--append-system-prompt"),
            crate::prompts::default("chat")
        );
        assert!(!argv.iter().any(|a| a == "acceptEdits"));
    }

    #[test]
    fn codex_chat_is_read_only_fresh_or_resumed() {
        let server = json!({"command": "/o", "args": ["mcp"], "env": {"ORCHD_TASK_MCP": "/f"}});
        let fresh = codex_argv(
            &route(Harness::Codex),
            "/repo",
            &server,
            None,
            ChatMode::Chat,
            &chat_tools::TurnTools::default(),
        );
        assert_eq!(&fresh[..2], &["exec".to_string(), "--json".to_string()]);
        assert!(fresh
            .windows(2)
            .any(|w| w[0] == "--sandbox" && w[1] == "read-only"));
        assert!(fresh
            .contains(&"mcp_servers.sushiai-orchestrator.env.ORCHD_TASK_MCP=\"/f\"".to_string()));
        let resumed = codex_argv(
            &route(Harness::Codex),
            "/repo",
            &server,
            Some("t1"),
            ChatMode::Chat,
            &chat_tools::TurnTools::default(),
        );
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
        app.assume_harnesses_installed();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        (app, dir, repo.to_string_lossy().to_string())
    }

    /// Writes `session` into the repo's store as a finished turn would.
    fn converse(app: &App, repo: &str, text: &str, harness_session: &str) {
        let mut session = load(app, repo).current().clone();
        push(&mut session, "user", text, ChatMode::Chat);
        push(&mut session, "assistant", "ok", ChatMode::Chat);
        session.session_id = Some(harness_session.to_string());
        save(app, repo, &session);
    }

    fn proposal_session(app: &App, repo: &str) -> String {
        let mut session = load(app, repo).current().clone();
        push(&mut session, "user", "plan it", ChatMode::Plan);
        push(&mut session, "assistant", "here", ChatMode::Plan);
        let task = |title: &str| ProposedTask {
            title: title.to_string(),
            goal: title.to_string(),
            ..Default::default()
        };
        let message = session.messages.last_mut().unwrap();
        message.proposal = Some(ChatProposal {
            feature: "Numbers".into(),
            tasks: vec![task("One"), task("Two"), task("Three")],
            ..Default::default()
        });
        let id = message.id.clone();
        save(app, repo, &session);
        id
    }

    fn git_repo(repo: &str) {
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        ] {
            std::process::Command::new("git")
                .args(args)
                .current_dir(repo)
                .status()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn rows_created_together_are_parts_of_one_feature() {
        for backlog in [false, true] {
            let (app, _dir, repo) = test_app();
            git_repo(&repo);
            let message = proposal_session(&app, &repo);
            let made = handle_create_proposal(
                &app,
                json!({"repo": repo, "messageId": message, "indices": [1, 2], "backlog": backlog}),
            )
            .await
            .unwrap();
            let created = made["created"].as_array().unwrap();
            let feature = created
                .iter()
                .find(|c| c["feature"] == json!(true))
                .unwrap()["taskId"]
                .as_str()
                .unwrap()
                .to_string();
            let parent = app.store.load_task(&feature).unwrap().unwrap();
            assert_eq!(parent.title, "Numbers");
            assert_eq!(
                parent.queue.backlog.map(|b| b.bucket),
                backlog.then_some(crate::model::BacklogBucket::Next)
            );
            for row in made["proposal"]["tasks"].as_array().unwrap().iter().take(2) {
                let part = app
                    .store
                    .load_task(row["taskId"].as_str().unwrap())
                    .unwrap()
                    .unwrap();
                assert_eq!(part.parent.as_deref(), Some(feature.as_str()));
                assert_eq!(part.base_ref.as_deref(), Some(parent.branch.as_str()));
                assert!(part.queue.backlog.is_none());
                if backlog {
                    assert_eq!(part.status, crate::model::TaskStatus::Queued);
                }
            }
            // A row created later joins the same feature.
            let made = handle_create_proposal(
                &app,
                json!({"repo": repo, "messageId": message, "indices": [3]}),
            )
            .await
            .unwrap();
            assert_eq!(made["created"].as_array().unwrap().len(), 1);
            let late = made["created"][0]["taskId"].as_str().unwrap();
            let late = app.store.load_task(late).unwrap().unwrap();
            assert_eq!(late.parent.as_deref(), Some(feature.as_str()));
            app.shutdown();
        }
    }

    #[tokio::test]
    async fn a_feature_whose_first_part_fails_is_removed() {
        let (app, _dir, repo) = test_app();
        git_repo(&repo);
        let mut session = load(&app, &repo).current().clone();
        push(&mut session, "user", "plan it", ChatMode::Plan);
        push(&mut session, "assistant", "here", ChatMode::Plan);
        let message = session.messages.last_mut().unwrap();
        message.proposal = Some(ChatProposal {
            tasks: vec![
                ProposedTask {
                    title: "One".into(),
                    depends_on: vec![ProposalDep::Id("gone".into())],
                    ..Default::default()
                },
                ProposedTask {
                    title: "Two".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let id = message.id.clone();
        save(&app, &repo, &session);

        let error = handle_create_proposal(
            &app,
            json!({"repo": repo, "messageId": id, "indices": [1, 2]}),
        )
        .await
        .unwrap_err();
        assert!(error.contains("unknown dependency"), "{error}");
        assert!(app.repo_tasks(&repo).is_empty());
        let (_, proposal) = find_proposal(&load(&app, &repo), &id).unwrap();
        assert!(proposal.feature_task_id.is_none());
        app.shutdown();
    }

    #[tokio::test]
    async fn creating_from_a_proposal_never_leaves_the_chat_busy() {
        let (app, _dir, repo) = test_app();
        for args in [
            vec!["init", "-q"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        ] {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap();
        }
        let message = proposal_session(&app, &repo);
        let mut events = app.subscribe();
        let calls = [
            json!({"indices": [1], "skip": []}),
            json!({"indices": [2], "backlog": true, "skip": []}),
            json!({"indices": [3]}),
        ];
        for call in calls {
            let mut params = call;
            params["repo"] = json!(repo);
            params["messageId"] = json!(message);
            handle_create_proposal(&app, params).await.unwrap();
            let stored = load(&app, &repo);
            assert!(stored.sessions.iter().all(|s| !s.busy));
            let on_disk: ChatStore =
                serde_json::from_str(&std::fs::read_to_string(store_path(&app, &repo)).unwrap())
                    .unwrap();
            assert!(on_disk.sessions.iter().all(|s| !s.busy));
        }
        let mut seen = 0;
        while let Ok(event) = events.try_recv() {
            if let Event::Chat {
                thread, sessions, ..
            } = event
            {
                seen += 1;
                assert_eq!(thread["busy"], json!(false), "{thread}");
                assert!(sessions
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|s| s["busy"] == json!(false)));
            }
        }
        assert!(seen >= 3);
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
        assert_eq!(chats.summaries().len(), 2);
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
        assert_eq!(chats.summaries().len(), 2);
        assert_eq!(chats.current, second);
        assert_eq!(chats.current().messages.len(), 2);

        // The turn's final save frees the slot in the same step, so the reply
        // lands in the session it belongs to and the same requests go through.
        let mut reply = chats.current().clone();
        push(&mut reply, "assistant", "done", ChatMode::Chat);
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
            push(&mut thread, "user", &i.to_string(), ChatMode::Chat);
        }
        assert_eq!(thread.messages.len(), MAX_MESSAGES);
        assert_eq!(thread.messages[0].text, "5");
    }

    fn block(json: &str) -> String {
        format!("```sushi-draft\n{json}\n```")
    }

    #[test]
    fn the_last_draft_block_wins_and_prose_around_it_stays() {
        let reply = format!(
            "Before.\n\n{}\n\nMiddle.\n\n{}\n\nAfter.",
            block(r#"{"title":"First"}"#),
            block(
                r#"{"title":"Second","goal":"g","criteria":["a"],"dependsOn":["t1"],"tier":"hard"}"#
            )
        );
        let parsed = parse_reply(&reply);
        let draft = parsed.draft.unwrap();
        assert_eq!(draft.title, "Second");
        assert_eq!(draft.goal, "g");
        assert_eq!(draft.criteria, vec!["a"]);
        assert_eq!(draft.depends_on, vec!["t1"]);
        assert_eq!(draft.tier, Some(Tier::Hard));
        assert!(parsed.text.starts_with("Before."));
        assert!(parsed.text.contains("Middle."));
        assert!(parsed.text.ends_with("After."));
        assert!(
            parsed.text.contains("sushi-draft"),
            "the earlier block stays"
        );
        assert!(!parsed.text.contains("Second"));
    }

    #[test]
    fn stripping_covers_a_fence_closed_on_the_json_line_and_the_tag_form() {
        let inline = parse_reply("Intro.\n```sushi-draft\n{\"title\":\"T\"}```\nOutro.");
        assert_eq!(inline.text, "Intro.\n\nOutro.");
        let tagged = parse_reply("Intro. <sushi-draft>{\"title\":\"T\"}</sushi-draft> Outro.");
        assert_eq!(tagged.text, "Intro.\n\nOutro.");
        assert_eq!(tagged.draft.unwrap().title, "T");
    }

    #[test]
    fn a_malformed_or_empty_block_is_no_block() {
        for reply in [
            block("{not json"),
            block(r#"{"title":"  ","goal":"g","questions":[{"text":""}]}"#),
            block("[]"),
        ] {
            let parsed = parse_reply(&format!("Hi\n{reply}"));
            assert_eq!(parsed.text, format!("Hi\n{reply}"));
            assert!(parsed.draft.is_none());
            assert!(parsed.questions.is_empty());
        }
    }

    #[test]
    fn an_unknown_tier_is_none_and_text_criteria_read_as_text() {
        let parsed = parse_reply(&block(
            r#"{"title":"T","tier":"huge","criteria":[{"text":"c1"},"c2",5]}"#,
        ));
        let draft = parsed.draft.unwrap();
        assert_eq!(draft.tier, None);
        assert_eq!(draft.criteria, vec!["c1", "c2"]);
    }

    #[test]
    fn a_questions_only_block_has_no_draft_and_empty_questions_are_dropped() {
        let parsed = parse_reply(&format!(
            "Which one?\n{}",
            block(r#"{"questions":[{"text":"A or B?","options":["A","B"]},{"text":" "}]}"#)
        ));
        assert!(parsed.draft.is_none());
        assert_eq!(parsed.text, "Which one?");
        assert_eq!(
            parsed.questions,
            vec![ChatQuestion {
                text: "A or B?".into(),
                options: vec!["A".into(), "B".into()]
            }]
        );
    }

    #[test]
    fn a_block_only_reply_reads_as_the_title_else_the_first_question() {
        let titled = parse_reply(&block(r#"{"title":"Do it","questions":[{"text":"Q?"}]}"#));
        assert_eq!(titled.text, "Do it");
        let asked = parse_reply(&block(r#"{"questions":[{"text":"Q1?"},{"text":"Q2?"}]}"#));
        assert_eq!(asked.text, "Q1?");
    }

    #[tokio::test]
    async fn summaries_carry_timestamps_and_counts_but_no_bodies() {
        let (app, _dir, repo) = test_app();
        let empty = handle_list(&app, json!({"repo": repo})).await.unwrap();
        assert!(empty["sessions"][0]["updatedAt"].as_i64().unwrap() > 0);
        converse(&app, &repo, "Hello there", "h");
        let list = handle_list(&app, json!({"repo": repo})).await.unwrap();
        let thread = handle_get(&app, json!({"repo": repo})).await.unwrap();
        let summary = &list["sessions"][0];
        assert_eq!(summary["title"], "Hello there");
        assert_eq!(summary["busy"], false);
        assert_eq!(summary["messageCount"], 2);
        assert_eq!(summary["updatedAt"], thread["messages"][1]["ts"]);
        assert!(summary.get("messages").is_none());
    }

    const CLAUDE_TASK_GET: &str = r##"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_011CfNVcnvh4Z3JFfM2rvWjU","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_018JzSDbZMnWxhb9CbN7KP8P","name":"mcp__sushiai-orchestrator__task_get","input":{"id":"eb6845cb-12d8-44a5-8e37-ad577410ae22"},"caller":{"type":"direct"}}],"container":null,"stop_reason":null,"stop_sequence":null,"stop_details":null,"usage":{"input_tokens":2,"cache_creation_input_tokens":1093,"cache_read_input_tokens":15516,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1093},"output_tokens":8,"service_tier":"standard","inference_geo":"not_available"},"input_transformations":[],"diagnostics":null,"context_management":null},"parent_tool_use_id":null,"session_id":"7c0af684-01e9-4264-8072-13b0dfee9284","uuid":"ecd1d7e9-6b54-438f-b970-222c42648ba8","timestamp":"2026-09-24T15:23:12.434Z","request_id":"req_011CfNVcnRg3KH9QGyJ5yp1U","wire_tool_inputs":{"toolu_018JzSDbZMnWxhb9CbN7KP8P":{"id":"eb6845cb-12d8-44a5-8e37-ad577410ae22"}},"tool_use_meta":[{"id":"toolu_018JzSDbZMnWxhb9CbN7KP8P","display_name":"Task Get","server_display_name":"sushiai-orchestrator"}]}"##;
    const CLAUDE_SUBAGENT_CALL: &str = r##"{"type":"assistant","message":{"model":"claude-sonnet-5","id":"msg_011CfNbyzMp8S5EiSJ3qs9ji","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_01R45q2hpFap9LAz5gHbVb41","name":"Bash","input":{"command":"grep -n \"orchestrator\" src/WorkspacePanels.tsx src/PanelIcon.tsx 2>/dev/null; echo \"---\"; grep -rn \"orchestrator\" src/App.tsx 2>/dev/null; echo \"---Sidebar---\"; ls src | grep -i sidebar"},"caller":{"type":"direct"}}],"container":null,"stop_reason":null,"stop_sequence":null,"stop_details":null,"usage":{"input_tokens":2,"cache_creation_input_tokens":9011,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":9011,"ephemeral_1h_input_tokens":0},"output_tokens":4,"service_tier":"standard","inference_geo":"not_available"},"input_transformations":[],"diagnostics":null,"context_management":null},"parent_tool_use_id":"toolu_01Lh54Qd1nHYeUUnnqgVJV6t","session_id":"230c3739-45f1-44a7-b3c5-08d1d59ed818","uuid":"d7d7435e-872b-4a20-8a7d-73cdd1f5e806","timestamp":"2026-09-24T16:46:41.108Z","request_id":"req_011CfNbyywkXgqVNioPJzxr6","subagent_type":"Explore","task_description":"Find how to open orchestrator panel in UI"}"##;
    // Real `cua_repl` pair from a task run's events.jsonl, kept as is.
    const CODEX_CUA_STARTED: &str = r##"{"type":"item.started","item":{"id":"item_10","type":"mcp_tool_call","server":"cua_repl","tool":"js","arguments":{"code":"const app = await cua.getApp('sushiAI'); await app.getAXStateAndScreenshot()","title":"Inspect the sushiAI panel"},"result":null,"error":null,"status":"in_progress"}}"##;
    const CODEX_CUA_COMPLETED: &str = r##"{"type":"item.completed","item":{"id":"item_10","type":"mcp_tool_call","server":"cua_repl","tool":"js","arguments":{"code":"const app = await cua.getApp('sushiAI'); await app.getAXStateAndScreenshot()","title":"Inspect the sushiAI panel"},"result":{"content":[{"type":"text","text":"Computer Use was not approved to use sushiAI"}],"_meta":{"codex/nodeReplExecutionDurationMs":9,"codex/toolSurface":{"app":{"appId":"local.sushiai.workspace","kind":"appId"},"kind":"computerUse"}},"structured_content":null},"error":null,"status":"failed"}}"##;
    // Copies of the pair above where only server/tool/arguments were swapped
    // for an orchd call: no real Codex call to sushiai-orchestrator exists.
    const CODEX_ORCHD_STARTED: &str = r##"{"type":"item.started","item":{"id":"item_10","type":"mcp_tool_call","server":"sushiai-orchestrator","tool":"task_get","arguments":{"id":"eb6845cb-12d8-44a5-8e37-ad577410ae22"},"result":null,"error":null,"status":"in_progress"}}"##;
    const CODEX_ORCHD_COMPLETED: &str = r##"{"type":"item.completed","item":{"id":"item_10","type":"mcp_tool_call","server":"sushiai-orchestrator","tool":"task_get","arguments":{"id":"eb6845cb-12d8-44a5-8e37-ad577410ae22"},"result":{"content":[{"type":"text","text":"Computer Use was not approved to use sushiAI"}],"_meta":{"codex/nodeReplExecutionDurationMs":9,"codex/toolSurface":{"app":{"appId":"local.sushiai.workspace","kind":"appId"},"kind":"computerUse"}},"structured_content":null},"error":null,"status":"failed"}}"##;
    const TASK_ID: &str = "eb6845cb-12d8-44a5-8e37-ad577410ae22";

    fn stored_task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            goal: "g".into(),
            criteria: vec![],
            verify: vec![],
            final_verify: vec![],
            checks: vec![],
            held_out: None,
            request: None,
            repo: "/repo".into(),
            worktree: "/wt".into(),
            worktree_removed: false,
            deliverables: vec![],
            visual_criteria: vec![],
            landed_sha: None,
            landed_at: None,
            diff_stat: None,
            report: None,
            report_at: None,
            lead_touch: None,
            follow_up_of: None,
            follow_ups: vec![],
            branch: "b".into(),
            base_sha: "s".into(),
            base_ref: None,
            depends_on: vec![],
            paths: vec![],
            parent: None,
            status: TaskStatus::Running,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            budget_raises: 0,
            daily_budget_ok_day: None,
            assumptions: vec![],
            question_history: vec![],
            judged_findings: vec![],
            archived: false,
            pr_url: None,
            planned_tier: None,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            source: None,
            eval_check: None,
            brief_check: Default::default(),
            queue: Default::default(),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn tools_of(app: &App, repo: &str, harness: Harness, lines: &[&str]) -> Vec<ChatTool> {
        let mut seen = std::collections::HashSet::new();
        let mut list = Vec::new();
        for line in lines {
            keep_tools(
                &mut list,
                tools_in_line(app, repo, harness, line, &mut seen),
            );
        }
        list
    }

    fn tool(name: &str, summary: &str) -> ChatTool {
        ChatTool {
            name: name.into(),
            summary: summary.into(),
        }
    }

    #[test]
    fn a_real_claude_task_get_line_becomes_the_tasks_title_and_never_its_id() {
        let (app, _dir, repo) = test_app();
        app.store
            .save_task(&stored_task(TASK_ID, "Keep the tool calls"))
            .unwrap();
        let tools = tools_of(&app, &repo, Harness::Claude, &[CLAUDE_TASK_GET]);
        assert_eq!(tools, vec![tool("task_get", "Keep the tool calls")]);

        let mut session = load(&app, &repo).current().clone();
        push(&mut session, "user", "status?", ChatMode::Chat);
        apply_reply(
            &mut session,
            "It is running.",
            tools,
            ChatMode::Chat,
            &|_, _| false,
        );
        let message = serde_json::to_value(session.messages.last().unwrap()).unwrap();
        assert_eq!(
            message["tools"],
            json!([{"name": "task_get", "summary": "Keep the tool calls"}])
        );
        assert!(!message.to_string().contains(TASK_ID));
    }

    #[test]
    fn an_unknown_task_id_gets_an_empty_summary_and_task_create_its_title() {
        let (app, _dir, repo) = test_app();
        let tools = tools_of(&app, &repo, Harness::Claude, &[CLAUDE_TASK_GET]);
        assert_eq!(tools, vec![tool("task_get", "")]);
        let create =
            json!({"type": "assistant", "parent_tool_use_id": null, "message": {"content": [
                {"type": "tool_use", "name": "mcp__sushiai-orchestrator__task_create",
                 "input": {"title": "Add a button", "goal": "g"}}
            ]}})
            .to_string();
        assert_eq!(
            tools_of(&app, &repo, Harness::Claude, &[&create]),
            vec![tool("task_create", "Add a button")]
        );
    }

    #[test]
    fn built_in_tools_keep_their_names_and_show_repo_relative_paths_or_patterns() {
        let (app, _dir, repo) = test_app();
        let line = json!({"type": "assistant", "parent_tool_use_id": null, "message": {"content": [
            {"type": "tool_use", "name": "Read", "input": {"file_path": format!("{repo}/src/a.rs")}},
            {"type": "tool_use", "name": "Grep", "input": {"pattern": "fn main"}},
            {"type": "tool_use", "name": "Glob", "input": {"pattern": "**/*.rs"}},
            {"type": "tool_use", "name": "mcp__sushiai-orchestrator__task_list", "input": {}}
        ]}})
        .to_string();
        assert_eq!(
            tools_of(&app, &repo, Harness::Claude, &[&line]),
            vec![
                tool("Read", "src/a.rs"),
                tool("Grep", "fn main"),
                tool("Glob", "**/*.rs"),
                tool("task_list", ""),
            ]
        );
    }

    #[test]
    fn a_subagent_call_is_not_recorded() {
        let (app, _dir, repo) = test_app();
        assert!(CLAUDE_SUBAGENT_CALL.contains("\"parent_tool_use_id\":\"toolu"));
        assert!(tools_of(&app, &repo, Harness::Claude, &[CLAUDE_SUBAGENT_CALL]).is_empty());
    }

    #[test]
    fn a_codex_mcp_call_is_recorded_once_per_item_id() {
        let (app, _dir, repo) = test_app();
        app.store
            .save_task(&stored_task(TASK_ID, "Keep the tool calls"))
            .unwrap();
        assert_eq!(
            tools_of(
                &app,
                &repo,
                Harness::Codex,
                &[CODEX_CUA_STARTED, CODEX_CUA_COMPLETED]
            ),
            vec![tool("js", "")]
        );
        assert_eq!(
            tools_of(
                &app,
                &repo,
                Harness::Codex,
                &[CODEX_ORCHD_STARTED, CODEX_ORCHD_COMPLETED]
            ),
            vec![tool("task_get", "Keep the tool calls")]
        );
    }

    #[test]
    fn a_turn_keeps_the_first_eight_calls_and_collapses_repeats() {
        let mut list = Vec::new();
        keep_tools(
            &mut list,
            vec![tool("a", ""), tool("a", ""), tool("b", ""), tool("a", "")],
        );
        assert_eq!(list, vec![tool("a", ""), tool("b", ""), tool("a", "")]);
        keep_tools(
            &mut list,
            (0..12).map(|i| tool("t", &i.to_string())).collect(),
        );
        assert_eq!(list.len(), 8);
        assert_eq!(list[3], tool("t", "0"));
        assert_eq!(list[7], tool("t", "4"));
    }

    #[test]
    fn a_long_summary_is_cut_to_120_chars_with_an_ellipsis() {
        let (app, _dir, repo) = test_app();
        let line = json!({"type": "assistant", "parent_tool_use_id": null, "message": {"content": [
            {"type": "tool_use", "name": "mcp__sushiai-orchestrator__task_create",
             "input": {"title": "x".repeat(300)}}
        ]}})
        .to_string();
        let tools = tools_of(&app, &repo, Harness::Claude, &[&line]);
        assert_eq!(tools[0].summary.chars().count(), 120);
        assert!(tools[0].summary.ends_with('…'));
    }

    #[test]
    fn messages_without_tools_serialize_and_load_as_before() {
        let old = r#"{"id":"m1","role":"assistant","text":"hi","ts":5}"#;
        let message: ChatMessage = serde_json::from_str(old).unwrap();
        assert!(message.tools.is_empty());
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            serde_json::from_str::<serde_json::Value>(old).unwrap()
        );

        let mut session = ChatSession::default();
        apply_reply(
            &mut session,
            "plain",
            Vec::new(),
            ChatMode::Chat,
            &|_, _| false,
        );
        let value = serde_json::to_value(session.messages.last().unwrap()).unwrap();
        assert!(value.get("tools").is_none());
    }

    #[test]
    fn both_prompts_say_to_name_tasks_by_title_and_the_draft_still_takes_ids() {
        for prompt in [
            crate::prompts::default("chat"),
            crate::prompts::default("brainstorm"),
            crate::prompts::default("plan"),
        ] {
            assert!(prompt.contains("refer to a task by its title, never by its id"));
            assert!(prompt.contains("'8ec68f8c'"));
        }
        assert!(crate::prompts::default("chat").contains("dependsOn"));
        assert!(crate::prompts::default("chat").contains("only the owner can decide"));
        assert!(crate::prompts::default("plan").contains("language of the owner's latest message"));
        assert!(crate::prompts::default("chat").contains("\"dependsOn\": [\"task ids\"]"));
    }

    #[test]
    fn brainstorm_and_plan_turns_carry_their_prompt_and_only_read_only_tools() {
        for (mode, prompt) in [
            (ChatMode::Brainstorm, crate::prompts::default("brainstorm")),
            (ChatMode::Plan, crate::prompts::default("plan")),
        ] {
            assert_ne!(prompt, crate::prompts::default("chat"));
            for session in [None, Some("sess")] {
                let claude = claude_argv(
                    &route(Harness::Claude),
                    Path::new("/d/mcp.json"),
                    Path::new("/d/settings.json"),
                    session,
                    mode,
                    &chat_tools::TurnTools::default(),
                );
                let at =
                    |flag: &str| claude[claude.iter().position(|a| a == flag).unwrap() + 1].clone();
                assert_eq!(at("--append-system-prompt"), prompt);
                let expected: Vec<String> = crate::mcp::READ_ONLY_TOOLS
                    .iter()
                    .map(|t| format!("mcp__{SERVER}__{t}"))
                    .collect();
                assert_eq!(
                    at("--allowedTools").split(',').collect::<Vec<_>>(),
                    expected
                );
                assert_eq!(claude.contains(&"--resume".to_string()), session.is_some());

                let server = json!({"command": "/o", "args": ["mcp", "--read-only"]});
                let codex = codex_argv(
                    &route(Harness::Codex),
                    "/repo",
                    &server,
                    session,
                    mode,
                    &chat_tools::TurnTools::default(),
                );
                assert!(codex.contains(&format!("developer_instructions={}", json!(prompt))));
                assert!(!codex.contains(&format!(
                    "developer_instructions={}",
                    json!(crate::prompts::default("chat"))
                )));
            }
        }
    }

    #[test]
    fn only_the_chat_mode_gets_a_writing_bridge() {
        let (app, _dir, _repo) = test_app();
        let has_read_only = |mode| {
            mcp_server(&app, None, mode)["args"]
                .as_array()
                .unwrap()
                .contains(&json!("--read-only"))
        };
        assert!(!has_read_only(ChatMode::Chat));
        assert!(has_read_only(ChatMode::Brainstorm));
        assert!(has_read_only(ChatMode::Plan));
    }

    #[test]
    fn the_mode_prompts_say_what_they_are_for() {
        for needle in [
            "exactly one focused question",
            "only the owner can decide",
            "language of the owner's latest message",
            "2-4 answer options",
            "weigh 2-3 approaches",
            "never create, start, stop, amend or answer tasks",
            "\"tasks\"",
            "dependsOn",
        ] {
            assert!(
                crate::prompts::default("brainstorm").contains(needle),
                "{needle}"
            );
        }
        for needle in [
            "small, ordered tasks",
            "how each is checked",
            "tier",
            "never create, start, stop, amend or answer tasks",
            "\"tasks\"",
        ] {
            assert!(crate::prompts::default("plan").contains(needle), "{needle}");
        }
    }

    #[test]
    fn a_task_list_reads_index_and_id_dependencies_and_a_bad_list_is_dropped() {
        let ok = parse_reply(&block(
            r#"{"tasks":[{"title":"A"},{"title":"B","dependsOn":[1,"abc-def","1"," 1 "],"tier":"hard"}]}"#,
        ));
        let proposal = ok.proposal.unwrap();
        assert_eq!(ok.text, "2 proposed tasks");
        assert_eq!(
            proposal.tasks[1].depends_on,
            vec![ProposalDep::Index(1), ProposalDep::Id("abc-def".into())]
        );
        assert_eq!(proposal.tasks[1].tier, Some(Tier::Hard));
        for bad in [
            r#"{"tasks":[]}"#,
            r#"{"tasks":"soon"}"#,
            r#"{"tasks":[{"goal":"no title"}]}"#,
            r#"{"tasks":[{"title":"A","dependsOn":[2]}]}"#,
            r#"{"tasks":[{"title":"A","dependsOn":[1]}]}"#,
            r#"{"tasks":[{"title":"A","dependsOn":[2]},{"title":"B","dependsOn":[1]}]}"#,
        ] {
            let parsed = parse_reply(&format!("Words.\n{}", block(bad)));
            assert!(parsed.proposal.is_none(), "{bad}");
            assert!(parsed.text.starts_with("Words."), "{bad}");
        }
        // A bad list beside a question keeps the question and the prose.
        let mixed = parse_reply(&format!(
            "Words.\n{}",
            block(r#"{"tasks":"x","questions":[{"text":"Q?","options":["a","b"]}]}"#)
        ));
        assert!(mixed.proposal.is_none());
        assert_eq!(mixed.text, "Words.");
        assert_eq!(mixed.questions.len(), 1);
    }

    #[test]
    fn proposals_order_dependencies_first() {
        let proposal = ChatProposal::parse(
            &json!([
                {"title": "A", "dependsOn": [2]},
                {"title": "B", "dependsOn": [3]},
                {"title": "C"},
            ]),
            None,
        )
        .unwrap();
        assert_eq!(proposal.order(&[0, 1, 2]), Some(vec![2, 1, 0]));
        assert_eq!(proposal.order(&[0, 1]), Some(vec![1, 0]));
    }

    #[test]
    fn a_store_from_before_modes_loads_its_brainstorm_sessions_as_chat_sessions() {
        let old = json!({
            "repo": "/r",
            "current": "a",
            "brainstormCurrent": "b",
            "sessions": [
                {"id": "a", "kind": "chat", "messages": []},
                {"id": "b", "kind": "brainstorm", "draft": {"title": "T"}, "messages": [
                    {"id": "m", "role": "user", "text": "idea", "ts": 5},
                    {"id": "n", "role": "assistant", "text": "which?", "ts": 6,
                     "draft": {"title": "T"}, "questions": [{"text": "Q?", "options": ["x"]}]},
                ]},
            ],
        })
        .to_string();
        let (chats, _) = parse_store("/r", Some(&old));
        assert_eq!(chats.current, "a");
        assert_eq!(chats.sessions.len(), 2);
        let brainstorm = &chats.sessions[1];
        assert_eq!(brainstorm.messages.len(), 2);
        assert_eq!(brainstorm.messages[1].questions.len(), 1);
        assert_eq!(brainstorm.mode, ChatMode::Chat);
        let thread = chats.thread_json();
        assert!(thread.get("kind").is_none());
    }
}
