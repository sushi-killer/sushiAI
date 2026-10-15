use super::api::{message_id_param, repo_param};
use super::sessions::{push, ChatSession};
use super::store::{change_idle, finish_turn, read_store, save, BUSY};
use super::tools::one_line;
use super::types::ChatMode;
use crate::engine::chat_tools;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::prompts::clip::truncate_chars;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum ActionState {
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
pub(super) struct ChatAction {
    pub(super) id: String,
    /// The connected tool's id.
    pub(super) server: String,
    pub(super) tool: String,
    pub(super) args: serde_json::Value,
    #[serde(default)]
    pub(super) summary: String,
    #[serde(default)]
    pub(super) target: String,
    pub(super) state: ActionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) result: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) noted: bool,
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
pub(super) fn take_action(
    reply: &str,
    proposable: &dyn Fn(&str, &str) -> bool,
) -> (String, Option<ChatAction>) {
    let untouched = || (reply.to_string(), None);
    let Some(tagged) = crate::prompts::reply::last_tagged_block(reply, "sushi-action") else {
        return untouched();
    };
    let Ok(block) = serde_json::from_str::<ActionBlock>(tagged.body) else {
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
    let mut text = tagged.remove_from(reply);
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

/// What became of the write calls the orchestrator proposed, for its next
/// turn: it cannot see the owner's OK card. Each call is reported once.
pub(super) fn take_action_notes(session: &mut ChatSession) -> String {
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
pub(in crate::engine) async fn handle_action_send(
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
            Some(cfg) => chat_tools::approved::execute(&app, cfg, &action.tool, &action.args).await,
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
pub(in crate::engine) async fn handle_action_decline(
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
#[path = "tests/actions.rs"]
mod tests;
