//! Client connections: framing, `hello`, method dispatch, attach streams.

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use sushiai_core::Screen;
use sushiai_protocol::{
    code, encode, method, AttachResult, Frame, Hello, HelloResult, Message, Notification, Request,
    Response, SessionClose, SessionCreate, SessionId, SessionInfo, SessionInput, SessionResize,
    SessionSnapshot, SessionStatus, SessionsResync, CAPABILITIES, PROTOCOL_VERSION,
};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::error::Fail;
use crate::framed::FrameReader;
use crate::holder::{self, HolderConn};
use crate::registry::Registry;
use crate::session::{self, Chunk, Handle, Snap};

type Outbox = mpsc::Sender<Vec<u8>>;

const OUTBOX: usize = 256;

pub async fn serve(listener: UnixListener, registry: Arc<Registry>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(connection(stream, registry.clone()));
            }
            Err(e) => {
                tracing::warn!("accept failed: {e}");
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

struct Conn {
    registry: Arc<Registry>,
    out: Outbox,
    said_hello: bool,
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

async fn connection(stream: UnixStream, registry: Arc<Registry>) {
    let (read, mut write) = stream.into_split();
    let (out, mut queue) = mpsc::channel::<Vec<u8>>(OUTBOX);
    let writer = tokio::spawn(async move {
        while let Some(bytes) = queue.recv().await {
            if write.write_all(&bytes).await.is_err() {
                break;
            }
        }
    });
    let mut conn = Conn {
        registry,
        out,
        said_hello: false,
        streams: HashMap::new(),
        events: None,
        starting: None,
    };
    let mut reader = FrameReader::new(read);
    while let Ok(Some(frame)) = reader.next().await {
        let Frame::Json(text) = frame else { continue };
        match Message::parse(&text) {
            Ok(Message::Request(request)) => {
                let response = conn.handle(&request).await;
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
    for task in conn.streams.values().chain(conn.events.iter()) {
        task.abort();
    }
    writer.abort();
}

fn params<T: DeserializeOwned>(request: &Request) -> Result<T, Fail> {
    serde_json::from_value(request.params.clone())
        .map_err(|e| (code::INVALID_PARAMS, e.to_string()))
}

impl Conn {
    async fn handle(&mut self, request: &Request) -> Response {
        match self.dispatch(request).await {
            Ok(result) => Response::ok(request.id.clone(), result),
            Err((code, message)) => Response::err(request.id.clone(), code, message),
        }
    }

    fn session(&self, id: &str) -> Result<Handle, Fail> {
        match self.registry.handle(id) {
            Some(handle) => Ok(handle),
            None if self.registry.known(id) => {
                Err((code::SESSION_NOT_RUNNING, "session is not running".into()))
            }
            None => Err((code::SESSION_NOT_FOUND, format!("no session {id}"))),
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
        if !self.said_hello {
            return Err((code::NOT_INITIALIZED, "send hello first".into()));
        }
        match request.method.as_str() {
            method::SESSION_CREATE => create(&self.registry, params(request)?).await,
            method::SESSION_LIST => Ok(json!(self.registry.list())),
            method::SESSION_INPUT => {
                let p: SessionInput = params(request)?;
                self.session(&p.id)?.input(p.data.into_bytes()).await?;
                Ok(json!({}))
            }
            method::SESSION_RESIZE => {
                let p: SessionResize = params(request)?;
                self.session(&p.id)?.resize(p.cols, p.rows).await?;
                Ok(json!({}))
            }
            method::SESSION_CLOSE => {
                let p: SessionClose = params(request)?;
                self.session(&p.id)?.close(p.graceful).await?;
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
            self.events = Some(tokio::spawn(forward_events(
                self.registry.clone(),
                self.registry.events.subscribe(),
                self.out.clone(),
            )));
        }
        Ok(json!(HelloResult {
            protocol: PROTOCOL_VERSION,
            capabilities: CAPABILITIES.iter().map(|c| (*c).to_string()).collect(),
            daemon: env!("CARGO_PKG_VERSION").into(),
        }))
    }

    async fn attach(&mut self, p: SessionId) -> Result<Value, Fail> {
        let handle = self.session(&p.id)?;
        let attached = handle
            .attach()
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

fn attach_result(snap: Snap) -> AttachResult {
    AttachResult {
        snapshot: snap.snapshot,
        seq: snap.seq,
        cols: snap.cols,
        rows: snap.rows,
    }
}

async fn forward_events(
    registry: Arc<Registry>,
    mut events: broadcast::Receiver<Notification>,
    out: Outbox,
) {
    loop {
        match events.recv().await {
            Ok(note) => {
                if out.send(encode(&note.frame())).await.is_err() {
                    break;
                }
            }
            // Missed events are replaced by the full list.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let sessions = registry.list();
                let note = Notification::new(method::SESSION_RESYNC, SessionsResync { sessions });
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
    if p.cmd.is_empty() || p.cols == 0 || p.rows == 0 {
        return Err((
            code::INVALID_PARAMS,
            "cmd, cols and rows are required".into(),
        ));
    }
    let spawn_failed = |e: &dyn std::fmt::Display| (code::SPAWN_FAILED, e.to_string());
    let id = random_id().map_err(|e| spawn_failed(&e))?;
    let dir = registry.home.sessions();
    let pid = holder::spawn(&holder::Spawn {
        id: &id,
        dir: &dir,
        cols: p.cols,
        rows: p.rows,
        cwd: &p.cwd,
        cmd: &p.cmd,
    })
    .await
    .map_err(|e| spawn_failed(&e))?;
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
            return Err(spawn_failed(&e));
        }
    };
    let info = SessionInfo {
        id: id.clone(),
        cmd: p.cmd,
        cwd: p.cwd,
        title: p.title,
        status: SessionStatus::Running,
        exit_code: None,
        holder_pid: None,
        cols: p.cols,
        rows: p.rows,
    };
    session::start(registry.clone(), info, conn, screen, seq, sock);
    Ok(json!({ "id": id }))
}

fn random_id() -> std::io::Result<String> {
    let mut bytes = [0u8; 8];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::Home;
    use sushiai_protocol::Decoder;

    #[tokio::test]
    async fn a_lagging_event_subscriber_gets_a_resync_with_the_full_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let registry = Arc::new(Registry::new(Home::new(dir.path().to_path_buf())));
        let (events, receiver) = broadcast::channel(2);
        for i in 0..5 {
            let _ = events.send(Notification::new("test.event", i));
        }
        let (out, mut queue) = mpsc::channel(16);
        tokio::spawn(forward_events(registry, receiver, out));
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
        assert!(note.params["sessions"].is_array());
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
        };
        session::start(registry.clone(), info, holder, Screen::new(24, 80), 0, sock);

        let (out, _queue) = mpsc::channel(8);
        let mut conn = Conn {
            registry,
            out,
            said_hello: true,
            streams: HashMap::new(),
            events: None,
            starting: None,
        };
        conn.attach(SessionId { id: "s1".into() })
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
