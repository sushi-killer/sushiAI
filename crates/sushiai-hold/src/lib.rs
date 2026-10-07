//! The session holder: owns a PTY and its child, keeps recent output in memory and
//! serves one client (the daemon) over a unix socket. Synchronous, no async runtime.
//! It outlives the daemon: a new daemon reattaches and replays the ring.
//!
//! No thread waits on slow I/O while holding the state lock or while handling control
//! requests: PTY input and client output each have their own writer thread and a bounded queue.

#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod ring;

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use std::{fs, thread};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use ring::Ring;
use serde_json::{json, Value};
use sushiai_protocol::{
    code, encode, method, Decoder, Frame, HoldAttach, HoldAttachResult, HoldClose, HoldExited,
    HoldInput, HoldResize, Message, Notification, Request, Response, Signal, SizeMark,
};

const RING_BYTES: usize = 2 * 1024 * 1024;
const REPLAY_CHUNK: usize = 64 * 1024;
const EXIT_WAIT: Duration = Duration::from_secs(60);
/// How often a holder checks that its socket (in the sessions directory) still exists.
const GONE_CHECK: Duration = Duration::from_secs(60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Bytes of PTY input that may wait for a child that is not reading.
const INPUT_LIMIT: usize = 256 * 1024;
/// Frames queued for the client; a client further behind is dropped and reattaches.
const CLIENT_QUEUE: usize = 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Start(String),
}

pub struct Config {
    pub id: String,
    pub dir: PathBuf,
    pub cols: u16,
    pub rows: u16,
    pub cwd: PathBuf,
    pub cmd: Vec<String>,
}

enum Out {
    Bytes(Vec<u8>),
    /// The `hold.exited` notification: the writer marks it delivered once written.
    Exit(Vec<u8>),
}

struct Client {
    conn: u64,
    tx: SyncSender<Out>,
    stream: UnixStream,
    attached: bool,
}

struct Exit {
    code: Option<i32>,
    at: Instant,
}

struct State {
    ring: Ring,
    /// Every PTY size with the stream offset it began at; the first entry is the spawn size.
    sizes: Vec<SizeMark>,
    client: Option<Client>,
    exit: Option<Exit>,
}

struct Holder {
    id: String,
    state: Mutex<State>,
    input: Sender<Vec<u8>>,
    input_pending: AtomicUsize,
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// The child's pid until it is reaped. Signals are sent under this lock.
    pid: Mutex<Option<u32>>,
    reader_done: AtomicBool,
    exit_delivered: AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A holder whose child runs and whose socket is bound; call `serve` to run it.
pub struct Running {
    holder: Arc<Holder>,
    listener: UnixListener,
    sock_path: PathBuf,
}

/// Starts the child and binds the socket. The socket exists only if the child started;
/// every error path leaves no socket and no child behind.
pub fn start(cfg: Config) -> Result<Running, Error> {
    let Some((program, args)) = cfg.cmd.split_first() else {
        return Err(Error::Start("empty command".into()));
    };
    let fail = |what: &str, e: &dyn std::fmt::Display| {
        Error::Start(format!(
            "cannot start {program} in {}: {what}: {e}",
            cfg.cwd.display()
        ))
    };
    if !cfg.cwd.is_dir() {
        return Err(Error::Start(format!(
            "cannot start {program}: working directory {} does not exist",
            cfg.cwd.display()
        )));
    }
    if !cfg.dir.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&cfg.dir)?;
    }
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: cfg.rows,
            cols: cfg.cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| fail("open pty", &e))?;
    let mut command = CommandBuilder::new(program);
    command.args(args);
    command.cwd(&cfg.cwd);
    command.env("TERM", "xterm-256color");
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| fail("spawn", &e))?;
    drop(pair.slave);
    let pid = child.process_id();
    let io = (|| {
        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        Ok::<_, String>((reader, writer))
    })();
    let (reader, writer) = match io {
        Ok(io) => io,
        Err(e) => {
            kill_group(pid, libc::SIGKILL);
            return Err(fail("pty io", &e));
        }
    };

    let sock_path = cfg.dir.join(format!("{}.sock", cfg.id));
    let listener = match bind(&sock_path) {
        Ok(listener) => listener,
        Err(e) => {
            kill_group(pid, libc::SIGKILL);
            let _ = fs::remove_file(&sock_path);
            return Err(fail("bind socket", &e));
        }
    };

    let (input_tx, input_rx) = channel();
    let holder = Arc::new(Holder {
        id: cfg.id.clone(),
        state: Mutex::new(State {
            ring: Ring::new(RING_BYTES),
            sizes: vec![SizeMark {
                seq: 0,
                cols: cfg.cols,
                rows: cfg.rows,
            }],
            client: None,
            exit: None,
        }),
        input: input_tx,
        input_pending: AtomicUsize::new(0),
        master: Mutex::new(pair.master),
        pid: Mutex::new(pid),
        reader_done: AtomicBool::new(false),
        exit_delivered: AtomicBool::new(false),
    });

    let h = holder.clone();
    thread::spawn(move || pump_output(&h, reader));
    let h = holder.clone();
    thread::spawn(move || pump_input(&h, writer, input_rx));
    let h = holder.clone();
    thread::spawn(move || {
        let code = reap(&h, &mut *child);
        wait_for_reader(&h);
        let mut state = lock(&h.state);
        state.exit = Some(Exit {
            code,
            at: Instant::now(),
        });
        deliver_exit(&h, &mut state);
    });
    Ok(Running {
        holder,
        listener,
        sock_path,
    })
}

