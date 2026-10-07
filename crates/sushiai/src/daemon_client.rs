//! The small blocking daemon client the `orch` commands and `sushiai mcp` share: one
//! connection after `hello`, and [`orch_caller`], the closure the orchestration library
//! sends its `orch.*` requests through.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};
use sushiai_orch::mcp::Caller;
use sushiai_protocol::{encode, Decoder, Frame, Message, Request, PROTOCOL_VERSION};

pub enum CallError {
    /// The connection ended before an answer; a fresh one may work.
    Closed(String),
    /// The daemon answered with an error.
    Rpc(String),
}

impl CallError {
    pub fn message(self) -> String {
        match self {
            CallError::Closed(e) | CallError::Rpc(e) => e,
        }
    }
}

/// One blocking connection to the daemon, after `hello`.
pub struct Conn {
    reader: Box<dyn Read>,
    writer: Box<dyn Write>,
    decoder: Decoder,
    next_id: u64,
    child: Option<Child>,
}

impl Conn {
    /// Through `sushiai proxy`, which starts the daemon when nobody answers.
    pub fn via_proxy(client: &str) -> Result<Conn, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find sushiai: {e}"))?;
        let mut child = Command::new(exe)
            .arg("proxy")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("cannot start sushiai proxy: {e}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("sushiai proxy has no pipes".into());
        };
        Conn::hello(Box::new(stdout), Box::new(stdin), Some(child), client)
    }

    /// Straight to the socket; never starts a daemon.
    pub fn direct(socket: &Path, client: &str) -> Result<Conn, String> {
        let stream = UnixStream::connect(socket).map_err(|e| e.to_string())?;
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        Conn::hello(Box::new(reader), Box::new(stream), None, client)
    }

    fn hello(
        reader: Box<dyn Read>,
        writer: Box<dyn Write>,
        child: Option<Child>,
        client: &str,
    ) -> Result<Conn, String> {
        let mut conn = Conn {
            reader,
            writer,
            decoder: Decoder::new(),
            next_id: 1,
            child,
        };
        let params = json!({"protocol": PROTOCOL_VERSION, "client": client});
        conn.call("hello", params).map_err(CallError::message)?;
        Ok(conn)
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, CallError> {
        let id = self.next_id;
        self.next_id += 1;
        let frame = Request::new(id, method, params).frame();
        self.writer
            .write_all(&encode(&frame))
            .and_then(|()| self.writer.flush())
            .map_err(|e| CallError::Closed(format!("daemon write failed: {e}")))?;
        let mut buf = [0u8; 8192];
        loop {
            let n = match self.reader.read(&mut buf) {
                Ok(0) => return Err(CallError::Closed("the daemon closed the connection".into())),
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(CallError::Closed(format!("daemon read failed: {e}"))),
            };
            let frames = self
                .decoder
                .push(&buf[..n])
                .map_err(|e| CallError::Closed(format!("bad frame from the daemon: {e}")))?;
            for frame in frames {
                // Notifications and output frames are not answers.
                let Frame::Json(text) = frame else { continue };
                if let Ok(Message::Response(r)) = Message::parse(&text) {
                    if r.id == json!(id) {
                        return match r.error {
                            Some(e) => Err(CallError::Rpc(e.message)),
                            None => Ok(r.result.unwrap_or(Value::Null)),
                        };
                    }
                }
            }
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        // The proxy ends when its stdin does; do not leave it behind if it does not.
        if let Some(mut child) = self.child.take() {
            let _ = std::mem::replace(&mut self.writer, Box::new(std::io::sink()));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A [`Caller`] for `orch.<method>` over a lazily opened connection (through `sushiai proxy`,
/// which starts the daemon when nobody answers). A dropped connection is retried once on a
/// fresh one.
pub fn orch_caller(client: &'static str) -> Caller {
    let conn: RefCell<Option<Conn>> = RefCell::new(None);
    Box::new(move |method, params| {
        let method = format!("orch.{method}");
        let mut slot = conn.borrow_mut();
        for attempt in 0..2 {
            if slot.is_none() {
                *slot = Some(Conn::via_proxy(client)?);
            }
            let Some(live) = slot.as_mut() else { break };
            match live.call(&method, params.clone()) {
                Err(CallError::Closed(e)) if attempt == 1 => return Err(e),
                Err(CallError::Closed(_)) => *slot = None,
                Err(CallError::Rpc(e)) => return Err(e),
                Ok(v) => return Ok(v),
            }
        }
        Err("the daemon connection failed".to_string())
    })
}
