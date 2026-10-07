//! One actor per session. It owns the screen, the stream offset and the holder connection.
//! Everything else talks to it through a bounded command queue and a broadcast of output.
//! The actor never awaits a socket write, so it always keeps reading the holder.

use std::collections::HashMap;
use std::future::pending;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sushiai_agents::heuristic::{self, ScreenAgent};
use sushiai_agents::hooks::{self, HookEvent};
use sushiai_agents::status::{Detected, ExitInfo, Input, SessionStatus as AgentState, Status};
use sushiai_core::hibernate::{can_hibernate, decide, BusyMeter, Candidate, Decision};
use sushiai_core::{mark_exited, mark_hibernated, Screen, SCROLLBACK_LINES};
use sushiai_protocol::{
    code, method, Ask, AskClosed, AskRespond, Frame, HoldAttach, HoldAttachResult, HoldExited,
    Message, Notification, SessionExited, SessionInfo, SessionMeta, SessionStatus,
    SessionStatusChanged, Signal,
};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{interval, sleep_until, timeout, Instant};

use crate::agent::{self, now_ms, random_hex, Prepared};
use crate::error::Fail;
use crate::foreground;
use crate::holder::{self, HolderConn, SendError};
use crate::procs;
use crate::registry::Registry;

const TERM_GRACE: Duration = Duration::from_secs(5);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_BACKLOG: usize = 256;
const MAX_PENDING: usize = 1024;
const RECONNECT_FIRST: Duration = Duration::from_millis(50);
const RECONNECT_MAX: Duration = Duration::from_secs(2);
/// A working agent that sends no hook for this long is no longer trusted to send them.
const HOOK_SILENCE: Duration = Duration::from_secs(15);
/// How long the process may outlive a `SessionEnd` hook.
const ENDING_GRACE: Duration = Duration::from_secs(3);
/// The screen is read at most this often: output can arrive in a flood.
const SCREEN_THROTTLE: Duration = Duration::from_millis(500);
/// Open permission asks per session; more are left to the agent's own terminal prompt.
const MAX_ASKS: usize = 20;
/// How often the foreground process of a shell session is looked at.
const FOREGROUND_POLL: Duration = Duration::from_secs(2);
/// Formatted history bytes one attach may carry; the oldest lines go first.
const HISTORY_BUDGET: usize = 4 * 1024 * 1024;
/// The saved screen of a sleeping session, history included.
const TAIL_MAX: usize = 1024 * 1024;
/// A tail is saved on a change to idle at most this often per session.
const TAIL_EVERY: Duration = Duration::from_secs(60);
/// Input typed while a session wakes, kept until it is ready.
const WAKE_QUEUE_MAX: usize = 64 * 1024;
/// A waking session is ready once its output has been quiet this long ...
const WAKE_QUIET: Duration = Duration::from_millis(800);
/// ... and at the latest this long after the wake began.
const WAKE_CAP: Duration = Duration::from_secs(20);
/// The old holder of a session that slept gets this long to be gone before a new one starts.
const HOLDER_GONE_WAIT: Duration = Duration::from_secs(3);
/// Most often the CPU of an agent's children is measured.
const SAMPLE_EVERY: Duration = Duration::from_secs(60);

/// A piece of output and its offset in the session's stream.
pub struct Chunk {
    pub seq: u64,
    pub data: Vec<u8>,
}

/// Screen state at a stream offset.
pub struct Snap {
    pub snapshot: Vec<u8>,
    pub seq: u64,
    pub cols: u16,
    pub rows: u16,
}

/// Plain screen text.
pub struct Text {
    pub text: String,
    pub rows: u16,
    pub cols: u16,
}

pub struct Attached {
    pub snap: Snap,
    /// Receives every chunk at or after `snap.seq`.
    pub output: broadcast::Receiver<Arc<Chunk>>,
}

type Reply = oneshot::Sender<Result<(), Fail>>;

enum Cmd {
    Input(Vec<u8>, Reply),
    Resize(u16, u16, Reply),
    Close(bool, Reply),
    Attach(usize, oneshot::Sender<Attached>),
    Read(usize, oneshot::Sender<Text>),
    Snapshot(oneshot::Sender<Snap>),
    Hook(Vec<u8>, oneshot::Sender<HookReply>),
    Respond(AskRespond, Reply),
    Wake(Box<Prepared>, Reply),
    Hibernate(Reply),
    /// The stored launch cannot open (or is gone): a hibernated session ends as exited.
    LaunchLost(Reply),
    /// The prune asks a hibernated session to remove itself when it is older than 30 days.
    Expire(u64),
    /// A wake was asked while the session is going to sleep: remembered for the sleep's end.
    WakeAsked(Reply),
    /// The wake that followed a sleep with input waiting failed: that input is dropped.
    SleepInputDropped,
    SaveTail(oneshot::Sender<()>),
    /// From the task that started a holder for a waking session.
    Spawned(Result<(HolderConn, u32), Fail>),
    /// From the task that measured the CPU of the agent's children.
    Cpu(Option<u64>),
}

/// Result of a hook event. For a permission ask: the receiver of the answer. The sender is
/// dropped when the ask times out or is cancelled, which means "no decision".
type HookReply = Result<Option<oneshot::Receiver<Value>>, Fail>;

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Cmd>,
}

fn not_running() -> Fail {
    (code::SESSION_NOT_RUNNING, "session is not running".into())
}

impl Handle {
    /// Queues a command without waiting for room, then waits (bounded) for the holder's answer.
    async fn ask(&self, make: impl FnOnce(Reply) -> Cmd) -> Result<(), Fail> {
        let (reply, answer) = oneshot::channel();
        self.tx.try_send(make(reply)).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => {
                (code::INPUT_BACKPRESSURE, "session is busy".to_string())
            }
            mpsc::error::TrySendError::Closed(_) => not_running(),
        })?;
        match timeout(ANSWER_TIMEOUT, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(not_running()),
            Err(_) => Err((code::INTERNAL, "holder did not answer".into())),
        }
    }

    pub async fn input(&self, data: Vec<u8>) -> Result<(), Fail> {
        self.ask(|r| Cmd::Input(data, r)).await
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), Fail> {
        self.ask(|r| Cmd::Resize(cols, rows, r)).await
    }

    pub async fn close(&self, graceful: bool) -> Result<(), Fail> {
        self.ask(|r| Cmd::Close(graceful, r)).await
    }

    pub async fn hook(&self, payload: Vec<u8>) -> HookReply {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Cmd::Hook(payload, tx))
            .await
            .map_err(|_| not_running())?;
        match timeout(ANSWER_TIMEOUT, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(not_running()),
            Err(_) => Err((code::INTERNAL, "session did not answer".into())),
        }
    }

    pub async fn respond(&self, answer: AskRespond) -> Result<(), Fail> {
        self.ask(|r| Cmd::Respond(answer, r)).await
    }

    /// Starts waking a hibernated or exited session with its freshly prepared launch.
    pub async fn wake(&self, prepared: Prepared) -> Result<(), Fail> {
        self.ask(|r| Cmd::Wake(Box::new(prepared), r)).await
    }

    /// A `session.wake` that arrived while the session goes to sleep.
    pub async fn wake_asked(&self) -> Result<(), Fail> {
        self.ask(Cmd::WakeAsked).await
    }

    /// The launch of this session is gone for good: it must not wake any more.
    pub async fn launch_lost(&self) -> Result<(), Fail> {
        self.ask(Cmd::LaunchLost).await
    }

    /// Asks a hibernated session to remove itself if it is older than the limit; a wake that
    /// came first wins, because both go through the same queue. Does not wait.
    pub fn expire(&self, now_ms: u64) {
        let _ = self.tx.try_send(Cmd::Expire(now_ms));
    }

    pub async fn hibernate(&self) -> Result<(), Fail> {
        self.ask(Cmd::Hibernate).await
    }

    /// Saves the screen tail of a running agent session (daemon stop).
    pub async fn save_tail(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Cmd::SaveTail(tx)).await.is_ok() {
            let _ = timeout(ANSWER_TIMEOUT, rx).await;
        }
    }

    /// Attaches with up to `scrollback` history lines before the screen snapshot.
    pub async fn attach(&self, scrollback: usize) -> Option<Attached> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Attach(scrollback, tx)).await.ok()?;
        rx.await.ok()
    }

    /// The screen as plain text, with up to `scrollback` history lines before it.
    pub async fn read(&self, scrollback: usize) -> Option<Text> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Read(scrollback, tx)).await.ok()?;
        rx.await.ok()
    }

    pub async fn snapshot(&self) -> Option<Snap> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Snapshot(tx)).await.ok()?;
        rx.await.ok()
    }
}

