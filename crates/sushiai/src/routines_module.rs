//! The composition point for the routines module: the only place that knows both the
//! daemon's `Module` trait and the `sushiai-routines` library. The daemon and the protocol
//! never learn what a routine is; a routine reaches the daemon as a schedule entry and its
//! runs as background sessions.
//!
//! The module is opt-in, like the orchestrator: the daemon hosts it only when
//! `<home>/modules/routines.enabled` exists when the daemon starts.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sushiai_daemon::{BoxFuture, Home, Module, ModuleNotify, ModuleSlot, Reply};
use sushiai_protocol::code;
use sushiai_routines::service::{CallError, Config, Connector, Git, Routines};

use crate::daemon_client::{Conn, Incoming};

const NAMESPACE: &str = "routines";
/// How often a dry run or a retry session is looked at.
const POLL: Duration = Duration::from_secs(2);
/// A git or gh command of an applied proposal may not run longer.
const GIT_WAIT: Duration = Duration::from_secs(120);

fn flag(home: &Path) -> PathBuf {
    home.join("modules").join(format!("{NAMESPACE}.enabled"))
}

/// Whether the daemon of `home` hosts the routines module.
pub fn enabled(home: &Path) -> bool {
    flag(home).is_file()
}

/// Turns hosting on or off for the next daemon start.
pub fn set_enabled(home: &Path, on: bool) -> io::Result<()> {
    let path = flag(home);
    if on {
        std::fs::create_dir_all(path.parent().unwrap_or(home))?;
        std::fs::write(&path, b"")
    } else {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// The routine store of a home: `<home>/routines`.
pub fn root(home: &Path) -> PathBuf {
    home.join("routines")
}

/// The modules `main` passes to the daemon: routines only when enabled.
pub fn slots(home: &Home) -> Vec<ModuleSlot> {
    if !enabled(home.dir()) {
        return Vec::new();
    }
    let dir = home.dir().to_path_buf();
    let socket = home.socket();
    vec![ModuleSlot::new(NAMESPACE, move |notify| {
        Arc::new(RoutinesModule::start(dir, socket, notify))
    })]
}

/// The daemon's own socket, over one lazily opened connection. A call that was written is
/// never sent twice; one that could not be written is retried on a fresh connection.
struct DaemonConnector {
    socket: PathBuf,
    conn: Mutex<Option<Conn>>,
    captures: Mutex<HashMap<String, Arc<Capture>>>,
}

/// Most output kept per session: a result is small; the tail holds the answer.
const CAPTURE_CAP: usize = 4 * 1024 * 1024;
/// How long a read waits for a capture whose session just ended.
const CAPTURE_WAIT: Duration = Duration::from_secs(5);

/// What one watched session printed. An exited session cannot be read from the daemon, so
/// the output is collected from an attach made while it runs.
struct Capture {
    state: Mutex<CaptureState>,
    done: Condvar,
}

#[derive(Default)]
struct CaptureState {
    /// The screen the attach started with: used only when no live output arrived.
    snapshot: Vec<u8>,
    live: Vec<u8>,
    finished: bool,
}

/// How long `session_watch` waits for its attach before it returns anyway.
const ATTACH_WAIT: Duration = Duration::from_secs(5);

/// Attaches to the session. `session_watch` waits for this before it returns, so a
/// session that ends at once is still seen by the attach.
fn attach(socket: &Path, id: &str, capture: &Capture) -> Result<(Conn, u64), String> {
    Conn::direct(socket, "routines-capture").and_then(|mut conn| {
        let snapshot = conn
            .call_keeping("session.attach", json!({"id": id, "scrollback": 2000}))
            .map_err(|e| e.message())?;
        let attach: sushiai_protocol::AttachResult =
            serde_json::from_value(snapshot).map_err(|e| e.to_string())?;
        capture
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot = attach.snapshot;
        Ok((conn, attach.seq))
    })
}

/// Collects what an attached session prints until it exits. The exit note and the output
/// frames are written to one connection by two tasks, so the note can overtake the last
/// output: the capture ends only when the stream offset the note reports has been received.
fn collect(attached: Result<(Conn, u64), String>, id: &str, capture: &Capture) {
    if let Ok((mut conn, mut received)) = attached {
        let mut end: Option<u64> = None;
        while let Ok(incoming) = conn.next_incoming() {
            match incoming {
                Incoming::Output { seq, mut data } => {
                    let tail = seq + data.len() as u64;
                    if tail <= received {
                        continue;
                    }
                    if seq < received {
                        data.drain(..(received - seq) as usize);
                    }
                    received = tail;
                    let mut state = capture.state.lock().unwrap_or_else(|e| e.into_inner());
                    state.live.extend_from_slice(&data);
                    if state.live.len() > CAPTURE_CAP {
                        let cut = state.live.len() - CAPTURE_CAP;
                        state.live.drain(..cut);
                    }
                }
                Incoming::Note(method, params) => {
                    if method == "session.exited" && params["id"] == id {
                        // A note without an offset (an older daemon) is all there is.
                        end = Some(params["seq"].as_u64().unwrap_or(received));
                        // The rest is already sent; a stream that stops short of it ends the
                        // capture instead of holding this thread.
                        conn.set_read_timeout(CAPTURE_WAIT);
                    } else if method == "session.snapshot" && params["id"] == id {
                        // The stream resynced: what it skipped is not coming.
                        received = params["seq"].as_u64().unwrap_or(received).max(received);
                    }
                }
            }
            if end.is_some_and(|end| received >= end) {
                break;
            }
        }
    }
    capture
        .state
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .finished = true;
    capture.done.notify_all();
}

impl DaemonConnector {
    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut slot = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        for attempt in 0..2 {
            if slot.is_none() {
                *slot = Some(Conn::direct(&self.socket, "routines")?);
            }
            let live = slot.as_mut().expect("connection was just opened");
            match live.call(method, params.clone()) {
                Ok(v) => return Ok(v),
                Err(crate::daemon_client::CallError::NotSent(e)) if attempt == 0 => {
                    let _ = e;
                    *slot = None;
                }
                Err(e) => {
                    let message = e.message();
                    *slot = None;
                    return Err(message);
                }
            }
        }
        Err("the daemon did not take the request".into())
    }
}

impl Connector for DaemonConnector {
    fn catalog(&self) -> Result<Value, String> {
        self.call("catalog.get", json!({}))
    }

    fn schedule_set(&self, set: Value) -> Result<(), String> {
        self.call("schedule.set", set).map(|_| ())
    }

    fn schedule_remove(&self, id: &str) -> Result<(), String> {
        match self.call("schedule.remove", json!({"id": id})) {
            // An entry that is already gone is what the caller wanted.
            Err(m) if m.contains("no schedule") => Ok(()),
            other => other.map(|_| ()),
        }
    }

    fn schedule_ids(&self) -> Result<Vec<String>, String> {
        let list = self.call("schedule.list", json!({}))?;
        Ok(list
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["id"].as_str().map(String::from))
            .collect())
    }

    fn schedule_fire(&self, id: &str) -> Result<Value, String> {
        self.call("schedule.fire", json!({"id": id}))
    }

    fn session_create(&self, params: Value) -> Result<String, String> {
        let created = self.call("session.create", params)?;
        created["id"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| "session.create returned no id".to_string())
    }

    fn session_info(&self, id: &str) -> Result<Value, String> {
        let list = self.call("session.list", json!({}))?;
        list.as_array()
            .into_iter()
            .flatten()
            .find(|s| s["id"] == id)
            .cloned()
            .ok_or_else(|| format!("no session {id}"))
    }

    fn session_watch(&self, id: &str) {
        let capture = Arc::new(Capture {
            state: Mutex::new(CaptureState::default()),
            done: Condvar::new(),
        });
        self.captures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), capture.clone());
        // Waits for the attach, bounded: a session that ends at once is still seen, and a
        // stalled daemon never holds the caller (the routines event reader).
        let (socket, id) = (self.socket.clone(), id.to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let attached = attach(&socket, &id, &capture);
            let _ = tx.send(());
            collect(attached, &id, &capture);
        });
        let _ = rx.recv_timeout(ATTACH_WAIT);
    }

    fn session_read(&self, id: &str) -> Result<String, String> {
        let capture = self
            .captures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .ok_or_else(|| format!("no output was captured for session {id}"))?;
        let guard = capture.state.lock().unwrap_or_else(|e| e.into_inner());
        let (state, _) = capture
            .done
            .wait_timeout_while(guard, CAPTURE_WAIT, |s| !s.finished)
            .unwrap_or_else(|e| e.into_inner());
        let bytes = if state.live.is_empty() {
            &state.snapshot
        } else {
            &state.live
        };
        Ok(sushiai_routines::result::plain(bytes))
    }

    fn session_close(&self, id: &str) -> Result<(), String> {
        self.call("session.close", json!({"id": id, "graceful": false}))
            .map(|_| ())
    }

    fn hook_ask(&self, params: Value) -> Result<Value, String> {
        crate::routines_cli::hook_ask(&self.socket, params)
    }
}

