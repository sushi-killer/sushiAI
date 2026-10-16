use super::sessions::{parse_store, push, ChatSession, ChatStore};
use super::types::ChatMode;
use super::wake::fire_deferred_wakes;
use crate::api::events::Event;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::policy::keys::simple_hash;
use crate::policy::routing::Route;
use crate::store;
use std::path::PathBuf;

pub(super) const BUSY: &str = "the orchestrator is still answering";

/// Where a repo's turns keep their MCP config, settings and event log.
pub(super) fn turn_dir(app: &App, repo: &str) -> PathBuf {
    app.data_dir.join("chats").join(simple_hash(repo))
}

pub(super) fn store_path(app: &App, repo: &str) -> PathBuf {
    app.data_dir
        .join("chats")
        .join(format!("{}.json", simple_hash(repo)))
}

/// Reads the repo's store, writing it straight back when it had to be built
/// or migrated so every caller sees the same session ids. Callers hold
/// `chat_turns`, which serialises all store reads and writes.
pub(super) fn read_store(app: &App, repo: &str) -> ChatStore {
    let path = store_path(app, repo);
    let text = std::fs::read_to_string(&path).ok();
    let (chats, changed) = parse_store(repo, text.as_deref());
    if changed {
        let _ = store::files::write_json_atomic(&path, &chats);
    }
    chats
}

pub(super) fn load(app: &App, repo: &str) -> ChatStore {
    let turns = app.chat_turns.lock().unwrap();
    let mut chats = read_store(app, repo);
    chats.mark_busy(repo, &turns);
    chats
}

pub(super) fn broadcast(app: &App, chats: &ChatStore) {
    let _ = app.events_tx.send(Event::Chat {
        thread: Box::new(chats.thread_json()),
        current: chats.current.clone(),
        sessions: Box::new(serde_json::to_value(chats.summaries()).unwrap_or_default()),
    });
}

/// Writes a turn's session back into its repo's store and broadcasts it.
pub(super) fn save(app: &App, repo: &str, session: &ChatSession) {
    write_session(app, repo, session, false);
}

/// Saves a turn's final session and frees the repo's turn slot under one
/// lock, so no new/switch/clear can slip in between and be overwritten by
/// the reply (or have cleared messages restored by it).
pub(super) fn finish_turn(app: &App, repo: &str, session: &ChatSession) {
    write_session(app, repo, session, true);
    fire_deferred_wakes(app, repo);
}

/// Frees the repo's turn slot without a session write, then runs the wakes
/// that were waiting for it.
pub(super) fn release_turn(app: &App, repo: &str) {
    app.chat_turns.lock().unwrap().remove(repo);
    fire_deferred_wakes(app, repo);
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
        let _ = store::files::write_json_atomic(&store_path(app, repo), &chats);
        chats
    };
    broadcast(app, &chats);
}

/// Applies `change` to the repo's store, saves and broadcasts it - refused
/// while a turn is live, since that turn belongs to the current session and
/// moving its pointer under it would orphan the reply. Holding `chat_turns`
/// throughout keeps a turn from starting halfway through.
pub(super) fn change_idle(
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
        store::files::write_json_atomic(&store_path(app, repo), &chats)
            .map_err(|e| e.to_string())?;
        chats
    };
    broadcast(app, &chats);
    Ok(chats)
}

/// Adds an assistant note to the repo's current chat session; false while a
/// turn is live (the caller retries later).
pub(in crate::engine) fn post_note(app: &App, repo: &str, text: &str) -> bool {
    change_idle(app, repo, |chats| {
        push(chats.current_mut(), "assistant", text, ChatMode::Chat);
        Ok(())
    })
    .is_ok()
}

/// Claims the repo's turn slot and loads the current session, starting a fresh harness session when the orchestrator's route
/// changed since the session's last turn.
pub(super) fn begin_turn(
    app: &App,
    repo: &str,
) -> Result<(ChatSession, Route, CancelToken), String> {
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