/// A request sent to the holder that waits for its answer.
enum Pending {
    Ack(Reply),
    Resize(u16, u16, Reply),
}

struct OpenAsk {
    ask: Ask,
    tx: oneshot::Sender<Value>,
    deadline: Instant,
}

/// What the actor tracks for a session with an agent status: one that sends hooks, or one whose
/// status is read from the screen (`gemini`, `cursor-agent`).
struct Agent {
    status: AgentState,
    /// Stamped on each received hook; the state machine drops stale ones.
    seq: u64,
    last_hook: Instant,
    ending_at: Option<Instant>,
    asks: Vec<OpenAsk>,
}

/// A session that is waking: its process starts and its input waits.
struct Wake {
    started: Instant,
    /// Typed meanwhile, sent in order once the agent is ready.
    queue: Vec<u8>,
    last_output: Option<Instant>,
    /// A hook came before the holder was connected: finish as soon as it is.
    hooked: bool,
    /// A resize that came before the holder was there.
    resize: Option<(u16, u16)>,
}

struct Actor {
    agent: Option<Agent>,
    info: SessionInfo,
    screen: Screen,
    seq: u64,
    /// None while the session sleeps.
    holder: Option<HolderConn>,
    /// Offset of the holder's stream start in this session's stream: a new holder counts
    /// from 0 again after a wake, the stream seen by clients does not.
    base: u64,
    holder_open: bool,
    pending: HashMap<u64, Pending>,
    output: broadcast::Sender<Arc<Chunk>>,
    registry: Arc<Registry>,
    kill_at: Option<Instant>,
    sock: PathBuf,
    /// When to try the holder socket again, and the delay after that.
    retry: Option<(Instant, Duration)>,
    /// Id of the `hold.attach` sent after a reconnect, until its answer arrives.
    attaching: Option<u64>,
    /// A close asked for while detached: sent once the holder is reachable again.
    close_pending: Option<bool>,
    /// Which screen heuristic reads this session, if any.
    screen_agent: Option<ScreenAgent>,
    /// When the screen was last read, and when the next read is due after output was held back.
    screen_read: Option<Instant>,
    screen_due: Option<Instant>,
    /// Pid of the PTY child, and when the foreground was last looked at. Only a session started
    /// without an agent is watched for one started by hand.
    child: Option<u32>,
    watch_foreground: bool,
    foreground_polled: Option<Instant>,
    /// Leader of the foreground group at the last poll: a new leader is a new agent process.
    foreground_leader: Option<u32>,
    me: mpsc::WeakSender<Cmd>,
    /// A sealed launch exists: the session can be woken.
    has_launch: bool,
    /// Unix ms of the last input a client sent.
    last_input_ms: u64,
    /// A TERM was sent to put the session to sleep: the exit that follows is a sleep.
    hibernating: bool,
    /// Typed while the process ends for a sleep (at most `WAKE_QUEUE_MAX`): handed to the wake.
    sleep_queue: Vec<u8>,
    /// A wake was asked while the process ended for a sleep.
    wake_asked: bool,
    /// The owner closed the session: its launch and tail go when it ends.
    closing: bool,
    /// The owner closed it (kept for good): no wake may revive it, not even a late one.
    closed: bool,
    /// The record was removed by the prune: the actor ends.
    removed: bool,
    wake: Option<Wake>,
    wake_due: Option<Instant>,
    /// Pid of the holder that ended with the last sleep; a new holder waits for it to be gone.
    dead_holder: Option<u32>,
    tail_saved: Option<Instant>,
    meter: BusyMeter,
    sampling: bool,
    sampled: Option<Instant>,
}

/// Starts the actor for a session whose holder is attached and replayed up to `seq`.
pub fn start(
    registry: Arc<Registry>,
    info: SessionInfo,
    holder: HolderConn,
    screen: Screen,
    seq: u64,
    sock: PathBuf,
    created: bool,
) -> Handle {
    spawn_actor(registry, info, Some(holder), screen, seq, sock, created)
}

/// Starts the actor of a session that has no process: a hibernated record (daemon start, or
/// a wake of an exited one). It serves attach and read from the saved tail.
pub fn start_hibernated(registry: Arc<Registry>, info: SessionInfo) -> Handle {
    let sock = registry.home.sessions().join(format!("{}.sock", info.id));
    let screen = Screen::new(info.rows, info.cols);
    spawn_actor(registry, info, None, screen, 0, sock, false)
}

fn spawn_actor(
    registry: Arc<Registry>,
    info: SessionInfo,
    holder: Option<HolderConn>,
    screen: Screen,
    seq: u64,
    sock: PathBuf,
    created: bool,
) -> Handle {
    let (tx, rx) = mpsc::channel(64);
    let me = tx.downgrade();
    let handle = Handle { tx };
    let mut info = info;
    let info_name = info.agent.name.clone();
    if let Some(holder) = &holder {
        info.holder_pid = holder.holder_pid.or(info.holder_pid);
    }
    registry.update(info.clone());
    if created {
        // Before the handle exists and the actor runs: `session.created` precedes any
        // `session.status`, `session.ask` or `session.exited` of this session.
        registry.announce_created(&info.id);
    }
    registry.set_handle(&info.id, handle.clone());
    let watch_foreground = agent::agent_of(info_name.as_deref()).is_none()
        && agent::screen_agent_of(info_name.as_deref()).is_none();
    let child = holder
        .as_ref()
        .and_then(|h| foreground::child_pid(h.child_pid, info.holder_pid));
    let has_launch = registry.launches().exists(&info.id);
    let mut actor = Actor {
        agent: restore_agent(&info),
        info,
        screen,
        seq,
        base: 0,
        holder_open: holder.is_some(),
        holder,
        pending: HashMap::new(),
        output: broadcast::channel(OUTPUT_BACKLOG).0,
        registry,
        kill_at: None,
        sock,
        retry: None,
        attaching: None,
        close_pending: None,
        screen_agent: agent::screen_agent_of(info_name.as_deref()),
        screen_read: None,
        screen_due: None,
        child,
        watch_foreground,
        foreground_polled: None,
        foreground_leader: None,
        me,
        has_launch,
        last_input_ms: 0,
        hibernating: false,
        sleep_queue: Vec::new(),
        wake_asked: false,
        closing: false,
        closed: false,
        removed: false,
        wake: None,
        wake_due: None,
        dead_holder: None,
        tail_saved: None,
        meter: BusyMeter::default(),
        sampling: false,
        sampled: None,
    };
    // The screen was replayed before the actor existed: read it once now.
    actor.screen_changed();
    tokio::spawn(actor.run(rx));
    handle
}

