//! One actor per session. It owns the screen, the stream offset and the holder connection.
//! Everything else talks to it through a bounded command queue and a broadcast of output.
//! The actor never awaits a socket write, so it always keeps reading the holder.

use std::collections::HashMap;
use std::future::pending;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sushiai_agents::hooks::{self, HookEvent};
use sushiai_agents::status::{ExitInfo, Input, SessionStatus as AgentState};
use sushiai_core::{mark_exited, Screen};
use sushiai_protocol::{
    code, method, Ask, AskClosed, AskRespond, Frame, HoldAttach, HoldAttachResult, HoldExited,
    Message, Notification, SessionExited, SessionInfo, SessionMeta, SessionStatus,
    SessionStatusChanged, Signal,
};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{interval, sleep_until, timeout, Instant};

use crate::agent::{self, now_ms, random_hex};
use crate::error::Fail;
use crate::holder::{HolderConn, SendError};
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
/// Open permission asks per session; more are left to the agent's own terminal prompt.
const MAX_ASKS: usize = 20;

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
    Attach(oneshot::Sender<Attached>),
    Snapshot(oneshot::Sender<Snap>),
    Hook(Vec<u8>, oneshot::Sender<HookReply>),
    Respond(AskRespond, Reply),
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

    pub async fn attach(&self) -> Option<Attached> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(Cmd::Attach(tx)).await.ok()?;
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

/// What the actor tracks for a session whose agent sends hooks.
struct Agent {
    status: AgentState,
    /// Stamped on each received hook; the state machine drops stale ones.
    seq: u64,
    last_hook: Instant,
    ending_at: Option<Instant>,
    asks: Vec<OpenAsk>,
}

struct Actor {
    agent: Option<Agent>,
    info: SessionInfo,
    screen: Screen,
    seq: u64,
    holder: HolderConn,
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
}

/// Starts the actor for a session whose holder is attached and replayed up to `seq`.
pub fn start(
    registry: Arc<Registry>,
    info: SessionInfo,
    holder: HolderConn,
    screen: Screen,
    seq: u64,
    sock: PathBuf,
) -> Handle {
    let (tx, rx) = mpsc::channel(64);
    let handle = Handle { tx };
    let mut info = info;
    info.holder_pid = holder.holder_pid.or(info.holder_pid);
    registry.update(info.clone());
    registry.set_handle(&info.id, handle.clone());
    let actor = Actor {
        agent: restore_agent(&info),
        info,
        screen,
        seq,
        holder,
        holder_open: true,
        pending: HashMap::new(),
        output: broadcast::channel(OUTPUT_BACKLOG).0,
        registry,
        kill_at: None,
        sock,
        retry: None,
        attaching: None,
        close_pending: None,
    };
    tokio::spawn(actor.run(rx));
    handle
}

/// The state machine for a hook-capable agent, resumed from the persisted record. A `blocked`
/// status stays blocked: the agent may still show its dialog. The ask itself died with the
/// previous daemon, so the machine is told the ask went back to the terminal.
fn restore_agent(info: &SessionInfo) -> Option<Agent> {
    agent::agent_of(info.agent.name.as_deref())?;
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
                frame = self.holder.reader.next(), if self.holder_open => {
                    self.holder_frame(frame.ok().flatten());
                }
                () = until(self.kill_at) => {
                    self.kill_at = None;
                    // Fire and forget: the answer is not tracked.
                    let _ = self.holder.close(Signal::Kill);
                }
                () = until(self.retry.map(|(at, _)| at)) => self.reconnect().await,
                _ = tick.tick() => self.sweep(),
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

    /// Sends a request to the holder and parks `pending` until its answer arrives.
    /// `build` gets the holder connection and returns the request id.
    fn track(
        &mut self,
        reply: Reply,
        build: impl FnOnce(&mut HolderConn) -> Result<u64, SendError>,
        pending: impl FnOnce(Reply) -> Pending,
    ) {
        if !self.running() {
            let _ = reply.send(Err(not_running()));
        } else if self.pending.len() >= MAX_PENDING {
            let _ = reply.send(Err((code::INPUT_BACKPRESSURE, "too many requests".into())));
        } else {
            match build(&mut self.holder) {
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
            Cmd::Input(data, reply) => {
                self.track(reply, |h| h.input(data), Pending::Ack);
            }
            Cmd::Resize(cols, rows, reply) => {
                self.track(
                    reply,
                    |h| h.resize(cols, rows),
                    |r| Pending::Resize(cols, rows, r),
                );
            }
            Cmd::Close(graceful, reply) if self.info.status == SessionStatus::Detached => {
                // Not connected to the holder right now: close as soon as it is back, or end
                // as exited if it turns out to be gone.
                self.close_pending = Some(graceful);
                let _ = reply.send(Ok(()));
            }
            Cmd::Close(graceful, reply) => {
                let signal = if graceful { Signal::Term } else { Signal::Kill };
                self.track(reply, |h| h.close(signal), Pending::Ack);
                if graceful && self.running() {
                    self.kill_at = Some(Instant::now() + TERM_GRACE);
                }
            }
            Cmd::Attach(reply) => {
                let _ = reply.send(Attached {
                    output: self.output.subscribe(),
                    snap: self.snap(),
                });
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
        }
    }

    fn hook(&mut self, payload: &[u8]) -> HookReply {
        let no_agent = || {
            (
                code::INVALID_PARAMS,
                "session has no agent hooks".to_string(),
            )
        };
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
            let params = SessionMeta {
                id,
                agent_session: self.info.agent.agent_session.clone(),
                transcript_path: self.info.agent.transcript_path.clone(),
            };
            let _ = self
                .registry
                .events
                .send(Notification::new(method::SESSION_META, params));
        }
    }

    fn snap(&self) -> Snap {
        let (rows, cols) = self.screen.size();
        Snap {
            snapshot: self.screen.snapshot(),
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
        if self.info.status != SessionStatus::Exited {
            self.info.status = SessionStatus::Detached;
            self.registry.update(self.info.clone());
            self.retry = Some((Instant::now() + RECONNECT_FIRST, RECONNECT_FIRST));
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
                let from_seq = self.seq;
                if let Ok(id) = conn.request(method::HOLD_ATTACH, HoldAttach { from_seq }) {
                    self.holder = conn;
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
                self.info.status = SessionStatus::Running;
                self.info.holder_pid = Some(attached.pid).filter(|p| *p != 0);
                self.registry.update(self.info.clone());
                if let Some(graceful) = self.close_pending.take() {
                    let signal = if graceful { Signal::Term } else { Signal::Kill };
                    let _ = self.holder.close(signal);
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
        self.seq = seq + data.len() as u64;
        // No receivers is normal when nobody is attached.
        let _ = self.output.send(Arc::new(Chunk { seq, data }));
    }

    fn exited(&mut self, code: Option<i32>) {
        self.kill_at = None;
        self.retry = None;
        self.close_pending = None;
        mark_exited(&mut self.info, code);
        // An exited session takes no more hooks: its token stops working.
        self.info.agent.token_hash = None;
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

/// Sleeps until `deadline`, or forever when there is none.
async fn until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => sleep_until(at).await,
        None => pending::<()>().await,
    }
}
