//! Client connections: framing, `hello`, method dispatch, attach streams.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sushiai_core::Screen;
use sushiai_protocol::catalog::{ProjectsSync, SessionUpdate};
use sushiai_protocol::{
    code, encode, method, AskRespond, AttachResult, DaemonConfigure, Frame, Hello, HelloResult,
    HookEvent, HookOpen, HookOpenResult, HookResult, Message, Notification, ReadResult, Request,
    Response, SessionAttach, SessionClose, SessionCreate, SessionFocus, SessionId, SessionInfo,
    SessionInput, SessionOpen, SessionRead, SessionResize, SessionSnapshot, SessionStatus,
    SessionWake, CAPABILITIES, PROTOCOL_VERSION,
};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

use crate::agent;
use crate::error::Fail;
use crate::framed::FrameReader;
use crate::holder::{self, HolderConn};
use crate::launch_store::Launch;
use crate::registry::{Registry, RemoveError};
use crate::session::{self, Chunk, Handle, Snap};
use crate::wake;

type Outbox = mpsc::Sender<Vec<u8>>;

const OUTBOX: usize = 256;
/// After the client's EOF: how long queued responses may take to reach it.
const WRITER_GRACE: Duration = Duration::from_secs(2);
/// How long a request waits for a restored session's holder: the reattach limit plus slack.
const RECOVERY_WAIT: Duration = Duration::from_secs(12);