fn bind(path: &Path) -> std::io::Result<UnixListener> {
    let _ = fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// Replaces stderr with /dev/null. The daemon reads the holder's stderr until it closes,
/// so closing it is the "started fine" signal.
pub fn detach_stderr() {
    if let Ok(null) = fs::OpenOptions::new().write(true).open("/dev/null") {
        // SAFETY: dup2 onto fd 2 with a valid descriptor; nothing else owns fd 2 here.
        unsafe {
            libc::dup2(null.as_raw_fd(), 2);
        }
    }
}

impl Running {
    /// Runs until the child has exited and the exit was delivered (or 60 s passed), or the
    /// session directory is gone: nobody can reach the holder then, so it ends too.
    pub fn serve(self) -> Result<(), Error> {
        self.serve_checking(GONE_CHECK)
    }

    /// `serve` that looks for the vanished session directory every `check`.
    pub fn serve_checking(self, check: Duration) -> Result<(), Error> {
        let Running {
            holder,
            listener,
            sock_path,
        } = self;
        let mut next_conn = 0u64;
        let mut checked = Instant::now();
        let result = loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    next_conn += 1;
                    // A broken connection attempt must not stop the holder.
                    let _ = connect(&holder, stream, next_conn);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(e) => break Err(e.into()),
            }
            let timed_out = lock(&holder.state)
                .exit
                .as_ref()
                .is_some_and(|e| e.at.elapsed() > EXIT_WAIT);
            if holder.exit_delivered.load(Ordering::SeqCst) || timed_out {
                break Ok(());
            }
            if checked.elapsed() >= check {
                checked = Instant::now();
                if !sock_path.exists() {
                    break Ok(());
                }
            }
        };
        let _ = fs::remove_file(&sock_path);
        result
    }
}