/// The state machine for a hook-capable agent, resumed from the persisted record. A `blocked`
/// status stays blocked: the agent may still show its dialog. The ask itself died with the
/// previous daemon, so the machine is told the ask went back to the terminal.
fn restore_agent(info: &SessionInfo) -> Option<Agent> {
    // A screen-only agent has a status in its record; any other plain command has none.
    if agent::agent_of(info.agent.name.as_deref()).is_none()
        && (agent::screen_agent_of(info.agent.name.as_deref()).is_none()
            || info.agent.agent_status.is_none())
    {
        return None;
    }
    let a = &info.agent;
    let mut status = AgentState::new(a.status_since.unwrap_or_else(now_ms));
    if let Some(s) = a.agent_status {
        status.status = agent::status_from_wire(s);
    }
    if let Some(s) = a.status_source {
        status.source = agent::source_from_wire(s);
    }
    if status.status == sushiai_agents::status::Status::Blocked {
        status.blocked_kind = Some(sushiai_agents::status::BlockedKind::Permission);
        status.apply(Input::PermissionClosed { decided: false }, now_ms());
    }
    status.agent_session_id = a.agent_session.clone();
    status.transcript_path = a.transcript_path.clone();
    Some(Agent {
        status,
        seq: 0,
        // A fresh start gets a full silence window.
        last_hook: Instant::now(),
        ending_at: None,
        asks: Vec::new(),
    })
}

