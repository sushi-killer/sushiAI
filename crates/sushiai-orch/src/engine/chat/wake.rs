use super::prompts::{state_header, wake_questions_prompt};
use super::store::{begin_turn, release_turn, save, BUSY};
use super::turn::ScheduledTurn;
use super::turn::Turn;
use crate::engine::messages;
use crate::engine::state::App;
use crate::overseer;
use crate::overseer_state;
use crate::task::state::TaskStatus;
use std::path::Path;

/// Starts a turn for the questions tasks sent the orchestrator in `repo`,
/// in the owner's current chat session, in the plain chat mode. A turn already running picks
/// them up when it ends; with no route configured they wait for one.
pub(in crate::engine) fn wake(app: &App, repo: &str) {
    if !Path::new(repo).is_dir() || !messages::has_pending_for_orchestrator(app, repo) {
        return;
    }
    let Ok((session, route, cancel)) = begin_turn(app, repo) else {
        return;
    };
    let Some((ids, questions)) = messages::take_for_orchestrator(app, repo) else {
        release_turn(app, repo);
        return;
    };
    save(app, repo, &session);
    let senders: Vec<String> = app
        .messages
        .lock()
        .unwrap()
        .iter()
        .filter(|m| ids.contains(&m.id))
        .map(|m| m.from.clone())
        .collect();
    let state = overseer_state::begin(
        &app.data_dir,
        repo,
        &app.store.list_tasks().unwrap_or_default(),
        &senders,
    );
    let prompt = wake_questions_prompt(&state, &questions);
    let repo = repo.to_string();
    ScheduledTurn {
        repo,
        thread: session,
        route,
        text: prompt,
        cancel,
        turn: Turn::Questions(ids),
    }
    .start(app);
}

/// Whether a wake for task `id` waits in `repo`'s queue behind the running
/// turn (a restart wake covers every task of the repo).
pub(in crate::engine) fn wake_queued_for(app: &App, repo: &str, id: &str) -> bool {
    app.deferred_wakes
        .lock()
        .unwrap()
        .get(repo)
        .is_some_and(|q| {
            q.iter().any(|w| {
                w.task_id.as_deref() == Some(id) || (w.task_id.is_none() && w.reason == "restart")
            })
        })
}

/// An overseer wake skipped because the chat was busy.
#[derive(Clone)]
pub(in crate::engine) struct WakeRequest {
    prompt: String,
    read_only: bool,
    reason: &'static str,
    task_id: Option<String>,
}

impl WakeRequest {
    pub(in crate::engine) fn new(
        prompt: String,
        read_only: bool,
        reason: &'static str,
        task_id: Option<&str>,
    ) -> Self {
        Self {
            prompt,
            read_only,
            reason,
            task_id: task_id.map(str::to_string),
        }
    }
}

/// A queued wake for tests.
#[cfg(test)]
pub(in crate::engine) fn test_deferred_wake(reason: &'static str, task_id: &str) -> WakeRequest {
    WakeRequest {
        prompt: String::new(),
        read_only: false,
        reason,
        task_id: Some(task_id.to_string()),
    }
}

/// Remembers a skipped wake for `repo`, once per reason and task.
fn defer_wake(app: &App, repo: &str, request: WakeRequest) {
    let WakeRequest {
        ref task_id,
        reason,
        ..
    } = request;
    let mut all = app.deferred_wakes.lock().unwrap();
    let queue = all.entry(repo.to_string()).or_default();
    if !queue
        .iter()
        .any(|w| w.reason == reason && w.task_id == *task_id)
    {
        queue.push(request);
    }
}

/// Whether the task of a deferred wake still needs it: a waiting or reminder
/// wake is moot once the question was answered.
fn wake_still_needed(app: &App, w: &WakeRequest) -> bool {
    let Some(id) = w.task_id.as_deref() else {
        return true;
    };
    if !matches!(w.reason, "waiting" | "reminder") {
        return true;
    }
    app.store
        .load_task(id)
        .ok()
        .flatten()
        .is_some_and(|t| t.status == TaskStatus::Waiting && t.question.is_some())
}

