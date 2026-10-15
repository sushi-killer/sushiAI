use super::actions::take_action;
use super::proposals::ChatProposal;
use super::sessions::{push, ChatSession};
use super::types::{ChatDraft, ChatMode, ChatQuestion, ChatTool};
use serde::Deserialize;

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
    let Some(tagged) = crate::prompts::reply::last_tagged_block(reply, "sushi-draft") else {
        return verbatim();
    };
    let Ok(block) = serde_json::from_str::<DraftBlock>(tagged.body) else {
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
    let mut text = tagged.remove_from(reply);
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

/// Adds the agent's reply, in the mode of the turn, to the session.
pub(super) fn apply_reply(
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

#[cfg(test)]
#[path = "tests/reply.rs"]
mod tests;