/// Waits for the child. `try_wait` runs under the pid lock, so a signal is never sent
/// to a pid that was already reaped.
fn reap(holder: &Holder, child: &mut (dyn portable_pty::Child + Send + Sync)) -> Option<i32> {
    loop {
        {
            let mut pid = lock(&holder.pid);
            match child.try_wait() {
                Ok(None) => {}
                Ok(Some(status)) => {
                    *pid = None;
                    return Some(status.exit_code() as i32);
                }
                Err(_) => {
                    *pid = None;
                    return None;
                }
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Most size marks kept; the oldest go first.
const MAX_SIZE_MARKS: usize = 256;

fn note_size(sizes: &mut Vec<SizeMark>, mark: SizeMark) {
    let last = sizes.last().copied();
    if last.is_some_and(|l| (l.cols, l.rows) == (mark.cols, mark.rows)) {
        return;
    }
    // A resize before any further output replaces the mark at the same offset.
    if last.is_some_and(|l| l.seq == mark.seq) {
        sizes.pop();
    }
    sizes.push(mark);
    if sizes.len() > MAX_SIZE_MARKS {
        sizes.remove(0);
    }
}

/// The marks that apply to a replay starting at `start`: the size in force there, rebased
/// to `start`, then every later change.
fn sizes_from(sizes: &[SizeMark], start: u64) -> Vec<SizeMark> {
    let Some(first) = sizes.iter().rposition(|m| m.seq <= start) else {
        return sizes.to_vec();
    };
    let mut out = sizes[first..].to_vec();
    out[0].seq = start;
    out
}

fn pump_output(holder: &Holder, mut reader: Box<dyn Read + Send>) {
    let mut buf = [0u8; 8192];
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            break;
        }
        let mut state = lock(&holder.state);
        let seq = state.ring.end();
        state.ring.push(&buf[..n]);
        let frame = Frame::Output {
            id: holder.id.clone(),
            seq,
            data: buf[..n].to_vec(),
        };
        enqueue(&mut state, None, Out::Bytes(encode(&frame)));
    }
    holder.reader_done.store(true, Ordering::SeqCst);
}

/// Writes queued input to the PTY. It may block on a child that does not read; nothing
/// else waits for it.
fn pump_input(holder: &Holder, mut writer: Box<dyn Write + Send>, queue: Receiver<Vec<u8>>) {
    while let Ok(data) = queue.recv() {
        let ok = writer
            .write_all(&data)
            .and_then(|()| writer.flush())
            .is_ok();
        holder.input_pending.fetch_sub(data.len(), Ordering::SeqCst);
        if !ok {
            return;
        }
    }
}

/// Gives the output reader a moment to drain after the child exits.
fn wait_for_reader(holder: &Holder) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !holder.reader_done.load(Ordering::SeqCst) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
}

/// Drops the client and closes its socket, so the daemon sees EOF and can reattach.
fn drop_client(state: &mut State) {
    if let Some(client) = state.client.take() {
        let _ = client.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Queues bytes for a client without waiting: `conn` picks a connection, `None` the attached
/// one. A client whose queue is full is dropped.
fn enqueue(state: &mut State, conn: Option<u64>, out: Out) -> bool {
    let Some(client) = state
        .client
        .as_ref()
        .filter(|c| conn.map_or(c.attached, |n| c.conn == n))
    else {
        return false;
    };
    if client.tx.try_send(out).is_ok() {
        return true;
    }
    drop_client(state);
    false
}

fn deliver_exit(holder: &Holder, state: &mut State) {
    if holder.exit_delivered.load(Ordering::SeqCst) {
        return;
    }
    let Some(code) = state.exit.as_ref().map(|e| e.code) else {
        return;
    };
    let frame = Notification::new(method::HOLD_EXITED, HoldExited { code }).frame();
    enqueue(state, None, Out::Exit(encode(&frame)));
}

fn connect(holder: &Arc<Holder>, stream: UnixStream, conn: u64) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let reader = stream.try_clone()?;
    let writer = stream.try_clone()?;
    let (tx, rx) = sync_channel(CLIENT_QUEUE);
    let mut state = lock(&holder.state);
    drop_client(&mut state);
    state.client = Some(Client {
        conn,
        tx,
        stream,
        attached: false,
    });
    drop(state);
    let h = holder.clone();
    thread::spawn(move || client_writer(&h, writer, rx));
    let h = holder.clone();
    thread::spawn(move || serve_client(&h, reader, conn));
    Ok(())
}

/// Owns the write side of one client connection.
fn client_writer(holder: &Holder, mut stream: UnixStream, queue: Receiver<Out>) {
    while let Ok(out) = queue.recv() {
        let (bytes, exit) = match out {
            Out::Bytes(bytes) => (bytes, false),
            Out::Exit(bytes) => (bytes, true),
        };
        if stream.write_all(&bytes).is_err() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return;
        }
        if exit {
            holder.exit_delivered.store(true, Ordering::SeqCst);
        }
    }
}

fn serve_client(holder: &Holder, mut stream: UnixStream, conn: u64) {
    let mut decoder = Decoder::new();
    let mut buf = [0u8; 8192];
    'read: loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let Ok(frames) = decoder.push(&buf[..n]) else {
            break;
        };
        for frame in frames {
            if let Frame::Json(text) = frame {
                match Message::parse(&text) {
                    Ok(Message::Request(request)) => handle(holder, conn, request),
                    Ok(_) => {}
                    Err(_) => break 'read,
                }
            }
        }
    }
    let mut state = lock(&holder.state);
    if state.client.as_ref().is_some_and(|c| c.conn == conn) {
        drop_client(&mut state);
    }
}

