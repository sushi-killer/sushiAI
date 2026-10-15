use super::actions::ChatAction;
use super::proposals::ChatProposal;
use crate::policy::routing::Tier;
use serde::Deserialize;
use serde::Serialize;

/// How one turn behaves: which prompt and which tools it gets. The owner picks
/// it per message inside one conversation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ChatMode {
    #[default]
    Chat,
    Brainstorm,
    Plan,
}

impl ChatMode {
    /// The optional `mode` parameter: absent means the plain chat.
    pub(super) fn param(params: &serde_json::Value) -> Result<Self, String> {
        match params.get("mode") {
            None | Some(serde_json::Value::Null) => Ok(ChatMode::Chat),
            Some(v) => serde_json::from_value(v.clone())
                .map_err(|_| "mode must be \"chat\", \"brainstorm\" or \"plan\"".to_string()),
        }
    }

    pub(super) fn is_chat(&self) -> bool {
        *self == ChatMode::Chat
    }

    /// This mode's prompt (`prompts/orchestrator.yaml`, overridable).
    pub(super) fn prompt(self) -> String {
        crate::prompts::get(match self {
            ChatMode::Chat => "chat",
            ChatMode::Brainstorm => "brainstorm",
            ChatMode::Plan => "plan",
        })
    }

    /// Brainstorm and Plan only look: they hand the owner a proposal instead
    /// of creating tasks.
    pub(super) fn read_only(self) -> bool {
        !self.is_chat()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChatMessage {
    pub(super) id: String,
    pub(super) role: String,
    pub(super) text: String,
    pub(super) ts: i64,
    /// The mode the turn ran in (on the owner's message and on the reply to
    /// it); absent means the plain chat.
    #[serde(default, skip_serializing_if = "ChatMode::is_chat")]
    pub(super) mode: ChatMode,
    /// The task the orchestrator proposed in this reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) draft: Option<ChatDraft>,
    /// The list of tasks the orchestrator proposed in this reply, and what the
    /// owner did with each row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) proposal: Option<ChatProposal>,
    /// Choices the orchestrator asked the owner to make in this reply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) questions: Vec<ChatQuestion>,
    /// The tool calls the orchestrator made while writing this reply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) tools: Vec<ChatTool>,
    /// The write call the orchestrator proposed in this reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) action: Option<ChatAction>,
}

/// One tool call of a reply: the tool's name without the orchd server prefix
/// and a one-line detail (empty when there is none worth showing).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(super) struct ChatTool {
    pub(super) name: String,
    pub(super) summary: String,
}

/// A task the orchestrator proposed for the owner to confirm, read from a
/// `sushi-draft` block.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct ChatDraft {
    #[serde(default)]
    pub(super) title: String,
    #[serde(default)]
    pub(super) goal: String,
    #[serde(
        default,
        deserialize_with = "crate::engine::planning::serde::lenient_criteria"
    )]
    pub(super) criteria: Vec<String>,
    #[serde(default)]
    pub(super) depends_on: Vec<String>,
    #[serde(
        default,
        deserialize_with = "crate::engine::planning::serde::lenient_tier"
    )]
    pub(super) tier: Option<Tier>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(super) struct ChatQuestion {
    pub(super) text: String,
    #[serde(default)]
    pub(super) options: Vec<String>,
}
