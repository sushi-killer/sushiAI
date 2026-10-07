//! `session.wake`: rebuild the launch of a hibernated (or exited) agent session from its sealed
//! store and hand it to the session's actor, which starts the process.

use std::sync::Arc;

use sushiai_core::{hibernate::can_hibernate, mark_hibernated};
use sushiai_protocol::{code, SessionCreate, SessionStatus};

use crate::agent::{self, now_ms};
use crate::error::Fail;
use crate::registry::Registry;
use crate::session;

/// Wakes the session `id`; a running or waking one is left as it is.
pub async fn wake(registry: &Arc<Registry>, id: &str) -> Result<(), Fail> {
    // One wake at a time: two must not both start an actor for a record that has none.
    let _one_at_a_time = registry.wake_lock.lock().await;
    let not_found = || (code::SESSION_NOT_FOUND, format!("no session {id}"));
    let info = registry.stored(id).ok_or_else(not_found)?;
    match info.status {
        SessionStatus::Hibernated | SessionStatus::Exited => {}
        SessionStatus::Running => {
            // An agent that is ending for a sleep wakes as soon as it is asleep.
            if let Some(handle) = registry.handle(id) {
                return handle.wake_asked().await;
            }
            return Ok(());
        }
        _ => return Ok(()),
    }
    let (Some(name), Some(conversation)) = (
        info.agent
            .name
            .clone()
            .filter(|n| agent::agent_of(Some(n)).is_some()),
        info.agent.agent_session.clone(),
    ) else {
        return Err((
            code::INVALID_PARAMS,
            "only a claude or codex session with a conversation can wake".into(),
        ));
    };
    let needs_launch = || {
        (
            code::WAKE_NEEDS_LAUNCH,
            "the session has no stored launch".to_string(),
        )
    };
    let (store, key) = (registry.clone(), id.to_string());
    let launch = tokio::task::spawn_blocking(move || store.launches().open(&key))
        .await
        .map_err(|e| (code::INTERNAL, e.to_string()))?
        .map_err(|_| needs_launch())?;
    let request = SessionCreate {
        cmd: Vec::new(),
        cwd: info.cwd.clone(),
        cols: info.cols,
        rows: info.rows,
        title: None,
        agent: Some(name),
        resume: Some(conversation),
        prompt: None,
        model: launch.model,
        extra_args: launch.extra_args,
        env: launch.env,
        project: None,
        group: None,
        claude_settings: launch.claude_settings,
        idempotency_key: None,
    };
    let socket = registry.home.socket().to_string_lossy().into_owned();
    let prepared = agent::prepare(&request, &socket, id)?;
    let home_dir = registry.home.dir().to_path_buf();
    tokio::task::spawn_blocking(move || agent::ensure_codex_home(&request, &home_dir))
        .await
        .map_err(|e| (code::INTERNAL, e.to_string()))??;
    let handle = match registry.handle(id) {
        Some(handle) => handle,
        None => {
            // An exited record whose actor is gone sleeps first, so the actor has one start.
            let mut record = info;
            if !can_hibernate(&record, true, true) {
                return Err(needs_launch());
            }
            mark_hibernated(&mut record, now_ms());
            session::start_hibernated(registry.clone(), record)
        }
    };
    handle.wake(prepared).await
}