impl Actor {
    async fn run(mut self, mut commands: mpsc::Receiver<Cmd>) {
        let mut tick = interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.command(command),
                    None => break,
                },
                frame = next_frame(&mut self.holder), if self.holder_open => {
                    self.holder_frame(frame.ok().flatten());
                }
                () = until(self.wake_due) => self.wake_check(),
                () = until(self.kill_at) => {
                    self.kill_at = None;
                    // Fire and forget: the answer is not tracked.
                    if let Some(holder) = self.holder.as_mut() {
                        let _ = holder.close(Signal::Kill);
                    }
                }
                () = until(self.retry.map(|(at, _)| at)) => self.reconnect().await,
                () = until(self.screen_due) => self.read_screen(),
                _ = tick.tick() => {
                    self.sweep();
                    self.poll_foreground();
                    self.hibernate_tick();
                }
            }
            if self.removed {
                break;
            }
            // An exited session with nobody attached has nothing left to serve.
            if self.info.status == SessionStatus::Exited && self.output.receiver_count() == 0 {
                break;
            }
        }
        self.registry.clear_handle(&self.info.id);
    }

    fn running(&self) -> bool {
        self.info.status == SessionStatus::Running
    }

    /// The holder takes requests: the session runs or is waking and its holder is here.
    fn live(&self) -> bool {
        matches!(
            self.info.status,
            SessionStatus::Running | SessionStatus::Waking
        ) && self.holder.is_some()
    }

    /// Sends a request to the holder and parks `pending` until its answer arrives.
    /// `build` gets the holder connection and returns the request id.
    fn track(
        &mut self,
        reply: Reply,
        build: impl FnOnce(&mut HolderConn) -> Result<u64, SendError>,
        pending: impl FnOnce(Reply) -> Pending,
    ) {
        let live = self.live();
        let Some(holder) = self.holder.as_mut().filter(|_| live) else {
            let _ = reply.send(Err(not_running()));
            return;
        };
        if self.pending.len() >= MAX_PENDING {
            let _ = reply.send(Err((code::INPUT_BACKPRESSURE, "too many requests".into())));
        } else {
            match build(holder) {
                Ok(id) => {
                    self.pending.insert(id, pending(reply));
                }
                Err(SendError::Full) => {
                    let _ = reply.send(Err((code::INPUT_BACKPRESSURE, "holder is busy".into())));
                }
                Err(SendError::Closed) => {
                    let _ = reply.send(Err(not_running()));
                }
            }
        }
    }

    fn command(&mut self, command: Cmd) {
        match command {
            Cmd::Input(data, reply) => self.input(data, reply),
            Cmd::Resize(cols, rows, reply) => match self.info.status {
                SessionStatus::Hibernated => {
                    // The size the session wakes at.
                    self.info.cols = cols;
                    self.info.rows = rows;
                    self.screen.resize(rows, cols);
                    self.registry.update(self.info.clone());
                    let _ = reply.send(Ok(()));
                }
                SessionStatus::Waking if self.holder.is_none() => {
                    if let Some(wake) = self.wake.as_mut() {
                        wake.resize = Some((cols, rows));
                    }
                    let _ = reply.send(Ok(()));
                }
                _ => self.track(
                    reply,
                    |h| h.resize(cols, rows),
                    |r| Pending::Resize(cols, rows, r),
                ),
            },
            Cmd::Close(_, reply) if self.info.status == SessionStatus::Hibernated => {
                // Nothing runs: the session ends as exited, and its launch and tail go.
                self.closing = true;
                self.closed = true;
                self.exited(None);
                let _ = reply.send(Ok(()));
            }
            Cmd::Close(graceful, reply)
                if self.info.status == SessionStatus::Detached
                    || (self.info.status == SessionStatus::Waking && self.holder.is_none()) =>
            {
                // Not connected to the holder right now: close as soon as it is back, or end
                // as exited if it turns out to be gone.
                self.closing = true;
                self.closed = true;
                self.close_pending = Some(graceful);
                let _ = reply.send(Ok(()));
            }
            Cmd::Close(graceful, reply) => {
                self.closing = true;
                self.closed = true;
                let signal = if graceful { Signal::Term } else { Signal::Kill };
                self.track(reply, |h| h.close(signal), Pending::Ack);
                if graceful
                    && matches!(
                        self.info.status,
                        SessionStatus::Running | SessionStatus::Waking
                    )
                {
                    self.kill_at = Some(Instant::now() + TERM_GRACE);
                }
            }
            Cmd::Attach(scrollback, reply) => {
                let output = self.output.subscribe();
                let mut snap = self.snap();
                if scrollback > 0 {
                    snap.snapshot = self.with_history(scrollback, snap.snapshot);
                }
                let _ = reply.send(Attached { output, snap });
            }
            Cmd::Read(scrollback, reply) => {
                let _ = reply.send(self.text(scrollback));
            }
            Cmd::Snapshot(reply) => {
                let _ = reply.send(self.snap());
            }
            Cmd::Hook(payload, reply) => {
                let _ = reply.send(self.hook(&payload));
            }
            Cmd::Respond(answer, reply) => {
                let _ = reply.send(self.respond(answer));
            }
            Cmd::Wake(prepared, reply) => {
                let result = if self.closed {
                    Err((
                        code::WAKE_NEEDS_LAUNCH,
                        "the session was closed".to_string(),
                    ))
                } else {
                    self.begin_wake(*prepared);
                    Ok(())
                };
                let _ = reply.send(result);
            }
            Cmd::LaunchLost(reply) => {
                if self.info.status == SessionStatus::Hibernated {
                    self.closing = true;
                    self.exited(None);
                } else {
                    self.registry.drop_files(&self.info.id);
                }
                let _ = reply.send(Ok(()));
            }
            Cmd::Expire(now) => {
                let old = self.info.agent.hibernated_at.is_some_and(|at| {
                    now.saturating_sub(at) > crate::registry::HIBERNATED_MAX_AGE_MS
                });
                if old && self.info.status == SessionStatus::Hibernated && self.wake.is_none() {
                    self.registry.remove_hibernated(&self.info.id);
                    self.removed = true;
                }
            }
            Cmd::WakeAsked(reply) => {
                self.wake_asked |= self.hibernating;
                let _ = reply.send(Ok(()));
            }
            Cmd::SleepInputDropped => {
                self.sleep_queue.clear();
                self.wake_asked = false;
            }
            Cmd::Hibernate(reply) => {
                let _ = reply.send(self.hibernate_now());
            }
            Cmd::SaveTail(reply) => {
                if self.running() && self.can_sleep() {
                    self.save_tail();
                }
                let _ = reply.send(());
            }
            Cmd::Spawned(spawned) => self.spawned(spawned),
            Cmd::Cpu(total) => {
                self.sampling = false;
                if let Some(total) = total {
                    self.meter.observe(now_ms(), total);
                }
            }
        }
    }

    /// Input from a client: sent on, or kept while the session wakes.
    fn input(&mut self, data: Vec<u8>, reply: Reply) {
        self.last_input_ms = now_ms();
        match (&mut self.wake, self.info.status) {
            (Some(wake), SessionStatus::Waking) => {
                let result = if wake.queue.len() + data.len() > WAKE_QUEUE_MAX {
                    Err((code::INPUT_BACKPRESSURE, "input queue is full".into()))
                } else {
                    wake.queue.extend(data);
                    Ok(())
                };
                let _ = reply.send(result);
            }
            // The process is ending for a sleep: its input would die with it.
            _ if self.hibernating && self.info.status == SessionStatus::Running => {
                let result = if self.sleep_queue.len() + data.len() > WAKE_QUEUE_MAX {
                    Err((code::INPUT_BACKPRESSURE, "input queue is full".into()))
                } else {
                    self.sleep_queue.extend(data);
                    Ok(())
                };
                let _ = reply.send(result);
            }
            _ => self.track(reply, |h| h.input(data), Pending::Ack),
        }
    }

    fn hook(&mut self, payload: &[u8]) -> HookReply {
        let no_agent = || {
            (
                code::INVALID_PARAMS,
                "session has no agent hooks".to_string(),
            )
        };
        // Only claude and codex send hooks; a screen-read agent has no token either.
        if agent::agent_of(self.info.agent.name.as_deref()).is_none() {
            return Err(no_agent());
        }
        let parsed = hooks::parse(payload).map_err(|e| (code::INVALID_PARAMS, e.to_string()))?;
        let agent = self.agent.as_mut().ok_or_else(no_agent)?;
        agent.seq += 1;
        agent.last_hook = Instant::now();
        let ask = match &parsed.event {
            HookEvent::PermissionRequest(r) => Some((r.tool_name.clone(), r.tool_input.clone())),
            _ => None,
        };
        let input = Input::Hook {
            seq: agent.seq,
            payload: Box::new(parsed),
        };
        agent.status.apply(input, now_ms());
        if agent.status.ending && agent.ending_at.is_none() {
            agent.ending_at = Some(Instant::now() + ENDING_GRACE);
        }
        let answer = match ask {
            Some((tool, input)) => self.open_ask(tool, input)?,
            None => None,
        };
        self.publish();
        // A hook from the new process: the agent is up.
        if self.info.status == SessionStatus::Waking {
            self.finish_wake();
        }
        Ok(answer)
    }

    fn open_ask(
        &mut self,
        tool: Option<String>,
        input: Value,
    ) -> Result<Option<oneshot::Receiver<Value>>, Fail> {
        if self
            .agent
            .as_ref()
            .is_some_and(|a| a.asks.len() >= MAX_ASKS)
        {
            // No decision: the agent asks in its terminal.
            return Ok(None);
        }
        let input = agent::clip_input(input);
        let ask_id = random_hex(8).map_err(|e| (code::INTERNAL, e.to_string()))?;
        let ask = Ask {
            ask_id,
            session: self.info.id.clone(),
            tool,
            input,
        };
        let (tx, rx) = oneshot::channel();
        if let Some(agent) = self.agent.as_mut() {
            agent.asks.push(OpenAsk {
                ask: ask.clone(),
                tx,
                deadline: Instant::now() + agent::ask_timeout(),
            });
        }
        self.info.agent.asks.push(ask.clone());
        self.registry.update(self.info.clone());
        let _ = self
            .registry
            .events
            .send(Notification::new(method::SESSION_ASK, ask));
        Ok(Some(rx))
    }

    fn respond(&mut self, answer: AskRespond) -> Result<(), Fail> {
        let not_found = || (code::ASK_NOT_FOUND, format!("no ask {}", answer.ask_id));
        let agent = self.agent.as_mut().ok_or_else(not_found)?;
        let at = agent
            .asks
            .iter()
            .position(|a| a.ask.ask_id == answer.ask_id)
            .ok_or_else(not_found)?;
        let open = agent.asks.remove(at);
        let _ = open.tx.send(agent::permission_answer(
            answer.decision,
            answer.message.as_deref(),
        ));
        self.asks_closed(&[open.ask.ask_id], true);
        Ok(())
    }

    /// Asks were answered (`decided`), timed out or cancelled (handed back to the terminal).
    /// The state machine hears about it when none is left open. Always saves the record.
    fn asks_closed(&mut self, closed: &[String], decided: bool) {
        let Some(agent) = self.agent.as_mut() else {
            return;
        };
        let open: Vec<Ask> = agent.asks.iter().map(|a| a.ask.clone()).collect();
        if open.is_empty() {
            agent
                .status
                .apply(Input::PermissionClosed { decided }, now_ms());
        }
        self.info.agent.asks = open;
        self.publish();
        self.registry.update(self.info.clone());
        for ask_id in closed {
            let params = AskClosed {
                ask_id: ask_id.clone(),
                decided,
            };
            let _ = self
                .registry
                .events
                .send(Notification::new(method::SESSION_ASK_CLOSED, params));
        }
    }

    /// Once a second: expire asks, and run the silence and ending timers.
    fn sweep(&mut self) {
        let Some(agent) = self.agent.as_mut() else {
            return;
        };
        let now = Instant::now();
        let (live, dead): (Vec<OpenAsk>, Vec<OpenAsk>) = std::mem::take(&mut agent.asks)
            .into_iter()
            .partition(|a| a.deadline > now && !a.tx.is_closed());
        agent.asks = live;
        let expired: Vec<String> = dead.into_iter().map(|a| a.ask.ask_id).collect();
        let silent = agent.status.source == sushiai_agents::status::StatusSource::Hook
            && agent.status.status == sushiai_agents::status::Status::Working
            && agent.last_hook + HOOK_SILENCE <= now;
        if silent {
            agent.status.apply(Input::HookSilence, now_ms());
        }
        if agent.ending_at.is_some_and(|at| at <= now) {
            agent.ending_at = None;
            agent.status.apply(Input::EndingTimeout, now_ms());
        }
        if !expired.is_empty() {
            self.asks_closed(&expired, false);
        } else {
            self.publish();
        }
    }

    /// Every `FOREGROUND_POLL`: an agent the owner started by hand in a shell is reported as
    /// `foregroundAgent` and `foregroundCwd`, with its own conversation id as `agentSession`
    /// when its files name one. All clear when the shell owns the terminal again. The record
    /// never becomes a hook agent. After the session exits the last values stay: that is what
    /// Reopen continues.
    fn poll_foreground(&mut self) {
        let Some(child) = self
            .child
            .filter(|_| self.watch_foreground && self.running())
        else {
            return;
        };
        if self
            .foreground_polled
            .is_some_and(|at| at.elapsed() < FOREGROUND_POLL)
        {
            return;
        }
        self.foreground_polled = Some(Instant::now());
        let seen = foreground::foreground(child);
        let same_leader = seen
            .as_ref()
            .is_some_and(|s| self.foreground_leader == Some(s.leader));
        self.foreground_leader = seen.as_ref().map(|s| s.leader);
        let known = self
            .info
            .agent
            .agent_session
            .clone()
            .filter(|_| same_leader);
        let session = seen.as_ref().and_then(|s| self.conversation(s, known));
        let record = &mut self.info.agent;
        let name = seen.as_ref().map(|s| s.agent.to_string());
        let cwd = seen.and_then(|s| s.cwd);
        if record.foreground_agent == name
            && record.foreground_cwd == cwd
            && record.agent_session == session
        {
            return;
        }
        record.foreground_agent = name;
        record.foreground_cwd = cwd;
        record.agent_session = session;
        self.registry.update(self.info.clone());
        self.send_meta();
    }

    /// The conversation id of a hand-started agent, or `known` when its files do not say.
    /// Claude names its pid in a file. Codex's id is the rollout file its process holds open,
    /// checked again on every poll: one that holds none (it talks to the shared app server)
    /// has no id.
    fn conversation(&self, seen: &foreground::Seen, known: Option<String>) -> Option<String> {
        match seen.agent {
            "claude" => seen
                .config_dir("CLAUDE_CONFIG_DIR", ".claude")
                .and_then(|dir| foreground::claude_session(&dir, seen.leader, foreground::pgrp_of))
                .or(known),
            "codex" => {
                let home = seen.config_dir("CODEX_HOME", ".codex")?;
                foreground::codex_session(&home, &foreground::group_open_files(seen.leader))
            }
            _ => None,
        }
    }

    /// Clears what hand-started agent detection recorded. True when something was set.
    fn forget_foreground(&mut self) -> bool {
        let record = &mut self.info.agent;
        let had = record.foreground_agent.is_some()
            || record.foreground_cwd.is_some()
            || record.agent_session.is_some();
        record.foreground_agent = None;
        record.foreground_cwd = None;
        record.agent_session = None;
        had
    }

    fn send_meta(&self) {
        let record = &self.info.agent;
        let params = SessionMeta {
            id: self.info.id.clone(),
            agent_session: record.agent_session.clone(),
            transcript_path: record.transcript_path.clone(),
            foreground_agent: record.foreground_agent.clone(),
            foreground_cwd: record.foreground_cwd.clone(),
        };
        let _ = self
            .registry
            .events
            .send(Notification::new(method::SESSION_META, params));
    }

    /// Copies the state machine into the session record, saves it and announces changes.
    fn publish(&mut self) {
        let Some(agent) = &self.agent else {
            return;
        };
        let s = &agent.status;
        let record = &mut self.info.agent;
        let status = (
            Some(agent::status_to_wire(s.status)),
            Some(agent::source_to_wire(s.source)),
            Some(s.since),
        );
        let status_changed = (
            record.agent_status,
            record.status_source,
            record.status_since,
        ) != status;
        let meta_changed = record.agent_session != s.agent_session_id
            || record.transcript_path != s.transcript_path;
        (
            record.agent_status,
            record.status_source,
            record.status_since,
        ) = status;
        record.agent_session = s.agent_session_id.clone();
        record.transcript_path = s.transcript_path.clone();
        if !status_changed && !meta_changed {
            return;
        }
        self.registry.update(self.info.clone());
        if status_changed && status.0 == Some(sushiai_protocol::AgentStatus::Idle) {
            self.save_tail_throttled();
        }
        let id = self.info.id.clone();
        if let (true, Some(status), Some(status_source), Some(status_since)) =
            (status_changed, status.0, status.1, status.2)
        {
            let params = SessionStatusChanged {
                id: id.clone(),
                status,
                status_source,
                status_since,
            };
            let _ = self
                .registry
                .events
                .send(Notification::new(method::SESSION_STATUS, params));
        }
        if meta_changed {
            self.send_meta();
        }
    }

    /// History lines in front of `snapshot`. The snapshot clears the screen, so the lines are
    /// scrolled up out of the way first: the cursor goes to the bottom row and one line break
    /// per row pushes the last of them into the terminal's own scrollback.
    fn with_history(&mut self, scrollback: usize, snapshot: Vec<u8>) -> Vec<u8> {
        self.with_history_within(scrollback, snapshot, HISTORY_BUDGET)
    }

    /// `with_history` with at most `budget` bytes of history lines.
    fn with_history_within(
        &mut self,
        scrollback: usize,
        snapshot: Vec<u8>,
        budget: usize,
    ) -> Vec<u8> {
        let mut lines = self.screen.history_formatted(scrollback);
        // Each line also costs its reset and line break.
        let mut size: usize = lines.iter().map(|l| l.len() + 5).sum();
        let mut skip = 0;
        while size > budget && skip < lines.len() {
            size -= lines[skip].len() + 5;
            skip += 1;
        }
        lines.drain(..skip);
        if lines.is_empty() {
            return snapshot;
        }
        let (rows, _) = self.screen.size();
        let mut out = format!("\x1b[m\x1b[{rows};1H").into_bytes();
        for line in lines {
            out.extend(line);
            out.extend(b"\x1b[m\r\n");
        }
        out.extend(b"\r\n".repeat(usize::from(rows).saturating_sub(1)));
        out.extend(snapshot);
        out
    }

    fn text(&mut self, scrollback: usize) -> Text {
        if self.info.status == SessionStatus::Hibernated {
            // The tail replayed on a screen of the session's size.
            let mut screen = Screen::new(self.info.rows, self.info.cols);
            screen.feed(&self.read_tail());
            return screen_text(&mut screen, scrollback);
        }
        screen_text(&mut self.screen, scrollback)
    }

    fn read_tail(&self) -> Vec<u8> {
        std::fs::read(self.registry.home.tail_file(&self.info.id)).unwrap_or_default()
    }

    fn snap(&self) -> Snap {
        let (rows, cols) = self.screen.size();
        // A sleeping session shows what it showed when it went to sleep.
        let snapshot = match self.info.status {
            SessionStatus::Hibernated => self.read_tail(),
            _ => self.screen.snapshot(),
        };
        Snap {
            snapshot,
            seq: self.seq,
            cols,
            rows,
        }
    }

    /// `None` means the holder connection ended.
    fn holder_frame(&mut self, frame: Option<Frame>) {
        match frame {
            Some(Frame::Output { seq, data, .. }) => self.output_chunk(seq, data),
            Some(Frame::Json(text)) => match Message::parse(&text) {
                Ok(Message::Notification(n)) if n.method == method::HOLD_EXITED => {
                    let code = serde_json::from_value::<HoldExited>(n.params)
                        .ok()
                        .and_then(|e| e.code);
                    self.exited(code);
                }
                Ok(Message::Response(r)) => self.answer(r),
                _ => {}
            },
            None => self.holder_lost(),
        }
    }

    /// The holder connection ended. That is not an exit: the holder may still run (it drops a
    /// client that falls too far behind). Reconnect until it answers or its socket is gone.
    fn holder_lost(&mut self) {
        self.holder_open = false;
        self.attaching = None;
        for (_, pending) in self.pending.drain() {
            let (Pending::Ack(reply) | Pending::Resize(_, _, reply)) = pending;
            let _ = reply.send(Err(not_running()));
        }
        match self.info.status {
            SessionStatus::Exited | SessionStatus::Hibernated => {}
            status => {
                if status == SessionStatus::Running {
                    self.info.status = SessionStatus::Detached;
                    self.registry.update(self.info.clone());
                }
                self.retry = Some((Instant::now() + RECONNECT_FIRST, RECONNECT_FIRST));
            }
        }
    }

    /// True when the holder's pid is known and no such process exists. A holder that is alive
    /// but not answering (its client backlog was full) keeps being retried.
    fn holder_dead(&self) -> bool {
        let Some(pid) = self.info.holder_pid.and_then(|p| i32::try_from(p).ok()) else {
            return false;
        };
        // SAFETY: kill(2) with signal 0 only checks that the process exists.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        !alive && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }

    async fn reconnect(&mut self) {
        let delay = self.retry.map_or(RECONNECT_FIRST, |(_, d)| d);
        self.retry = None;
        match HolderConn::connect(&self.sock).await {
            Ok(mut conn) => {
                // Ask for everything after the last byte seen; a gap shows up as a jump in seq.
                let from_seq = self.seq.saturating_sub(self.base);
                if let Ok(id) = conn.request(method::HOLD_ATTACH, HoldAttach { from_seq }) {
                    self.holder = Some(conn);
                    self.holder_open = true;
                    self.attaching = Some(id);
                    return;
                }
            }
            Err(_) if !self.sock.exists() || self.holder_dead() => {
                // The holder is gone (a killed holder leaves its socket file behind).
                let _ = std::fs::remove_file(&self.sock);
                self.exited(None);
                return;
            }
            Err(_) => {}
        }
        let delay = (delay * 2).min(RECONNECT_MAX);
        self.retry = Some((Instant::now() + delay, delay));
    }

    fn answer(&mut self, response: sushiai_protocol::Response) {
        if response.id.as_u64().is_some() && response.id.as_u64() == self.attaching {
            self.attaching = None;
            let attached = response
                .result
                .and_then(|r| serde_json::from_value::<HoldAttachResult>(r).ok());
            if let Some(attached) = attached {
                if self.info.status != SessionStatus::Waking {
                    self.info.status = SessionStatus::Running;
                }
                self.info.holder_pid = Some(attached.pid).filter(|p| *p != 0);
                self.child =
                    foreground::child_pid(attached.child.filter(|p| *p != 0), self.info.holder_pid)
                        .or(self.child);
                self.registry.update(self.info.clone());
                if let Some(graceful) = self.close_pending.take() {
                    let signal = if graceful { Signal::Term } else { Signal::Kill };
                    if let Some(holder) = self.holder.as_mut() {
                        let _ = holder.close(signal);
                    }
                    if graceful {
                        self.kill_at = Some(Instant::now() + TERM_GRACE);
                    }
                }
            } else {
                self.holder_lost();
            }
            return;
        }
        let Some(pending) = response.id.as_u64().and_then(|id| self.pending.remove(&id)) else {
            return;
        };
        let outcome = match response.error {
            Some(e) => Err((e.code, e.message)),
            None => Ok(()),
        };
        match pending {
            Pending::Ack(reply) => {
                let _ = reply.send(outcome);
            }
            Pending::Resize(cols, rows, reply) => {
                // The screen follows the PTY only once the holder has resized it.
                if outcome.is_ok() {
                    self.screen.resize(rows, cols);
                    self.info.cols = cols;
                    self.info.rows = rows;
                    self.registry.update(self.info.clone());
                }
                let _ = reply.send(outcome);
            }
        }
    }

    fn output_chunk(&mut self, seq: u64, mut data: Vec<u8>) {
        let seq = seq + self.base;
        if let Some(wake) = self.wake.as_mut() {
            // Ready once the output goes quiet.
            let now = Instant::now();
            wake.last_output = Some(now);
            self.wake_due = Some((now + WAKE_QUIET).min(wake.started + WAKE_CAP));
        }
        // Skip bytes already seen; a gap (holder ring wrapped) just moves the offset forward.
        let (seq, skip) = if seq < self.seq {
            (self.seq, (self.seq - seq) as usize)
        } else {
            (seq, 0)
        };
        if skip >= data.len() {
            return;
        }
        data.drain(..skip);
        self.screen.feed(&data);
        self.screen_changed();
        self.seq = seq + data.len() as u64;
        // No receivers is normal when nobody is attached.
        let _ = self.output.send(Arc::new(Chunk { seq, data }));
    }

    /// Output arrived: read the screen now, or once the throttle allows when it was read lately.
    fn screen_changed(&mut self) {
        if self.screen_agent.is_none() || self.screen_due.is_some() {
            return;
        }
        match self.screen_read {
            Some(at) if at.elapsed() < SCREEN_THROTTLE => {
                self.screen_due = Some(at + SCREEN_THROTTLE);
            }
            _ => self.read_screen(),
        }
    }

    /// Reads the screen with the agent's heuristic. The state machine ignores it while hooks
    /// drive the status, so a hook status always wins. Codex dialogs only ever block; for the
    /// others a quiet screen with text on it means the agent waits for the owner.
    fn read_screen(&mut self) {
        self.screen_due = None;
        self.screen_read = Some(Instant::now());
        let (Some(kind), Some(agent)) = (self.screen_agent, self.agent.as_mut()) else {
            return;
        };
        let text = self.screen.text();
        let lines: Vec<&str> = text.lines().collect();
        let seen = match (heuristic::detect(kind, &lines), kind) {
            (Some(blocked @ Detected::Blocked(_)), _) => Some(blocked),
            (Some(_), ScreenAgent::Codex) => None,
            (Some(other), _) => Some(other),
            (None, ScreenAgent::Codex) => {
                (agent.status.status == Status::Blocked).then_some(Detected::Idle)
            }
            (None, _) => lines
                .iter()
                .any(|l| !l.trim().is_empty())
                .then_some(Detected::Idle),
        };
        if let Some(seen) = seen {
            agent.status.apply(Input::Screen(seen), now_ms());
            self.publish();
        }
    }

    fn exited(&mut self, code: Option<i32>) {
        if self.hibernating && !self.closing {
            return self.hibernated();
        }
        self.kill_at = None;
        self.retry = None;
        self.close_pending = None;
        self.wake = None;
        self.wake_due = None;
        mark_exited(&mut self.info, code);
        // An exited session takes no more hooks: its token stops working.
        self.info.agent.token_hash = None;
        // A shell that ended by itself may have been left a moment after its agent quit, before
        // the next poll: nothing is running in it, so Reopen has no agent to continue. A lost
        // holder (no exit code) keeps what was last seen.
        let forget = self.watch_foreground && code.is_some() && self.forget_foreground();
        let mut dropped = Vec::new();
        if let Some(agent) = self.agent.as_mut() {
            dropped = agent.asks.drain(..).map(|a| a.ask.ask_id).collect();
            agent.ending_at = None;
            let exit = ExitInfo { code, signal: None };
            agent.status.apply(Input::Exit(exit), now_ms());
            self.info.agent.asks.clear();
        }
        self.registry.update(self.info.clone());
        self.publish();
        if self.closing {
            self.registry.drop_files(&self.info.id);
        }
        if forget {
            self.send_meta();
        }
        self.announce_asks_dropped(dropped);
        let note = Notification::new(
            method::SESSION_EXITED,
            SessionExited {
                id: self.info.id.clone(),
                code,
            },
        );
        let _ = self.registry.events.send(note);
    }
}