/// Runs git and gh with a bounded wait; output is returned, never logged.
struct ProcessGit;

impl Git for ProcessGit {
    fn run(&self, argv: &[String], cwd: &Path) -> Result<String, String> {
        let (program, args) = argv.split_first().ok_or("empty command")?;
        if program != "git" && program != "gh" {
            return Err(format!("{program} is not a git command"));
        }
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start {program}: {e}"))?;
        let deadline = std::time::Instant::now() + GIT_WAIT;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{program} took longer than {GIT_WAIT:?}"));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(e.to_string()),
            }
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
}

/// The `PATH` of a run: the folder of the executable the module runs in comes first, so
/// `sushiai` resolves, then the daemon's own `PATH`.
fn run_path() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(path_with(
        exe.parent()?,
        std::env::var("PATH").ok().as_deref(),
    ))
}

fn path_with(bin_dir: &Path, path: Option<&str>) -> String {
    let dir = bin_dir.to_string_lossy();
    match path.filter(|p| !p.is_empty()) {
        Some(p) => format!("{dir}:{p}"),
        None => dir.into_owned(),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

struct RoutinesModule {
    svc: Routines,
}

impl RoutinesModule {
    fn start(home: PathBuf, socket: PathBuf, notify: ModuleNotify) -> RoutinesModule {
        let svc = Routines::new(Config {
            root: root(&home),
            connector: Arc::new(DaemonConnector {
                socket: socket.clone(),
                conn: Mutex::new(None),
                captures: Mutex::new(HashMap::new()),
            }),
            git: Arc::new(ProcessGit),
            emit: Arc::new(move |event| notify.send("event", event)),
            now_ms: Arc::new(now_ms),
            run_path: run_path(),
            poll: POLL,
        });
        let listener = svc.clone();
        let spawned = std::thread::Builder::new()
            .name("routines-events".into())
            .spawn(move || listen(&listener, &socket));
        if let Err(e) = spawned {
            eprintln!("routines: cannot start the event thread: {e}");
        }
        RoutinesModule { svc }
    }
}

/// Reads the daemon's `schedule.event` notifications for ever, reconnecting after a drop.
/// Every connect first syncs the schedule entries with the store.
fn listen(svc: &Routines, socket: &Path) {
    loop {
        if let Ok(mut conn) = Conn::direct(socket, "routines-events") {
            let sync = svc.clone();
            std::thread::spawn(move || sync.resync());
            while let Ok((method, params)) = conn.next_notification() {
                if method != "schedule.event" {
                    continue;
                }
                // Queued here in arrival order; the service handles each routine's events on
                // a worker of its own (a run is read and maybe retried, which waits).
                svc.enqueue_event(params);
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

impl Module for RoutinesModule {
    fn namespace(&self) -> &'static str {
        NAMESPACE
    }

    fn capability(&self) -> &'static str {
        NAMESPACE
    }

    fn call(&self, method: &str, params: Value) -> BoxFuture<Reply> {
        let svc = self.svc.clone();
        let method = method.to_string();
        Box::pin(async move {
            let name = method.clone();
            let outcome = tokio::task::spawn_blocking(move || svc.call(&method, params))
                .await
                .map_err(|e| (code::INTERNAL, format!("routines call panicked: {e}")))?;
            outcome.map_err(|e| match e {
                CallError::UnknownMethod(_) => (
                    code::METHOD_NOT_FOUND,
                    format!("unknown method {NAMESPACE}.{name}"),
                ),
                CallError::Failed(message) => (code::INVALID_REQUEST, message),
            })
        })
    }

    fn shutdown(&self) -> BoxFuture<()> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use sushiai_protocol::{encode, Decoder, Frame};

    fn json_frame(value: Value) -> Vec<u8> {
        encode(&Frame::Json(value.to_string()))
    }

    /// A daemon that answers `hello` and `session.attach`. `early` goes out before the attach
    /// answer; once `go` fires it sends `tail`: the frames a real daemon writes from two tasks
    /// (the output stream and the event forwarder) in whatever order they win the connection.
    /// The connection stays open until the test drops `hold`, as a live daemon keeps it.
    fn fake_daemon(
        socket: &Path,
        early: Vec<Vec<u8>>,
        go: mpsc::Receiver<()>,
        tail: Vec<Vec<u8>>,
        hold: mpsc::Receiver<()>,
    ) {
        let listener = UnixListener::bind(socket).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut decoder = Decoder::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0, "the client closed before it attached");
                for frame in decoder.push(&buf[..n]).unwrap() {
                    let Frame::Json(text) = frame else { continue };
                    let request: Value = serde_json::from_str(&text).unwrap();
                    let attach = request["method"] == "session.attach";
                    let result = if attach {
                        for frame in &early {
                            stream.write_all(frame).unwrap();
                        }
                        json!({"snapshot": "", "seq": 0, "cols": 80, "rows": 24})
                    } else {
                        json!({"protocol": 1, "capabilities": [], "daemon": "t", "host": "t"})
                    };
                    let reply = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
                    stream.write_all(&json_frame(reply)).unwrap();
                    if attach {
                        go.recv().unwrap();
                        for frame in &tail {
                            stream.write_all(frame).unwrap();
                        }
                        stream.flush().unwrap();
                        let _ = hold.recv();
                        return;
                    }
                }
            }
        });
    }

    fn exited(id: &str, end: u64) -> Vec<u8> {
        json_frame(json!({"jsonrpc": "2.0", "method": "session.exited",
                          "params": {"id": id, "code": 0, "seq": end}}))
    }

    fn output(id: &str, seq: u64, data: &[u8]) -> Vec<u8> {
        encode(&Frame::Output {
            id: id.into(),
            seq,
            data: data.to_vec(),
        })
    }

    /// What the capture of session `s1` holds once it finished, and how long that took.
    fn collected(early: Vec<Vec<u8>>, tail: Vec<Vec<u8>>) -> (String, Duration) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let (go, wait) = mpsc::channel();
        let (_hold, held) = mpsc::channel();
        fake_daemon(&socket, early, wait, tail, held);
        let capture = Arc::new(Capture {
            state: Mutex::new(CaptureState::default()),
            done: Condvar::new(),
        });
        let attached = attach(&socket, "s1", &capture);
        go.send(()).unwrap();
        let started = std::time::Instant::now();
        let (done, finished) = mpsc::channel();
        let collector = capture.clone();
        std::thread::spawn(move || {
            collect(attached, "s1", &collector);
            let _ = done.send(());
        });
        // Bounded so a capture that never ends fails instead of hanging the test.
        finished
            .recv_timeout(CAPTURE_WAIT * 4)
            .expect("the capture never finished");
        let took = started.elapsed();
        let state = capture.state.lock().unwrap();
        assert!(state.finished);
        (String::from_utf8_lossy(&state.live).into_owned(), took)
    }

    #[test]
    fn output_that_arrives_after_the_exit_note_is_still_collected() {
        let (live, took) = collected(
            vec![],
            vec![
                output("s1", 0, b"first "),
                exited("s1", 10),
                output("s1", 6, b"last"),
            ],
        );
        assert_eq!(live, "first last");
        assert!(took < CAPTURE_WAIT, "it waited for nothing: {took:?}");
    }

    #[test]
    fn an_exit_note_ahead_of_all_output_does_not_end_the_capture_empty() {
        let (live, took) = collected(vec![], vec![exited("s1", 4), output("s1", 0, b"done")]);
        assert_eq!(live, "done");
        assert!(took < CAPTURE_WAIT, "it waited for nothing: {took:?}");
    }

    #[test]
    fn output_and_exit_sent_before_the_attach_answer_are_kept() {
        let (live, took) = collected(vec![output("s1", 0, b"quick"), exited("s1", 5)], vec![]);
        assert_eq!(live, "quick");
        assert!(took < CAPTURE_WAIT, "it waited for nothing: {took:?}");
    }

    #[test]
    fn overlapping_output_frames_are_collected_once() {
        let (live, _) = collected(
            vec![],
            vec![
                output("s1", 0, b"ab"),
                output("s1", 1, b"bc"),
                exited("s1", 3),
            ],
        );
        assert_eq!(live, "abc");
    }

    #[test]
    fn a_stream_that_stops_short_of_the_exit_ends_the_capture_with_what_came() {
        let (live, took) = collected(vec![], vec![output("s1", 0, b"part"), exited("s1", 9)]);
        assert_eq!(live, "part");
        assert!(took >= CAPTURE_WAIT, "it ended before the bound: {took:?}");
    }

    #[test]
    fn a_run_finds_sushiai_through_the_executables_folder_then_the_daemons_path() {
        assert_eq!(
            path_with(Path::new("/opt/app/bin"), Some("/usr/bin:/bin")),
            "/opt/app/bin:/usr/bin:/bin"
        );
        assert_eq!(path_with(Path::new("/opt/app/bin"), None), "/opt/app/bin");
        assert_eq!(
            path_with(Path::new("/opt/app/bin"), Some("")),
            "/opt/app/bin"
        );
    }
}
