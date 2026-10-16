use super::actions::ActionState;
use super::types::{ChatMessage, ChatMode};
use crate::clip::truncate_chars;
use crate::clock::now_ms;
use crate::harness::cancel::CancelToken;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;

const MAX_MESSAGES: usize = 200;

pub(super) const MAX_TEXT: usize = 20_000;

/// Same length a task title gets when the planner is off (`createTask` in
/// OrchestratorPanel.tsx).
const MAX_TITLE: usize = 60;

/// One conversation with the orchestrator agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChatSession {
    pub(super) id: String,
    /// When the session was created, in ms.
    #[serde(default)]
    pub(super) created_at: i64,
    /// Set once from the session's first owner message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) messages: Vec<ChatMessage>,
    /// True only for the current session while a turn is live in this
    /// daemon; a session saved as busy by a daemon that died reads back idle.
    #[serde(default)]
    pub(super) busy: bool,
    /// The last progress line of a live turn (a tool call, "session started").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) route_id: Option<String>,
    /// The project's MCP launch config the app resolved; tasks this agent
    /// creates for the same repo get it (`ORCHD_TASK_MCP`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) task_mcp: Option<serde_json::Value>,
    /// The mode of the last message the owner sent; the composer restores it.
    #[serde(default, skip_serializing_if = "ChatMode::is_chat")]
    pub(super) mode: ChatMode,
}

impl ChatSession {
    pub(super) fn new() -> Self {
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
pub(super) struct ChatStore {
    pub(super) repo: String,
    pub(super) current: String,
    pub(super) sessions: Vec<ChatSession>,
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
pub(super) struct SessionSummary {
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

    pub(super) fn current(&self) -> &ChatSession {
        self.sessions
            .iter()
            .find(|s| s.id == self.current)
            .expect("parse_store keeps the pointer at a session")
    }

    pub(super) fn current_mut(&mut self) -> &mut ChatSession {
        let id = self.current.clone();
        self.sessions
            .iter_mut()
            .find(|s| s.id == id)
            .expect("parse_store keeps the pointer at a session")
    }

    /// Marks the current session busy while a turn is live; a note only means
    /// something on a live turn.
    pub(super) fn mark_busy(&mut self, repo: &str, turns: &HashMap<String, CancelToken>) {
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

    pub(super) fn summaries(&self) -> Vec<SessionSummary> {
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
    pub(super) fn thread_json(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self.current()).unwrap_or_default();
        value["repo"] = json!(self.repo);
        value
    }

    pub(super) fn list_json(&self) -> serde_json::Value {
        json!({"current": self.current, "sessions": self.summaries()})
    }
}

pub(super) fn title_for(text: &str) -> String {
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
pub(super) fn parse_store(repo: &str, text: Option<&str>) -> (ChatStore, bool) {
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

pub(super) fn push(session: &mut ChatSession, role: &str, text: &str, mode: ChatMode) {
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

#[cfg(test)]
#[path = "tests/sessions.rs"]
mod tests;