/// Hibernation and wake.
impl Actor {
    /// An agent the daemon can resume, with a conversation and a stored launch.
    fn can_sleep(&self) -> bool {
        can_hibernate(
            &self.info,
            self.has_launch,
            agent::agent_of(self.info.agent.name.as_deref()).is_some(),
        )
    }

    fn candidate(&self) -> Candidate {
        let record = &self.info.agent;
        Candidate {
            eligible: true,
            running: self.running() && !self.hibernating && self.child.is_some(),
            agent_status: record.agent_status,
            open_asks: self.agent.as_ref().map_or(0, |a| a.asks.len()),
            pinned: self.registry.pinned(&self.info.id),
            focused: self.registry.focused(&self.info.id),
            quiet_since_ms: record.status_since.unwrap_or(0).max(self.last_input_ms),
        }
    }

    /// Once a second: a session that has been idle long enough goes to sleep.
    fn hibernate_tick(&mut self) {
        if !self.running() || !self.can_sleep() {
            return;
        }
        let Some(after) = self.registry.hibernate_after() else {
            return;
        };
        let threshold = u64::try_from(after.as_millis()).unwrap_or(u64::MAX);
        let now = now_ms();
        match decide(&self.candidate(), now, threshold, self.meter.busy(now)) {
            Decision::Keep => self.meter = BusyMeter::default(),
            Decision::Sample => self.sample(after),
            Decision::Hibernate => self.go_to_sleep(),
        }
    }

