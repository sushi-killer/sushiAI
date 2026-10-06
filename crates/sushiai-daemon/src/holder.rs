//! Daemon side of the holder protocol, and spawning holders.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use sushiai_core::Screen;
use sushiai_protocol::{
    method, Frame, HoldAttach, HoldAttachResult, HoldClose, HoldInput, HoldResize, Message,
    Request, Signal,
};
use tokio::io::AsyncWriteExt;
use tokio::net::unix::OwnedReadHalf;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::framed::FrameReader;
use crate::{Error, Result};

const WRITE_QUEUE: usize = 256;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

pub enum SendError {
    /// The writer queue is full: the holder is not taking requests.
    Full,
    Closed,
}

/// A connection to one holder. Writes go through a separate task and a bounded queue, so
/// the owner never waits on the socket and can always keep reading.
pub struct HolderConn {
    pub reader: FrameReader<OwnedReadHalf>,
    writes: mpsc::Sender<Vec<u8>>,
    next_id: u64,
    /// The holder's pid, known once an attach was answered.
    pub holder_pid: Option<u32>,
}

impl HolderConn {
    pub async fn connect(sock: &Path) -> Result<HolderConn> {
        let (read, mut write) = UnixStream::connect(sock).await?.into_split();
        let (writes, mut queue) = mpsc::channel::<Vec<u8>>(WRITE_QUEUE);
        tokio::spawn(async move {
            while let Some(bytes) = queue.recv().await {
                if write.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        });
        Ok(HolderConn {
            reader: FrameReader::new(read),
            writes,
            next_id: 1,
            holder_pid: None,
        })
    }

    /// Queues a request without waiting and returns its id.
    pub fn request(
        &mut self,
        name: &str,
        params: impl serde::Serialize,
    ) -> std::result::Result<u64, SendError> {
        let id = self.next_id;
        let bytes = sushiai_protocol::encode(&Request::new(id, name, params).frame());
        match self.writes.try_send(bytes) {
            Ok(()) => {
                self.next_id += 1;
                Ok(id)
            }
            Err(mpsc::error::TrySendError::Full(_)) => Err(SendError::Full),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(SendError::Closed),
        }
    }

    pub fn input(&mut self, data: Vec<u8>) -> std::result::Result<u64, SendError> {
        self.request(method::HOLD_INPUT, HoldInput { data })
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> std::result::Result<u64, SendError> {
        self.request(method::HOLD_RESIZE, HoldResize { cols, rows })
    }

    pub fn close(&mut self, signal: Signal) -> std::result::Result<u64, SendError> {
        self.request(method::HOLD_CLOSE, HoldClose { signal })
    }

    /// Attaches from the start of the ring and feeds the replay into `screen`.
    /// Returns the stream offset reached; live output continues from there.
    pub async fn attach_and_replay(&mut self, screen: &mut Screen) -> Result<u64> {
        self.request(method::HOLD_ATTACH, HoldAttach { from_seq: 0 })
            .map_err(|_| Error::Holder("holder is not taking requests".into()))?;
        timeout(Duration::from_secs(10), self.replay(screen))
            .await
            .map_err(|_| Error::Holder("replay timed out".into()))?
    }

    async fn replay(&mut self, screen: &mut Screen) -> Result<u64> {
        let mut head: Option<HoldAttachResult> = None;
        let mut seq = 0;
        loop {
            if let Some(h) = &head {
                if seq >= h.next_seq {
                    return Ok(seq);
                }
            }
            let Some(frame) = self.reader.next().await? else {
                // A holder drops a client that cannot keep up with a flood. With the attach
                // answered, stop here: the actor sees the same EOF and reconnects from `seq`.
                return match head {
                    Some(_) => Ok(seq),
                    None => Err(Error::Holder("holder closed during attach".into())),
                };
            };
            match frame {
                Frame::Json(text) => {
                    if let Ok(Message::Response(r)) = Message::parse(&text) {
                        let result = r
                            .result
                            .ok_or_else(|| Error::Holder("attach refused".into()))?;
                        let parsed: HoldAttachResult = serde_json::from_value(result)?;
                        self.holder_pid = Some(parsed.pid).filter(|p| *p != 0);
                        seq = parsed.from_seq;
                        head = Some(parsed);
                    }
                }
                Frame::Output { seq: at, data, .. } => {
                    screen.feed(&data);
                    seq = at + data.len() as u64;
                }
            }
        }
    }
}

pub struct Spawn<'a> {
    pub id: &'a str,
    pub dir: &'a Path,
    pub cols: u16,
    pub rows: u16,
    pub cwd: &'a str,
    pub cmd: &'a [String],
    /// Variables for the child. They ride in the holder's environment, never its argv.
    pub env: &'a [(String, String)],
}

/// Starts `sushiai hold` in its own session, so it survives this daemon, and returns its pid.
/// The holder reports a startup failure on stderr and closes stderr once it runs, so the
/// error text (bad command, bad cwd) reaches the caller.
pub async fn spawn(spec: &Spawn<'_>) -> Result<u32> {
    let mut command = Command::new(crate::binlink::real_exe()?);
    command
        .args(["hold", "--id", spec.id, "--dir"])
        .arg(spec.dir)
        .args([
            "--cols",
            &spec.cols.to_string(),
            "--rows",
            &spec.rows.to_string(),
        ])
        .args(["--cwd", spec.cwd, "--"])
        .args(spec.cmd)
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // SAFETY: setsid(2) is async-signal-safe and touches no Rust state between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let pid = child.id();
    let stderr = child.stderr.take();
    // Reap the holder if it exits while this daemon is alive.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    let read = tokio::task::spawn_blocking(move || {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    });
    let text = match timeout(STARTUP_TIMEOUT, read).await {
        Ok(Ok(text)) => text,
        _ => {
            kill_group(pid);
            return Err(Error::Holder("holder did not report startup".into()));
        }
    };
    if text.trim().is_empty() {
        Ok(pid)
    } else {
        Err(Error::Holder(text.trim().to_string()))
    }
}

/// SIGKILLs a holder's process group (the holder is its own session leader).
pub fn kill_group(pid: u32) {
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: kill(2) takes plain integers; the pid is a holder this daemon started.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}