/// Fires the first deferred wake of `repo` that is still needed. The rest
/// stay queued: the turn it starts fires the next one when it ends.
pub(in crate::engine) fn fire_deferred_wakes(app: &App, repo: &str) {
    loop {
        if app.shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let next = {
            let mut all = app.deferred_wakes.lock().unwrap();
            let Some(queue) = all.get_mut(repo) else {
                return;
            };
            if queue.is_empty() {
                all.remove(repo);
                return;
            }
            queue.remove(0)
        };
        if !wake_still_needed(app, &next) {
            continue;
        }
        // Busy again re-queues it; a started wake ends the loop.
        wake_overseer(app, repo, next);
        return;
    }
}

/// Starts an overseer turn for `repo` with a server-built `prompt`; its
/// reply is appended to the repo's current chat session. `read_only` offers
/// only the looking tools. Returns whether it started: not when a turn is
/// already running, no route is configured or `repo` is not a directory.
pub(in crate::engine) fn wake_overseer(app: &App, repo: &str, request: WakeRequest) -> bool {
    let reason = request.reason;
    let read_only = request.read_only;
    let task_id = request.task_id.as_deref();
    if !Path::new(repo).is_dir() {
        app.hand_over_unwoken(repo, reason, task_id);
        return false;
    }
    if tokio::runtime::Handle::try_current().is_err()
        || app.shutting_down.load(std::sync::atomic::Ordering::SeqCst)
    {
        return false;
    }
    let Some((session, route, cancel)) = claim_overseer_turn(app, repo, &request) else {
        return false;
    };
    let about = app.wake_questions(repo, reason, task_id);
    let held = app.hold_for_overseer(repo, reason, task_id);
    app.record_overseer(overseer::Entry::new(
        "woke",
        repo,
        task_id,
        &format!("Woke for {reason}"),
    ));
    save(app, repo, &session);
    let wake_ids: Vec<String> = task_id.map(str::to_string).into_iter().collect();
    let state = overseer_state::begin(
        &app.data_dir,
        repo,
        &app.store.list_tasks().unwrap_or_default(),
        &wake_ids,
    );
    let prompt = format!("{}{}", state_header(&state), request.prompt);
    let repo = repo.to_string();
    let task_id = task_id.map(str::to_string);
    ScheduledTurn {
        repo,
        thread: session,
        route,
        text: prompt,
        cancel,
        turn: Turn::Overseer {
            read_only,
            reason,
            task_id,
            held,
            about,
        },
    }
    .start(app);
    true
}

fn claim_overseer_turn(
    app: &App,
    repo: &str,
    request: &WakeRequest,
) -> Option<(
    super::sessions::ChatSession,
    crate::policy::routing::Route,
    crate::harness::cancel::CancelToken,
)> {
    let reason = request.reason;
    let task_id = request.task_id.as_deref();
    match begin_turn(app, repo) {
        Ok(started) => Some(started),
        Err(error) => {
            // The digest retries every minute until it starts: log it once it does.
            if reason != "digest" {
                let why = if error == BUSY {
                    // Queued for the overseer is already the overseer's: the
                    // owner hears only when it hands the question on (or the
                    // hold's TTL ends).
                    app.hold_for_overseer(repo, reason, task_id);
                    defer_wake(app, repo, request.clone());
                    "chat is busy, queued for the end of its turn"
                } else {
                    // Nobody will take its questions: they are the owner's.
                    app.hand_over_unwoken(repo, reason, task_id);
                    "no route"
                };
                app.record_overseer(overseer::Entry::new(
                    "skipped",
                    repo,
                    task_id,
                    &format!("Not woken ({reason}): {why}"),
                ));
            }
            None
        }
    }
}