fn handle(holder: &Holder, conn: u64, request: Request) {
    let id = request.id.clone();
    let response = match dispatch(holder, conn, &request) {
        Ok(Some(result)) => Response::ok(id, result),
        // Attach queued its own response ahead of the replayed bytes.
        Ok(None) => return,
        Err((code, message)) => Response::err(id, code, message),
    };
    enqueue(
        &mut lock(&holder.state),
        Some(conn),
        Out::Bytes(encode(&response.frame())),
    );
}

type Fail = (i64, String);

fn params<T: serde::de::DeserializeOwned>(request: &Request) -> Result<T, Fail> {
    serde_json::from_value(request.params.clone())
        .map_err(|e| (code::INVALID_PARAMS, e.to_string()))
}

fn dispatch(holder: &Holder, conn: u64, request: &Request) -> Result<Option<Value>, Fail> {
    match request.method.as_str() {
        method::HOLD_ATTACH => {
            let HoldAttach { from_seq } = params(request)?;
            let mut state = lock(&holder.state);
            let (start, bytes) = state.ring.read_from(from_seq);
            let next_seq = state.ring.end();
            let head = Response::ok(
                request.id.clone(),
                HoldAttachResult {
                    from_seq: start,
                    next_seq,
                    pid: std::process::id(),
                    sizes: sizes_from(&state.sizes, start),
                    child: *lock(&holder.pid),
                },
            );
            enqueue(&mut state, Some(conn), Out::Bytes(encode(&head.frame())));
            for (i, chunk) in bytes.chunks(REPLAY_CHUNK).enumerate() {
                let seq = start + (i * REPLAY_CHUNK) as u64;
                let frame = Frame::Output {
                    id: holder.id.clone(),
                    seq,
                    data: chunk.to_vec(),
                };
                enqueue(&mut state, Some(conn), Out::Bytes(encode(&frame)));
            }
            if let Some(client) = state.client.as_mut().filter(|c| c.conn == conn) {
                client.attached = true;
            }
            deliver_exit(holder, &mut state);
            Ok(None)
        }
        method::HOLD_INPUT => {
            let HoldInput { data } = params(request)?;
            let len = data.len();
            if holder.input_pending.fetch_add(len, Ordering::SeqCst) + len > INPUT_LIMIT {
                holder.input_pending.fetch_sub(len, Ordering::SeqCst);
                return Err((
                    code::INPUT_BACKPRESSURE,
                    "the child is not reading input".into(),
                ));
            }
            if holder.input.send(data).is_err() {
                holder.input_pending.fetch_sub(len, Ordering::SeqCst);
                return Err((code::SESSION_NOT_RUNNING, "input closed".into()));
            }
            Ok(Some(json!({})))
        }
        method::HOLD_RESIZE => {
            let HoldResize { cols, rows } = params(request)?;
            // Under the state lock, so no output byte lands between the new size and its mark.
            let mut state = lock(&holder.state);
            lock(&holder.master)
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| (code::INTERNAL, e.to_string()))?;
            let seq = state.ring.end();
            note_size(&mut state.sizes, SizeMark { seq, cols, rows });
            Ok(Some(json!({})))
        }
        method::HOLD_CLOSE => {
            let HoldClose { signal } = params(request)?;
            let sig = match signal {
                Signal::Term => libc::SIGTERM,
                Signal::Kill => libc::SIGKILL,
            };
            // Nothing to signal once the child is reaped.
            kill_group(*lock(&holder.pid), sig);
            Ok(Some(json!({})))
        }
        other => Err((code::METHOD_NOT_FOUND, format!("unknown method {other}"))),
    }
}

/// Signals the child's process group; the child is a session leader, so its pgid is its pid.
fn kill_group(pid: Option<u32>, sig: i32) {
    let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) else {
        return;
    };
    // SAFETY: kill(2) takes plain integers; it signals only the group of a child this holder started.
    unsafe {
        libc::kill(-pid, sig);
    }
}