    /// Measures the CPU of the agent's children without waiting for the answer.
    fn sample(&mut self, after: Duration) {
        let every = (after / 3).clamp(Duration::from_millis(100), SAMPLE_EVERY);
        if self.sampling || self.sampled.is_some_and(|at| at.elapsed() < every) {
            return;
        }
        let Some(child) = self.child else {
            return;
        };
        self.sampling = true;
        self.sampled = Some(Instant::now());
        let me = self.me.clone();
        tokio::spawn(async move {
            let total = procs::sample(child).await;
            if let Some(tx) = me.upgrade() {
                let _ = tx.send(Cmd::Cpu(total)).await;
            }
        });
    }

    /// `session.hibernate`: sleep now, idle or not.
    fn hibernate_now(&mut self) -> Result<(), Fail> {
        if agent::agent_of(self.info.agent.name.as_deref()).is_none() {
            return Err((
                code::INVALID_PARAMS,
                "only claude and codex sessions hibernate".into(),
            ));
        }
        match self.info.status {
            SessionStatus::Hibernated => return Ok(()),
            SessionStatus::Running if self.hibernating => return Ok(()),
            SessionStatus::Running => {}
            _ => return Err(not_running()),
        }
        if self.info.agent.agent_session.is_none() {
            return Err((
                code::INVALID_PARAMS,
                "the session has no agent conversation yet".into(),
            ));
        }
        if !self.has_launch {
            return Err((
                code::WAKE_NEEDS_LAUNCH,
                "the session has no stored launch".into(),
            ));
        }
        self.go_to_sleep();
        Ok(())
    }

