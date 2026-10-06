//! `sushiai open <target> <path>`: asks the desktop to open `path` with `target` (for example
//! `preview/files`). It reports as the agent's session (token from the environment) over a
//! hook-role connection. It fails open: anything wrong is a note on stderr and exit 0.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use sushiai_protocol::{
    encode, method, Decoder, Frame, Hello, HookOpen, Message, Request, PROTOCOL_VERSION,
};

const BUDGET: Duration = Duration::from_secs(2);

/// `args` are what follows `open`: the target, then the path.
pub fn run(args: impl Iterator<Item = String>) {
    let args: Vec<String> = args.collect();
    if let Err(note) = send(&args) {
        eprintln!("sushiai open: {note}");
    }
}

fn send(args: &[String]) -> Result<(), String> {
    let [target, path] = args else {
        return Err("usage: sushiai open TARGET PATH".into());
    };
    let var = |name: &str| {
        std::env::var(name).map_err(|_| "not running inside a sushiai session".to_string())
    };
    let (socket, session, token) = (
        var("SUSHIAI_SOCKET")?,
        var("SUSHIAI_SESSION_ID")?,
        var("SUSHIAI_SESSION_TOKEN")?,
    );
    let arg = std::path::absolute(path)
        .map_err(|e| format!("cannot resolve {path}: {e}"))?
        .to_string_lossy()
        .into_owned();
    let io = |e: std::io::Error| format!("daemon: {e}");
    let mut stream = UnixStream::connect(socket).map_err(io)?;
    stream.set_write_timeout(Some(BUDGET)).map_err(io)?;
    let hello = Hello {
        protocol: PROTOCOL_VERSION,
        client: "open".into(),
        role: Some("hook".into()),
    };
    let params = HookOpen {
        session,
        token,
        target: target.clone(),
        arg,
    };
    for request in [
        Request::new(1, method::HELLO, hello),
        Request::new(2, method::HOOK_OPEN, params),
    ] {
        stream.write_all(&encode(&request.frame())).map_err(io)?;
    }
    let deadline = Instant::now() + BUDGET;
    let mut decoder = Decoder::new();
    let mut buf = [0u8; 8192];
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("daemon did not answer")?;
        stream.set_read_timeout(Some(left)).map_err(io)?;
        let n = stream.read(&mut buf).map_err(io)?;
        if n == 0 {
            return Err("daemon closed the connection".into());
        }
        for frame in decoder.push(&buf[..n]).map_err(|e| e.to_string())? {
            let Frame::Json(text) = frame else { continue };
            if let Ok(Message::Response(r)) = Message::parse(&text) {
                if r.id == 2 {
                    return match r.error {
                        Some(e) => Err(e.message),
                        None => Ok(()),
                    };
                }
            }
        }
    }
}
