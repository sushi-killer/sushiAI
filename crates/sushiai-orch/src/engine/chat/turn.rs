use super::launch::{prepare, PreparedLaunch};
use super::overseer::{finish_overseer, NoticeDisposition};
use super::reply::apply_reply;
use super::sessions::ChatSession;
use super::store::finish_turn;
use super::stream::run;
use super::types::ChatMode;
use super::wake::wake;
use crate::clip::truncate_chars;
use crate::clock::now_ms;
use crate::engine::chat_tools;
use crate::engine::messages;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::policy::routing::Route;
use std::sync::Arc;

/// Who a turn answers: the owner's chat message, or the questions tasks sent
/// meanwhile (their message ids). A question turn leaves the chat's messages
/// alone; its reply goes back to the tasks instead.
pub(super) enum Turn {
    Owner(ChatMode),
    Questions(Vec<String>),
    /// A turn the server starts on its own (a waiting task, a finished task,
    /// the morning digest): the prompt is built here, so it is trusted, and
    /// the final text lands in the owner's chat session like an owner turn's.
    Overseer {
        read_only: bool,
        /// Why it woke (`waiting`, `done`, `digest`) and the task it concerns,
        /// for the overseer log.
        reason: &'static str,
        task_id: Option<String>,
        /// The question holds this turn made (task id, hold time).
        held: Vec<(String, i64)>,
        /// The questions the turn is about (task id, `asked_at`), for the
        /// owner-notice gate.
        about: Vec<(String, i64)>,
    },
}

pub(super) struct ScheduledTurn {
    pub(super) repo: String,
    pub(super) thread: ChatSession,
    pub(super) route: Route,
    pub(super) text: String,
    pub(super) cancel: CancelToken,
    pub(super) turn: Turn,
}

impl ScheduledTurn {
    pub(super) fn start(self, app: &App) {
        let app = app.arc();
        tokio::spawn(async move {
            run_turn(&app, self).await;
        });
    }
}

async fn run_turn(app: &Arc<App>, mut scheduled: ScheduledTurn) {
    let started = now_ms();
    let launch = prepare(app, &scheduled).await;
    // A bounded overseer turn cannot leave a question or the chat held forever.
    let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let timer = matches!(scheduled.turn, Turn::Overseer { .. }).then(|| {
        let (cancel, flag) = (scheduled.cancel.clone(), timed_out.clone());
        let limit = app
            .overseer_turn_timeout_ms
            .load(std::sync::atomic::Ordering::SeqCst);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(limit)).await;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            cancel.cancel();
        })
    });
    let (result, tools) = match run(app, &mut scheduled, &launch).await {
        Ok((outcome, tools)) => (Ok(outcome), tools),
        Err(error) => (Err(error), Vec::new()),
    };
    let _ = std::fs::remove_file(&launch.key_path);
    if let Some(timer) = timer {
        timer.abort();
    }
    scheduled.thread.note = None;
    let execution = ExecutedTurn {
        result,
        tools,
        started,
        timed_out: timed_out.load(std::sync::atomic::Ordering::SeqCst),
    };
    let disposition = finish_overseer(app, &scheduled.repo, &scheduled.turn, &execution);
    apply_result(app, &mut scheduled, &launch, execution, disposition);
    finish_turn(app, &scheduled.repo, &scheduled.thread);
    // Questions that arrived during this turn get the next one.
    wake(app, &scheduled.repo);
}

pub(super) struct ExecutedTurn {
    pub(super) result: Result<crate::harness::RunOutcome, RunError>,
    pub(super) tools: Vec<super::types::ChatTool>,
    pub(super) started: i64,
    pub(super) timed_out: bool,
}

fn apply_result(
    app: &App,
    scheduled: &mut ScheduledTurn,
    launch: &PreparedLaunch,
    execution: ExecutedTurn,
    disposition: NoticeDisposition,
) {
    let ScheduledTurn { thread, turn, .. } = scheduled;
    let ExecutedTurn { result, tools, .. } = execution;
    let NoticeDisposition { blocked, notice } = disposition;
    let owner = matches!(turn, Turn::Owner(_));
    let mode = launch.mode;
    let settings = &launch.settings;
    match (result, &*turn) {
        (Ok(outcome), _) => {
            if owner && outcome.session_id.is_some() {
                thread.session_id = outcome.session_id.clone();
            }
            match (
                outcome.final_text.filter(|t| !t.trim().is_empty()),
                outcome.error,
                turn,
            ) {
                (Some(_), _, Turn::Overseer { .. }) if blocked => {}
                (Some(reply), _, Turn::Owner(_) | Turn::Overseer { .. }) => {
                    let reply = notice.unwrap_or(reply);
                    apply_reply(thread, &reply, tools, mode, &|server, tool| {
                        chat_tools::approved::proposable(app, settings, server, tool)
                    });
                }
                (Some(reply), _, Turn::Questions(ids)) => {
                    messages::reply_where_unanswered(app, ids, &reply)
                }
                (None, Some(error), _) => {
                    if owner {
                        // A session the CLI no longer knows fails every resume.
                        thread.session_id = None;
                        thread.error = Some(truncate_chars(&error, 2000));
                    }
                }
                (None, None, Turn::Owner(_)) => {
                    thread.error = Some("The orchestrator gave no reply.".into())
                }
                (None, None, Turn::Questions(_) | Turn::Overseer { .. }) => {}
            }
        }
        (Err(RunError::Cancelled), Turn::Owner(_)) => thread.error = Some("Stopped.".into()),
        (Err(RunError::Io(error) | RunError::NotFound(error)), Turn::Owner(_)) => {
            thread.error = Some(error)
        }
        (Err(_), Turn::Questions(_) | Turn::Overseer { .. }) => {}
    }
}