pub async fn serve(listener: UnixListener, registry: Arc<Registry>) {
    let mut next_conn = 0u64;
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                next_conn += 1;
                tokio::spawn(connection(stream, registry.clone(), next_conn));
            }
            Err(e) => {
                tracing::warn!("accept failed: {e}");
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

/// Requests that change something: refused once the daemon is stopping, so nothing is lost
/// between the final state flush and the exit.
const CHANGES: &[&str] = &[
    method::SESSION_CREATE,
    method::SESSION_INPUT,
    method::SESSION_RESIZE,
    method::SESSION_CLOSE,
    method::SESSION_UPDATE,
    method::SESSION_REMOVE,
    method::SESSION_WAKE,
    method::SESSION_HIBERNATE,
    method::DAEMON_CONFIGURE,
    method::PROJECTS_SYNC,
    method::GROUPS_SYNC,
];

struct Conn {
    registry: Arc<Registry>,
    /// Which connection this is: what `session.focus` is kept under.
    id: u64,
    out: Outbox,
    said_hello: bool,
    /// A hook process: it may call `hook.*` only and receives no notifications.
    hook_only: bool,
    streams: HashMap<String, JoinHandle<()>>,
    events: Option<JoinHandle<()>>,
    /// An attach whose output stream starts once its response is queued.
    starting: Option<Stream>,
}

struct Stream {
    id: String,
    handle: Handle,
    output: broadcast::Receiver<Arc<Chunk>>,
    floor: u64,
}

async fn connection(stream: UnixStream, registry: Arc<Registry>, id: u64) {
    let (read, mut write) = stream.into_split();
    let (out, mut queue) = mpsc::channel::<Vec<u8>>(OUTBOX);
    let mut writer = tokio::spawn(async move {
        while let Some(bytes) = queue.recv().await {
            if write.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });
    let focus = registry.clone();
    let mut conn = Conn {
        registry,
        id,
        out,
        said_hello: false,
        hook_only: false,
        streams: HashMap::new(),
        events: None,
        starting: None,
    };
    let mut reader = FrameReader::new(read);
    while let Ok(Some(frame)) = reader.next().await {
        let Frame::Json(text) = frame else { continue };
        match Message::parse(&text) {
            Ok(Message::Request(request)) => {
                let response = if request.method == method::HOOK_EVENT {
                    // A hook may wait for the owner's answer. If its process goes away
                    // meanwhile, stop waiting: the actor then closes the ask.
                    let handling = conn.handle(&request);
                    tokio::pin!(handling);
                    loop {
                        tokio::select! {
                            response = &mut handling => break Some(response),
                            frame = reader.next() => {
                                if !matches!(frame, Ok(Some(_))) {
                                    break None;
                                }
                            }
                        }
                    }
                } else {
                    Some(conn.handle(&request).await)
                };
                let Some(response) = response else { break };
                if conn.out.send(encode(&response.frame())).await.is_err() {
                    break;
                }
                // The output stream of an attach starts only after its response is queued.
                conn.start_stream();
            }
            Ok(_) => {}
            Err(e) => {
                let response = Response::err(Value::Null, code::PARSE_ERROR, e.to_string());
                let _ = conn.out.send(encode(&response.frame())).await;
            }
        }
    }
    // Whatever the client looked at is no longer looked at.
    focus.clear_focus(id);
    // The client is gone or has half-closed. Responses already queued still go out: stop the
    // tasks that hold a sender, drop ours, and let the writer drain its queue (bounded).
    let Conn {
        out,
        streams,
        events,
        ..
    } = conn;
    let tasks: Vec<JoinHandle<()>> = streams.into_values().chain(events).collect();
    for task in &tasks {
        task.abort();
    }
    for task in tasks {
        let _ = task.await;
    }
    drop(out);
    if timeout(WRITER_GRACE, &mut writer).await.is_err() {
        writer.abort();
    }
}

/// Parses the params. A bad request never echoes what the caller sent (params can hold
/// secrets such as `claudeSettings`): the message names the method and, for a missing or
/// unknown field, the field name, and nothing else.
fn params<T: DeserializeOwned>(request: &Request) -> Result<T, Fail> {
    serde_json::from_value(request.params.clone()).map_err(|e| {
        let text = e.to_string();
        let field = ["missing field `", "unknown field `"]
            .iter()
            .find_map(|lead| text.strip_prefix(lead))
            .and_then(|rest| rest.split('`').next())
            .filter(|name| name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        let message = match field {
            Some(name) => format!("invalid {} params: field {name}", request.method),
            None => format!("invalid {} params", request.method),
        };
        (code::INVALID_PARAMS, message)
    })
}

impl Conn {
    async fn handle(&mut self, request: &Request) -> Response {
        match self.dispatch(request).await {
            Ok(result) => Response::ok(request.id.clone(), result),
            Err((code, message)) => Response::err(request.id.clone(), code, message),
        }
    }

    /// The session's handle. A session without an actor yet is waited for (at most
    /// `RECOVERY_WAIT`): a restored one, so a client that reconnects at once can attach, type,
    /// resize and close, and a new one, so its child's first `hook.*` call is not refused while
    /// `session.create` attaches. After a timeout or an exit the normal error is returned.
    async fn session(&self, id: &str) -> Result<Handle, Fail> {
        let deadline = tokio::time::Instant::now() + RECOVERY_WAIT;
        loop {
            // Subscribe before looking, so a settle between the look and the wait is not lost.
            let settled = self.registry.settled().notified();
            tokio::pin!(settled);
            settled.as_mut().enable();
            match self.registry.handle(id) {
                Some(handle) => return Ok(handle),
                None if self.registry.awaiting_actor(id) => {}
                None if self.registry.known(id) => {
                    return Err((code::SESSION_NOT_RUNNING, "session is not running".into()))
                }
                None => return Err((code::SESSION_NOT_FOUND, format!("no session {id}"))),
            }
            if tokio::time::timeout_at(deadline, settled).await.is_err() {
                return Err((code::SESSION_NOT_RUNNING, "session is not running".into()));
            }
        }
    }

    fn start_stream(&mut self) {
        let Some(Stream {
            id,
            handle,
            output,
            floor,
        }) = self.starting.take()
        else {
            return;
        };
        let task = tokio::spawn(stream_output(
            id.clone(),
            handle,
            output,
            floor,
            self.out.clone(),
        ));
        if let Some(old) = self.streams.insert(id, task) {
            old.abort();
        }
    }

    async fn dispatch(&mut self, request: &Request) -> Result<Value, Fail> {
        if request.method == method::HELLO {
            return self.hello(request);
        }
        if request.method == method::PING {
            return Ok(json!({}));
        }
        if !self.said_hello {
            return Err((code::NOT_INITIALIZED, "send hello first".into()));
        }
        if self.hook_only && !request.method.starts_with("hook.") {
            return Err((
                code::UNAUTHORIZED,
                "hook connections may only call hook.*".into(),
            ));
        }
        if let Some((namespace, rest)) = request.method.split_once('.') {
            if let Some(module) = self.registry.module(namespace) {
                if self.registry.stopping() {
                    return Err((code::SHUTTING_DOWN, "the daemon is stopping".into()));
                }
                return call_module(module, rest, request.params.clone()).await;
            }
        }
        if self.registry.stopping() && CHANGES.contains(&request.method.as_str()) {
            return Err((code::SHUTTING_DOWN, "the daemon is stopping".into()));
        }
        match request.method.as_str() {
            method::HOOK_EVENT => self.hook(params(request)?).await,
            method::HOOK_OPEN => self.open(params(request)?).await,
            method::SESSION_READ => {
                let p: SessionRead = params(request)?;
                let text = self
                    .session(&p.id)
                    .await?
                    .read(scrollback(p.scrollback))
                    .await
                    .ok_or_else(|| (code::INTERNAL, "session actor is gone".to_string()))?;
                Ok(json!(ReadResult {
                    text: text.text,
                    rows: text.rows,
                    cols: text.cols,
                }))
            }
            method::ASK_RESPOND => {
                let p: AskRespond = params(request)?;
                let id = self
                    .registry
                    .session_of_ask(&p.ask_id)
                    .ok_or_else(|| (code::ASK_NOT_FOUND, format!("no ask {}", p.ask_id)))?;
                self.session(&id).await?.respond(p).await?;
                Ok(json!({}))
            }
            method::SESSION_CREATE => create(&self.registry, params(request)?).await,
            method::SESSION_LIST => Ok(json!(self.registry.list())),
            method::SESSION_INPUT => {
                let p: SessionInput = params(request)?;
                // Typing into a sleeping session wakes it; the bytes wait for the agent.
                if self.registry.info(&p.id).map(|i| i.status) == Some(SessionStatus::Hibernated) {
                    wake::wake(&self.registry, &p.id).await?;
                }
                self.session(&p.id)
                    .await?
                    .input(p.data.into_bytes())
                    .await?;
                Ok(json!({}))
            }
            method::SESSION_RESIZE => {
                let p: SessionResize = params(request)?;
                check_size(p.cols, p.rows)?;
                self.session(&p.id).await?.resize(p.cols, p.rows).await?;
                Ok(json!({}))
            }
            method::SESSION_CLOSE => {
                let p: SessionClose = params(request)?;
                self.session(&p.id).await?.close(p.graceful).await?;
                Ok(json!({}))
            }
            method::SESSION_WAKE => {
                let p: SessionWake = params(request)?;
                wake::wake(&self.registry, &p.id).await?;
                Ok(json!({}))
            }
            method::SESSION_HIBERNATE => {
                let p: SessionId = params(request)?;
                self.session(&p.id).await?.hibernate().await?;
                Ok(json!({}))
            }
            method::SESSION_FOCUS => {
                let p: SessionFocus = params(request)?;
                if !self.registry.known(&p.id) {
                    return Err((code::SESSION_NOT_FOUND, format!("no session {}", p.id)));
                }
                self.registry.set_focus(self.id, &p.id, p.focused);
                Ok(json!({}))
            }
            method::DAEMON_CONFIGURE => {
                let p: DaemonConfigure = params(request)?;
                let registry = self.registry.clone();
                tokio::task::spawn_blocking(move || registry.configure(p.hibernate_after_secs))
                    .await
                    .map_err(|e| (code::INTERNAL, e.to_string()))?
                    .map_err(|e| (code::INTERNAL, format!("cannot save the settings: {e}")))?;
                Ok(json!({}))
            }
            method::PROJECTS_SYNC => {
                let p: ProjectsSync = params(request)?;
                if p.host != self.registry.host {
                    let message = format!("daemon is {}", self.registry.host);
                    return Err((code::INVALID_PARAMS, message));
                }
                Ok(json!(self.registry.sync_projects(&p)))
            }
            method::CATALOG_GET => Ok(json!(self.registry.catalog_snapshot())),
            method::GROUPS_SYNC => Ok(json!(self.registry.sync_groups(&params(request)?))),
            method::SESSION_UPDATE => {
                let p: SessionUpdate = params(request)?;
                match self.registry.update_session(p) {
                    Some(info) => Ok(json!(info)),
                    None => Err((code::SESSION_NOT_FOUND, "no such session".into())),
                }
            }
            method::SESSION_REMOVE => {
                let p: SessionId = params(request)?;
                match self.registry.remove(&p.id) {
                    Ok(()) => Ok(json!({})),
                    Err(RemoveError::NotFound) => {
                        Err((code::SESSION_NOT_FOUND, format!("no session {}", p.id)))
                    }
                    Err(RemoveError::StillRunning) => Err((
                        code::SESSION_STILL_RUNNING,
                        "only an exited session can be removed".into(),
                    )),
                }
            }
            method::DAEMON_SHUTDOWN => {
                self.registry.request_stop();
                Ok(json!({}))
            }
            method::SESSION_ATTACH => self.attach(params(request)?).await,
            method::SESSION_DETACH => {
                let p: SessionId = params(request)?;
                if let Some(task) = self.streams.remove(&p.id) {
                    task.abort();
                }
                Ok(json!({}))
            }
            other => Err((code::METHOD_NOT_FOUND, format!("unknown method {other}"))),
        }
    }

    fn hello(&mut self, request: &Request) -> Result<Value, Fail> {
        let hello: Hello = params(request)?;
        if hello.protocol != PROTOCOL_VERSION {
            let message = format!(
                "protocol {} is not supported (daemon speaks {PROTOCOL_VERSION})",
                hello.protocol
            );
            return Err((code::PROTOCOL_MISMATCH, message));
        }
        if !self.said_hello {
            self.said_hello = true;
            self.hook_only = hello.role.as_deref() == Some("hook");
            if !self.hook_only {
                self.events = Some(tokio::spawn(forward_events(
                    self.registry.events.subscribe(),
                    self.out.clone(),
                )));
            }
        }
        Ok(json!(HelloResult {
            protocol: PROTOCOL_VERSION,
            capabilities: CAPABILITIES
                .iter()
                .copied()
                .chain(self.registry.modules().iter().map(|m| m.capability()))
                .map(str::to_string)
                .collect(),
            daemon: env!("CARGO_PKG_VERSION").into(),
            host: self.registry.host.clone(),
            build: own_build().map(str::to_string),
        }))
    }

    async fn hook(&self, p: HookEvent) -> Result<Value, Fail> {
        if !self.hook_only {
            return Err((
                code::UNAUTHORIZED,
                "hook.event needs a connection that said hello with role hook".into(),
            ));
        }
        // A wrong token, an unknown session and an exited one (its token is cleared) look
        // the same.
        if !self.registry.token_matches(&p.session, &p.token) {
            return Err((code::UNAUTHORIZED, "invalid session token".into()));
        }
        // A nested agent inherits the variables of its parent: only the session's own agent
        // may report.
        if self.registry.agent_of(&p.session).as_deref() != Some(p.agent.as_str()) {
            return Err((code::UNAUTHORIZED, "hook is from another agent".into()));
        }
        // The session is known from `session.create` on; `session` waits for a starting actor.
        let handle = self.session(&p.session).await?;
        let waiting = handle
            .hook(serde_json::to_vec(&p.payload).unwrap_or_default())
            .await?;
        let answer = match waiting {
            Some(rx) => rx.await.ok(),
            None => None,
        };
        Ok(json!(HookResult { answer }))
    }

    /// `hook.open`: an agent's `sushiai open` asks the desktop to open `arg` with `target`.
    async fn open(&self, p: HookOpen) -> Result<Value, Fail> {
        if !self.hook_only {
            return Err((
                code::UNAUTHORIZED,
                "hook.open needs a connection that said hello with role hook".into(),
            ));
        }
        if !self.registry.token_matches(&p.session, &p.token) {
            return Err((code::UNAUTHORIZED, "invalid session token".into()));
        }
        // A restored session whose holder has not answered yet is waited for.
        self.session(&p.session).await?;
        if !valid_target(&p.target) {
            return Err((
                code::INVALID_PARAMS,
                "target must look like extension/surface (a-z, 0-9, -)".into(),
            ));
        }
        if !p.arg.starts_with('/') || p.arg.len() > 4096 || p.arg.chars().any(char::is_control) {
            return Err((
                code::INVALID_PARAMS,
                "arg must be an absolute path of at most 4096 bytes without control characters"
                    .into(),
            ));
        }
        let nonce = agent::random_hex(8).map_err(|e| (code::INTERNAL, e.to_string()))?;
        let note = SessionOpen {
            id: p.session,
            target: p.target,
            arg: p.arg,
            nonce: nonce.clone(),
        };
        // No subscriber is fine: the request is then simply not seen.
        let _ = self
            .registry
            .events
            .send(Notification::new(method::SESSION_OPEN, note));
        Ok(json!(HookOpenResult { nonce }))
    }

    async fn attach(&mut self, p: SessionAttach) -> Result<Value, Fail> {
        let handle = self.session(&p.id).await?;
        let attached = handle
            .attach(scrollback(p.scrollback))
            .await
            .ok_or_else(|| (code::INTERNAL, "session actor is gone".to_string()))?;
        if let Some(old) = self.streams.remove(&p.id) {
            old.abort();
        }
        self.starting = Some(Stream {
            id: p.id,
            handle,
            output: attached.output,
            floor: attached.snap.seq,
        });
        Ok(json!(attach_result(attached.snap)))
    }
}

/// Runs a module call on its own task: a panic (in the call or its future) answers INTERNAL
/// for this request only.
async fn call_module(
    module: Arc<dyn crate::module::Module>,
    method: &str,
    params: Value,
) -> Result<Value, Fail> {
    let method = method.to_string();
    match tokio::spawn(async move { module.call(&method, params).await }).await {
        Ok(reply) => reply,
        Err(e) => Err((code::INTERNAL, format!("module call failed: {e}"))),
    }
}

/// `^[a-z0-9-]+/[a-z0-9-]+$`.
fn valid_target(target: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && p.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    target
        .split_once('/')
        .is_some_and(|(a, b)| part(a) && part(b))
}

/// Most history lines a client may ask for (the screen model keeps 2000).
fn scrollback(requested: Option<u32>) -> usize {
    (requested.unwrap_or(0) as usize).min(sushiai_core::SCROLLBACK_LINES)
}

fn attach_result(snap: Snap) -> AttachResult {
    AttachResult {
        snapshot: snap.snapshot,
        seq: snap.seq,
        cols: snap.cols,
        rows: snap.rows,
    }
}

async fn forward_events(mut events: broadcast::Receiver<Notification>, out: Outbox) {
    loop {
        match events.recv().await {
            Ok(note) => {
                if out.send(encode(&note.frame())).await.is_err() {
                    break;
                }
            }
            // Missed events: the client lists the sessions again. The note carries no list.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let note = Notification::new(method::SESSION_RESYNC, json!({}));
                if out.send(encode(&note.frame())).await.is_err() {
                    break;
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// Sends a session's output to one client. A client that falls behind, or that sees a gap in
/// the stream (the holder's ring no longer covered what the daemon missed), gets a fresh
/// snapshot instead; the session never waits for it.
async fn stream_output(
    id: String,
    handle: Handle,
    mut output: broadcast::Receiver<Arc<Chunk>>,
    first: u64,
    out: Outbox,
) {
    // Offset of the next byte this client expects.
    let mut next = first;
    loop {
        let resync = match output.recv().await {
            Ok(chunk) => {
                let end = chunk.seq + chunk.data.len() as u64;
                if end <= next {
                    continue;
                }
                if chunk.seq <= next {
                    let frame = Frame::Output {
                        id: id.clone(),
                        seq: chunk.seq,
                        data: chunk.data.clone(),
                    };
                    if out.send(encode(&frame)).await.is_err() {
                        break;
                    }
                    next = end;
                    continue;
                }
                true
            }
            Err(broadcast::error::RecvError::Lagged(_)) => true,
            Err(broadcast::error::RecvError::Closed) => break,
        };
        if resync {
            let Some(snap) = handle.snapshot().await else {
                break;
            };
            next = snap.seq;
            let params = SessionSnapshot {
                id: id.clone(),
                attach: attach_result(snap),
            };
            let note = Notification::new(method::SESSION_SNAPSHOT, params);
            if out.send(encode(&note.frame())).await.is_err() {
                break;
            }
        }
    }
}

async fn create(registry: &Arc<Registry>, p: SessionCreate) -> Result<Value, Fail> {
    let Some(key) = p.idempotency_key.clone() else {
        return create_new(registry, p, None).await;
    };
    if key.is_empty() || key.len() > 256 {
        return Err((
            code::INVALID_PARAMS,
            "idempotencyKey must be 1 to 256 bytes".into(),
        ));
    }
    // One keyed launch at a time, so two requests with one key cannot both start a session.
    let _one_at_a_time = registry.keyed_create.lock().await;
    // The stop may have been requested while this launch waited its turn.
    if registry.stopping() {
        return Err((code::SHUTTING_DOWN, "the daemon is stopping".into()));
    }
    keyed_create(registry, p, &key).await
}

async fn keyed_create(
    registry: &Arc<Registry>,
    p: SessionCreate,
    key: &str,
) -> Result<Value, Fail> {
    if let Some(id) = registry.by_key(key) {
        return Ok(json!({ "id": id }));
    }
    create_new(registry, p, Some(key.to_string())).await
}

/// A terminal grid is `cols x rows` cells of memory in the daemon and the holder, so both
/// sides are bounded.
fn check_size(cols: u16, rows: u16) -> Result<(), Fail> {
    if (1..=1000).contains(&cols) && (1..=500).contains(&rows) {
        return Ok(());
    }
    Err((
        code::INVALID_PARAMS,
        "cols must be 1 to 1000 and rows 1 to 500".into(),
    ))
}

async fn create_new(
    registry: &Arc<Registry>,
    p: SessionCreate,
    key: Option<String>,
) -> Result<Value, Fail> {
    check_size(p.cols, p.rows)?;
    let spawn_failed = |e: &dyn std::fmt::Display| (code::SPAWN_FAILED, e.to_string());
    let id = agent::random_hex(8).map_err(|e| spawn_failed(&e))?;
    let socket = registry.home.socket().to_string_lossy().into_owned();
    let binding = registry.bind_for_create(p.project.clone(), p.group.clone(), &p.cwd);
    let mut prepared = agent::prepare(&p, &socket, &id)?;
    let (home_dir, request) = (registry.home.dir().to_path_buf(), p.clone());
    tokio::task::spawn_blocking(move || agent::ensure_codex_home(&request, &home_dir))
        .await
        .map_err(|e| (code::INTERNAL, e.to_string()))??;
    prepared.agent.idempotency_key = key;
    // What a wake needs, sealed before the child runs. A session that cannot store it still
    // runs; it just never sleeps.
    if agent::agent_of(p.agent.as_deref()).is_some() {
        let (store, key, launch) = (registry.clone(), id.clone(), Launch::of(&p));
        let sealed = tokio::task::spawn_blocking(move || store.launches().seal(&key, &launch))
            .await
            .map_err(|e| (code::INTERNAL, e.to_string()))?;
        if let Err(e) = sealed {
            tracing::warn!("session {id} cannot store its launch: {e}");
        }
    }
    let info = SessionInfo {
        id: id.clone(),
        cmd: prepared.cmd.clone(),
        cwd: p.cwd.clone(),
        title: p.title.clone(),
        status: SessionStatus::Running,
        exit_code: None,
        holder_pid: None,
        cols: p.cols,
        rows: p.rows,
        project: binding.project,
        group: binding.group,
        agent: prepared.agent,
    };
    // Known before the child runs: its first hook may arrive before the actor does.
    registry.update(info.clone());
    let dir = registry.home.sessions();
    let spawned = holder::spawn(&holder::Spawn {
        id: &id,
        dir: &dir,
        cols: p.cols,
        rows: p.rows,
        cwd: &p.cwd,
        cmd: &prepared.run_cmd,
        env: &prepared.env,
    })
    .await;
    let pid = match spawned {
        Ok(pid) => pid,
        Err(e) => {
            registry.forget(&id);
            registry.drop_files(&id);
            return Err(spawn_failed(&e));
        }
    };
    // The holder is up: its socket exists. If attaching fails, take the holder down again.
    let sock = dir.join(format!("{id}.sock"));
    let mut screen = Screen::new(p.rows, p.cols);
    let attached = async {
        let mut conn = HolderConn::connect(&sock).await?;
        let seq = conn.attach_and_replay(&mut screen).await?;
        Ok::<_, crate::Error>((conn, seq))
    }
    .await;
    let (conn, seq) = match attached {
        Ok(attached) => attached,
        Err(e) => {
            holder::kill_group(pid);
            let _ = std::fs::remove_file(&sock);
            registry.forget(&id);
            registry.drop_files(&id);
            return Err(spawn_failed(&e));
        }
    };
    // Announced inside `start`, before the actor can broadcast anything about the session.
    session::start(registry.clone(), info, conn, screen, seq, sock, true);
    // The answer promises a session that outlives a daemon crash: its state is on disk first.
    let flushing = registry.clone();
    let _ = tokio::task::spawn_blocking(move || flushing.flush()).await;
    Ok(json!({ "id": id }))
}

/// sha256 (hex) of the running binary, measured once: what `hello.build` reports.
pub(crate) fn own_build() -> Option<&'static str> {
    static BUILD: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    BUILD
        .get_or_init(|| {
            use sha2::{Digest, Sha256};
            let bytes = std::fs::read(crate::binlink::real_exe().ok()?).ok()?;
            Some(
                Sha256::digest(bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            )
        })
        .as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::Home;
    use sushiai_protocol::Decoder;

    #[tokio::test]
    async fn a_lagging_event_subscriber_gets_a_resync_without_a_list() {
        let (events, receiver) = broadcast::channel(2);
        for i in 0..5 {
            let _ = events.send(Notification::new("test.event", i));
        }
        let (out, mut queue) = mpsc::channel(16);
        tokio::spawn(forward_events(receiver, out));
        let bytes = tokio::time::timeout(Duration::from_secs(2), queue.recv())
            .await
            .expect("no resync within 2 s")
            .expect("first frame");
        let frames = Decoder::new().push(&bytes).expect("decode");
        let Some(Frame::Json(text)) = frames.into_iter().next() else {
            panic!("expected a JSON frame");
        };
        let Ok(Message::Notification(note)) = Message::parse(&text) else {
            panic!("expected a notification");
        };
        assert_eq!(note.method, method::SESSION_RESYNC);
        assert_eq!(note.params, json!({}), "the client lists again; no payload");
    }

    #[tokio::test]
    async fn an_attach_stream_starts_only_after_the_response_is_queued() {
        let dir = tempfile::Builder::new()
            .prefix("sv")
            .tempdir_in("/tmp")
            .expect("tempdir");
        let registry = Arc::new(Registry::new(Home::new(dir.path().to_path_buf())));
        // A holder that accepts the connection and says nothing.
        let sock = dir.path().join("h.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock).expect("bind");
        let holder = HolderConn::connect(&sock).await.expect("connect");
        let (_peer, _) = listener.accept().expect("accept");
        let info = SessionInfo {
            id: "s1".into(),
            cmd: vec![],
            cwd: String::new(),
            title: None,
            status: SessionStatus::Running,
            exit_code: None,
            holder_pid: None,
            cols: 80,
            rows: 24,
            project: None,
            group: None,
            agent: Default::default(),
        };
        session::start(
            registry.clone(),
            info,
            holder,
            Screen::new(24, 80),
            0,
            sock,
            false,
        );

        let (out, _queue) = mpsc::channel(8);
        let mut conn = Conn {
            registry,
            id: 0,
            out,
            said_hello: true,
            hook_only: false,
            streams: HashMap::new(),
            events: None,
            starting: None,
        };
        conn.attach(SessionAttach {
            id: "s1".into(),
            scrollback: None,
        })
        .await
        .expect("attach");
        assert!(
            conn.streams.is_empty(),
            "the stream started before the response"
        );
        conn.start_stream();
        assert_eq!(conn.streams.len(), 1);
    }
}
