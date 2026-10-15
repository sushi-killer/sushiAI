use super::{from_schema, ToolSpec};
use serde_json::json;

pub(super) fn specs() -> Vec<ToolSpec> {
    vec![
        (
    "peer_list",
    "peer.list",
    "List the other tasks in a repository (id, title, status) you can message.",
    json!({
        "type": "object",
        "properties": {
            "from": from_schema(),
            "repo": {"type": "string", "description": "Absolute path; needed only when sending as the orchestrator."},
        },
    }),
),
        (
    "peer_send",
    "message.send",
    "Message another task by id. It reads the message on its next attempt; the attempt it is running is not interrupted.",
    json!({
        "type": "object",
        "properties": {
            "from": from_schema(),
            "to": {"type": "string", "description": "The recipient task's id."},
            "text": {"type": "string"},
            "replyTo": {"type": "string", "description": "Id of a message sent to you that this answers."},
        },
        "required": ["to", "text"],
    }),
),
        (
    "inbox_read",
    "message.inbox",
    "Read every message sent to you, delivered or still waiting for your next turn.",
    json!({"type": "object", "properties": {"from": from_schema()}}),
),
        (
    "ask_orchestrator",
    "message.send",
    "Ask the orchestrator a question without stopping your work. Its reply arrives with your next attempt.",
    json!({
        "type": "object",
        "properties": {"from": from_schema(), "text": {"type": "string"}},
        "required": ["text"],
    }),
),
        (
    "orchestrator_reply",
    "message.send",
    "Answer a question a task asked you; the task reads it on its next attempt.",
    json!({
        "type": "object",
        "properties": {
            "question": {"type": "string", "description": "The question's message id."},
            "text": {"type": "string"},
        },
        "required": ["question", "text"],
    }),
),
    ]
}