    /// Saves the screen, then asks the agent to end. Its exit completes the sleep.
    fn go_to_sleep(&mut self) {
        if self.holder.is_none() {
            return;
        }
        self.save_tail();
        self.hibernating = true;
        if let Some(holder) = self.holder.as_mut() {
            let _ = holder.close(Signal::Term);
        }
        self.kill_at = Some(Instant::now() + TERM_GRACE);
    }

    /// The agent's process ended because the session went to sleep.
    fn hibernated(&mut self) {
        self.hibernating = false;
        self.kill_at = None;
        self.retry = None;
        self.close_pending = None;
        self.attaching = None;
        for (_, pending) in self.pending.drain() {
            let (Pending::Ack(reply) | Pending::Resize(_, _, reply)) = pending;
            let _ = reply.send(Err(not_running()));
        }
        self.holder = None;
        self.holder_open = false;
        self.child = None;
        self.dead_holder = self.info.holder_pid;
        let mut dropped = Vec::new();
        if let Some(agent) = self.agent.as_mut() {
            dropped = agent.asks.drain(..).map(|a| a.ask.ask_id).collect();
            agent.ending_at = None;
        }
        mark_hibernated(&mut self.info, now_ms());
        // Dropped with its history: the saved tail shows it.
        self.screen = Screen::new(self.info.rows, self.info.cols);
        self.registry.update(self.info.clone());
        self.registry.announce_updated(&self.info.id);
        self.announce_asks_dropped(dropped);
        if !self.sleep_queue.is_empty() || self.wake_asked {
            // Someone typed or asked during the grace: wake at once; the queue goes along.
            self.wake_asked = false;
            let (registry, id, me) = (self.registry.clone(), self.info.id.clone(), self.me.clone());
            tokio::spawn(async move {
                if let Err((_, message)) = crate::wake::wake(&registry, &id).await {
                    tracing::warn!("session {id} cannot wake after its sleep: {message}");
                    if let Some(tx) = me.upgrade() {
                        let _ = tx.send(Cmd::SleepInputDropped).await;
                    }
                }
            });
        }
    }

    fn save_tail(&mut self) {
        self.tail_saved = Some(Instant::now());
        let snapshot = self.screen.snapshot();
        let budget = TAIL_MAX.saturating_sub(snapshot.len());
        let bytes = self.with_history_within(SCROLLBACK_LINES, snapshot, budget);
        let path = self.registry.home.tail_file(&self.info.id);
        if let Err(e) = write_private(&path, &bytes) {
            tracing::warn!("session {} cannot save its tail: {e}", self.info.id);
        }
    }

    /// A tail so a reboot has something to show; at most once a minute.
    fn save_tail_throttled(&mut self) {
        let lately = self.tail_saved.is_some_and(|at| at.elapsed() < TAIL_EVERY);
        if self.running() && !lately && self.can_sleep() {
            self.save_tail();
        }
    }

