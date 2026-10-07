//! Parse the JSON a Claude Code or Codex hook receives on stdin.
//!
//! Both CLIs share the common fields. Unknown fields are ignored, an unknown
//! event name becomes `HookEvent::Unknown`, and nothing here panics. Input
//! that is not a JSON object is the only error.

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("hook payload is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("hook payload is not a JSON object")]
    NotAnObject,
}

/// Fields every event carries (all optional: a missing one is not an error).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Common {
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub permission_mode: Option<String>,
    /// Present only inside a subagent (Claude).
    pub agent_id: Option<String>,
    pub turn_id: Option<String>,
    /// Claude's per-prompt id (Codex uses `turn_id`).
    pub prompt_id: Option<String>,
    /// The tool input of a tool event, when present.
    pub tool_input: Option<Value>,
}

impl Common {
    /// The turn this event belongs to: `turn_id` (Codex) or `prompt_id` (Claude).
    pub fn turn(&self) -> Option<&str> {
        self.turn_id.as_deref().or(self.prompt_id.as_deref())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PermissionRequest {
    pub tool_name: Option<String>,
    pub tool_input: Value,
    /// Not read by the daemon yet; kept because both CLIs may send it.
    pub tool_use_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HookEvent {
    SessionStart {
        source: Option<String>,
    },
    UserPromptSubmit,
    PreToolUse {
        tool_name: Option<String>,
    },
    PostToolUse {
        tool_name: Option<String>,
    },
    PostToolUseFailure {
        tool_name: Option<String>,
    },
    PermissionRequest(PermissionRequest),
    Notification {
        notification_type: Option<String>,
        message: Option<String>,
    },
    Stop,
    StopFailure {
        error: Option<String>,
    },
    SubagentStart,
    SubagentStop,
    Interrupt,
    SessionEnd {
        reason: Option<String>,
    },
    /// An event this crate does not model. Holds the raw name (may be empty).
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookPayload {
    pub common: Common,
    pub event: HookEvent,
}

fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Every payload field `parse` reads. A test checks each one against the
/// captured fixtures.
pub const READ_FIELDS: &[&str] = &[
    "session_id",
    "transcript_path",
    "cwd",
    "permission_mode",
    "agent_id",
    "turn_id",
    "prompt_id",
    "hook_event_name",
    "tool_name",
    "tool_input",
    "tool_use_id",
    "source",
    "notification_type",
    "message",
    "error",
    "reason",
];

pub fn parse(bytes: &[u8]) -> Result<HookPayload, ParseError> {
    let v: Value = serde_json::from_slice(bytes)?;
    if !v.is_object() {
        return Err(ParseError::NotAnObject);
    }
    let common = Common {
        session_id: string(&v, "session_id"),
        transcript_path: string(&v, "transcript_path"),
        cwd: string(&v, "cwd"),
        permission_mode: string(&v, "permission_mode"),
        agent_id: string(&v, "agent_id"),
        turn_id: string(&v, "turn_id"),
        prompt_id: string(&v, "prompt_id"),
        tool_input: v.get("tool_input").cloned(),
    };
    let name = string(&v, "hook_event_name").unwrap_or_default();
    let tool = || string(&v, "tool_name");
    let event = match name.as_str() {
        "SessionStart" => HookEvent::SessionStart {
            source: string(&v, "source"),
        },
        "UserPromptSubmit" => HookEvent::UserPromptSubmit,
        "PreToolUse" => HookEvent::PreToolUse { tool_name: tool() },
        "PostToolUse" => HookEvent::PostToolUse { tool_name: tool() },
        "PostToolUseFailure" => HookEvent::PostToolUseFailure { tool_name: tool() },
        "PermissionRequest" => HookEvent::PermissionRequest(PermissionRequest {
            tool_name: tool(),
            tool_input: v.get("tool_input").cloned().unwrap_or(Value::Null),
            tool_use_id: string(&v, "tool_use_id"),
        }),
        "Notification" => HookEvent::Notification {
            notification_type: string(&v, "notification_type"),
            message: string(&v, "message"),
        },
        "Stop" => HookEvent::Stop,
        "StopFailure" => HookEvent::StopFailure {
            error: string(&v, "error"),
        },
        "SubagentStart" => HookEvent::SubagentStart,
        "SubagentStop" => HookEvent::SubagentStop,
        "Interrupt" => HookEvent::Interrupt,
        "SessionEnd" => HookEvent::SessionEnd {
            reason: string(&v, "reason"),
        },
        _ => HookEvent::Unknown(name),
    };
    Ok(HookPayload { common, event })
}
