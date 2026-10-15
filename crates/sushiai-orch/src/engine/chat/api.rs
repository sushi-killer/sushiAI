use super::actions::take_action_notes;
use super::prompts::{history_prompt, with_questions};
use super::sessions::{push, title_for, ChatSession, MAX_TEXT};
use super::store::{begin_turn, change_idle, load, release_turn, save};
use super::turn::ScheduledTurn;
use super::turn::Turn;
use super::types::ChatMode;
use crate::clock::now_ms;
use crate::engine::messages;
use crate::engine::state::App;
use crate::prompts::clip::truncate_chars;
use serde_json::json;
use std::path::Path;

pub(in crate::engine) fn repo_param(params: &serde_json::Value) -> Result<String, String> {
    let repo = params
        .get("repo")
        .and_then(|v| v.as_str())
        .ok_or("repo is required")?;
    if !Path::new(repo).is_absolute() || !Path::new(repo).is_dir() {
        return Err("repo must be an existing absolute directory".to_string());
    }
    Ok(repo.to_string())
}

pub(in crate::engine) async fn handle_get(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    Ok(load(app, &repo).thread_json())
}

pub(in crate::engine) async fn handle_list(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    Ok(load(app, &repo).list_json())
}

/// Starts an empty session and makes it the current one.
pub(in crate::engine) async fn handle_new(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    let chats = change_idle(app, &repo, |chats| {
        let session = ChatSession::new();
        chats.current = session.id.clone();
        chats.sessions.push(session);
        Ok(())
    })?;
    Ok(chats.thread_json())
}

pub(in crate::engine) async fn handle_switch(
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
pub(in crate::engine) async fn handle_clear(
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

pub(in crate::engine) async fn handle_cancel(
    app: &App,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let repo = repo_param(&params)?;
    if let Some(cancel) = app.chat_turns.lock().unwrap().get(&repo) {
        cancel.cancel();
    }
    Ok(json!({}))
}

pub(in crate::engine) async fn handle_send(
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
    let prompt = take_question_prompt(app, &repo, text, mode);
    let prompt = format!("{notes}{prompt}");
    ScheduledTurn {
        repo,
        thread: session,
        route,
        text: prompt,
        cancel,
        turn: Turn::Owner(mode),
    }
    .start(app);
    Ok(json!({}))
}

/// Replaces one of the owner's messages, drops everything after it (with its
/// proposals, pending actions, draft and questions) and runs a new turn from
/// there on a fresh harness session that is given the kept history. Tasks
/// already created from a dropped proposal stay. Refused while a turn runs.
pub(in crate::engine) async fn handle_edit(
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
        release_turn(app, &repo);
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
    let prompt = take_question_prompt(app, &repo, text, mode);
    let prompt = format!("{notes}{history}{prompt}");
    ScheduledTurn {
        repo,
        thread: session,
        route,
        text: prompt,
        cancel,
        turn: Turn::Owner(mode),
    }
    .start(app);
    Ok(json!({}))
}

fn take_question_prompt(app: &App, repo: &str, text: String, mode: ChatMode) -> String {
    let waiting = match mode {
        ChatMode::Chat => messages::take_for_orchestrator(app, repo),
        _ => None,
    };
    match waiting {
        Some((_, questions)) => with_questions(&text, &questions),
        None => text,
    }
}

pub(super) fn message_id_param(params: &serde_json::Value) -> Result<String, String> {
    params
        .get("messageId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "messageId is required".to_string())
}

#[cfg(test)]
#[path = "tests/api.rs"]
mod tests;