    /// Starts the agent again from its stored launch. The session keeps its id and stream.
    fn begin_wake(&mut self, prepared: Prepared) {
        if !matches!(
            self.info.status,
            SessionStatus::Hibernated | SessionStatus::Exited
        ) {
            return;
        }
        let now = now_ms();
        let dead = self.dead_holder.take().or(self.info.holder_pid);
        let record = &mut self.info.agent;
        record.token_hash = prepared.agent.token_hash;
        record.incarnation += 1;
        record.hibernated_at = None;
        record.asks.clear();
        record.agent_status = Some(sushiai_protocol::AgentStatus::Starting);
        record.status_source = Some(sushiai_protocol::StatusSource::Heuristic);
        record.status_since = Some(now);
        self.info.cmd = prepared.cmd;
        self.info.status = SessionStatus::Waking;
        self.info.exit_code = None;
        self.info.holder_pid = None;
        self.agent = restore_agent(&self.info);
        self.screen = Screen::new(self.info.rows, self.info.cols);
        self.meter = BusyMeter::default();
        self.closing = false;
        self.hibernating = false;
        // Clients clear their terminal; the new process then writes on a blank screen.
        let at = self.seq;
        self.seq += 2;
        let _ = self.output.send(Arc::new(Chunk {
            seq: at,
            data: b"\x1bc".to_vec(),
        }));
        self.base = self.seq;
        self.wake = Some(Wake {
            started: Instant::now(),
            queue: std::mem::take(&mut self.sleep_queue),
            last_output: None,
            hooked: false,
            resize: None,
        });
        self.wake_due = Some(Instant::now() + WAKE_CAP);
        self.registry.update(self.info.clone());
        self.registry.announce_updated(&self.info.id);

        let (me, sock, dir) = (
            self.me.clone(),
            self.sock.clone(),
            self.registry.home.sessions(),
        );
        let (id, cols, rows, cwd) = (
            self.info.id.clone(),
            self.info.cols,
            self.info.rows,
            self.info.cwd.clone(),
        );
        tokio::spawn(async move {
            holder_gone(&sock, dead).await;
            let started = start_holder(&holder::Spawn {
                id: &id,
                dir: &dir,
                cols,
                rows,
                cwd: &cwd,
                cmd: &prepared.run_cmd,
                env: &prepared.env,
            })
            .await;
            if let Some(tx) = me.upgrade() {
                let _ = tx.send(Cmd::Spawned(started)).await;
            }
        });
    }

    /// The new holder is up (or failed): attach to it from its first byte.
    fn spawned(&mut self, started: Result<(HolderConn, u32), Fail>) {
        if self.wake.is_none() || self.info.status != SessionStatus::Waking {
            if let Ok((_, pid)) = started {
                holder::kill_group(pid);
            }
            return;
        }
        let (mut conn, pid) = match started {
            Ok(started) => started,
            Err((_, message)) => {
                tracing::warn!("session {} cannot wake: {message}", self.info.id);
                return self.wake_failed();
            }
        };
        self.info.holder_pid = Some(pid);
        match conn.request(method::HOLD_ATTACH, HoldAttach { from_seq: 0 }) {
            Ok(id) => self.attaching = Some(id),
            Err(_) => {
                holder::kill_group(pid);
                return self.wake_failed();
            }
        }
        if let Some((cols, rows)) = self.wake.as_mut().and_then(|w| w.resize.take()) {
            let _ = conn.resize(cols, rows);
            self.screen.resize(rows, cols);
            self.info.cols = cols;
            self.info.rows = rows;
        }
        self.holder = Some(conn);
        self.holder_open = true;
        self.registry.update(self.info.clone());
        if self.wake.as_ref().is_some_and(|w| w.hooked) {
            self.finish_wake();
        }
    }

    /// The process could not start: back to sleep, so a later wake can try again.
    fn wake_failed(&mut self) {
        if self.closing {
            return self.exited(None);
        }
        if self.wake.as_ref().is_some_and(|w| !w.queue.is_empty()) {
            tracing::warn!(
                "session {} could not wake: the input typed meanwhile is dropped",
                self.info.id
            );
        }
        self.wake = None;
        self.wake_due = None;
        self.holder = None;
        self.holder_open = false;
        self.attaching = None;
        self.close_pending = None;
        mark_hibernated(&mut self.info, now_ms());
        self.screen = Screen::new(self.info.rows, self.info.cols);
        self.registry.update(self.info.clone());
        self.registry.announce_updated(&self.info.id);
    }

    /// The wake timer fired: ready after quiet output, or give up at the cap.
    fn wake_check(&mut self) {
        self.wake_due = None;
        let Some(wake) = &self.wake else {
            return;
        };
        let quiet = wake
            .last_output
            .is_some_and(|at| at.elapsed() >= WAKE_QUIET);
        if wake.started.elapsed() >= WAKE_CAP {
            if self.holder.is_some() {
                self.finish_wake();
            } else {
                self.wake_failed();
            }
        } else if quiet {
            self.finish_wake();
        } else {
            self.wake_due = Some(wake.started + WAKE_CAP);
        }
    }

    /// The agent is up: the input typed meanwhile goes in, in order.
    fn finish_wake(&mut self) {
        if self.holder.is_none() {
            // The queue needs the holder: finish once `spawned` has connected it.
            if let Some(wake) = self.wake.as_mut() {
                wake.hooked = true;
            }
            return;
        }
        let Some(wake) = self.wake.take() else {
            return;
        };
        self.wake_due = None;
        self.info.status = SessionStatus::Running;
        // A resumed agent may send no hook until the first prompt: it is idle, not starting.
        if let Some(agent) = self.agent.as_mut() {
            agent.status.ready(now_ms());
        }
        self.publish();
        if let (Some(holder), false) = (self.holder.as_mut(), wake.queue.is_empty()) {
            let _ = holder.input(wake.queue);
        }
        self.registry.update(self.info.clone());
        self.registry.announce_updated(&self.info.id);
    }

    fn announce_asks_dropped(&self, dropped: Vec<String>) {
        for ask_id in dropped {
            let params = AskClosed {
                ask_id,
                decided: false,
            };
            let _ = self
                .registry
                .events
                .send(Notification::new(method::SESSION_ASK_CLOSED, params));
        }
    }
}

async fn start_holder(spec: &holder::Spawn<'_>) -> Result<(HolderConn, u32), Fail> {
    let fail = |e: &dyn std::fmt::Display| (code::SPAWN_FAILED, e.to_string());
    let pid = holder::spawn(spec).await.map_err(|e| fail(&e))?;
    let sock = spec.dir.join(format!("{}.sock", spec.id));
    match HolderConn::connect(&sock).await {
        Ok(conn) => Ok((conn, pid)),
        Err(e) => {
            holder::kill_group(pid);
            let _ = std::fs::remove_file(&sock);
            Err(fail(&e))
        }
    }
}

/// Writes `bytes` to `path` (0600) through a temp file, so a reader never sees half of it.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(bytes)?;
    std::fs::rename(&tmp, path)
}

/// The next frame of the holder; never ready while there is none.
async fn next_frame(holder: &mut Option<HolderConn>) -> crate::Result<Option<Frame>> {
    match holder {
        Some(holder) => holder.reader.next().await,
        None => pending().await,
    }
}

fn screen_text(screen: &mut Screen, scrollback: usize) -> Text {
    let (rows, cols) = screen.size();
    let mut text = screen.history_text(scrollback);
    text.push_str(&screen.text());
    text.truncate(text.trim_end().len());
    Text { text, rows, cols }
}

/// Waits until the process `pid` is gone or its socket is: only then may a new holder bind
/// the same socket path (the old one removes it as its last step).
async fn holder_gone(sock: &std::path::Path, pid: Option<u32>) {
    let alive = |pid: u32| {
        i32::try_from(pid).is_ok_and(|p| {
            // SAFETY: kill(2) with signal 0 only checks that the process exists.
            unsafe { libc::kill(p, 0) == 0 }
        })
    };
    let deadline = Instant::now() + HOLDER_GONE_WAIT;
    while sock.exists() && pid.is_some_and(alive) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Sleeps until `deadline`, or forever when there is none.
async fn until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => sleep_until(at).await,
        None => pending::<()>().await,
    }
}
