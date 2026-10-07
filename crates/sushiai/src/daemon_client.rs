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
    /// The request was never written; a fresh connection may work.
    NotSent(String),
    /// The connection ended after the request was written: the daemon may have
    /// run it, so it is not sent again.
    Closed(String),
    /// The daemon answered with an error.
    Rpc(String),
}

impl CallError {
    pub fn message(self) -> String {
        match self {
            CallError::NotSent(e) | CallError::Closed(e) | CallError::Rpc(e) => e,
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
            .map_err(|e| CallError::NotSent(format!("daemon write failed: {e}")))?;
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
/// which starts the daemon when nobody answers). A request that could not be written is
/// retried once on a fresh connection; one that was written is never sent twice.
pub fn orch_caller(client: &'static str) -> Caller {
    let conn: RefCell<Option<Conn>> = RefCell::new(None);
    Box::new(move |method, params| {
        let method = format!("orch.{method}");
        call_once_sent(
            &mut conn.borrow_mut(),
            || Conn::via_proxy(client),
            &method,
            params,
        )
    })
}

fn call_once_sent(
    slot: &mut Option<Conn>,
    mut connect: impl FnMut() -> Result<Conn, String>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    for attempt in 0..2 {
        if slot.is_none() {
            *slot = Some(connect()?);
        }
        let Some(live) = slot.as_mut() else { break };
        match live.call(method, params.clone()) {
            Err(CallError::NotSent(e)) if attempt == 1 => return Err(e),
            Err(CallError::NotSent(_)) => *slot = None,
            Err(CallError::Closed(e)) => {
                *slot = None;
                return Err(e);
            }
            Err(CallError::Rpc(e)) => return Err(e),
            Ok(v) => return Ok(v),
        }
    }
    Err("the daemon connection failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenPipe;
    impl Write for BrokenPipe {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn conn(writer: Box<dyn Write>) -> Conn {
        Conn {
            reader: Box::new(std::io::empty()),
            writer,
            decoder: Decoder::new(),
            next_id: 1,
            child: None,
        }
    }

    #[test]
    fn a_request_the_daemon_may_have_received_is_not_sent_again() {
        let mut connects = 0;
        let result = call_once_sent(
            &mut None,
            || {
                connects += 1;
                Ok(conn(Box::new(std::io::sink())))
            },
            "orch.task_create",
            json!({}),
        );
        assert!(result.is_err());
        assert_eq!(connects, 1, "EOF after the write must not trigger a resend");
    }

    #[test]
    fn a_request_that_could_not_be_written_is_retried_on_a_fresh_connection() {
        let mut connects = 0;
        let result = call_once_sent(
            &mut None,
            || {
                connects += 1;
                Ok(conn(if connects == 1 {
                    Box::new(BrokenPipe)
                } else {
                    Box::new(std::io::sink())
                }))
            },
            "orch.task_create",
            json!({}),
        );
        assert!(result.is_err(), "the second connection hits EOF");
        assert_eq!(connects, 2);
    }
}
