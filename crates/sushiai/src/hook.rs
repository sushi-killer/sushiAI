//! `sushiai hook <event>`: the command an agent runs for each hook. It forwards the hook
//! JSON from stdin to the daemon and fails open: anything wrong means exit 0 with no output,
//! so the agent carries on (and a permission ask falls back to its own terminal prompt).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::Value;
use sushiai_agents::launch::permission_timeout;
use sushiai_daemon::ASK_WAIT_SECS;
use sushiai_protocol::{
    encode, method, Decoder, Frame, Hello, HookEvent, HookResult, Message, Request,
    PROTOCOL_VERSION,
};

/// Budget for a status-only event.
const ASYNC_BUDGET: Duration = Duration::from_secs(2);
/// A permission hook waits for the owner, as long as the hook timeout of the launch spec.
fn permission_budget() -> Duration {
    Duration::from_secs(permission_timeout(ASK_WAIT_SECS))
}
/// Hook payloads are small; a larger one is not forwarded.
const MAX_PAYLOAD: u64 = 1024 * 1024;

/// `args` are what follows `hook`: the event, then optionally `--agent NAME` (default claude).
pub fn run(args: impl Iterator<Item = String>) {
    let args: Vec<String> = args.collect();
    let event = args.first().map(String::as_str).unwrap_or_default();
    let agent = args
        .windows(2)
        .find(|w| w[0] == "--agent")
        .map_or("claude", |w| w[1].as_str());
    if let Some(answer) = forward(event, agent) {
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{answer}");
        let _ = out.flush();
    }
}

/// The JSON to print, if the daemon gave a decision.
fn forward(event: &str, agent: &str) -> Option<Value> {
    // A nested agent (a codex run by claude) inherits the session variables of its parent;
    // only the session's own agent may report.
    if std::env::var("SUSHIAI_AGENT").ok()? != agent {
        return None;
    }
    let socket = std::env::var("SUSHIAI_SOCKET").ok()?;
    let session = std::env::var("SUSHIAI_SESSION_ID").ok()?;
    let token = std::env::var("SUSHIAI_SESSION_TOKEN").ok()?;
    let mut raw = Vec::new();
    std::io::stdin()
        .take(MAX_PAYLOAD)
        .read_to_end(&mut raw)
        .ok()?;
    let payload: Value = serde_json::from_slice(&raw).ok()?;

    let waits = event == "permission";
    let budget = if waits {
        permission_budget()
    } else {
        ASYNC_BUDGET
    };
    let deadline = Instant::now() + budget;
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_write_timeout(Some(ASYNC_BUDGET)).ok()?;
    let hello = Hello {
        protocol: PROTOCOL_VERSION,
        client: "hook".into(),
        role: Some("hook".into()),
    };
    let params = HookEvent {
        session,
        token,
        event: event.into(),
        agent: agent.into(),
        payload,
    };
    stream
        .write_all(&encode(&Request::new(1, method::HELLO, hello).frame()))
        .ok()?;
    stream
        .write_all(&encode(
            &Request::new(2, method::HOOK_EVENT, params).frame(),
        ))
        .ok()?;

    let mut decoder = Decoder::new();
    let mut buf = [0u8; 8192];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        stream.set_read_timeout(Some(left)).ok()?;
        let n = stream.read(&mut buf).ok().filter(|n| *n > 0)?;
        for frame in decoder.push(&buf[..n]).ok()? {
            let Frame::Json(text) = frame else { continue };
            if let Ok(Message::Response(r)) = Message::parse(&text) {
                if r.id == 2 {
                    let result: HookResult = serde_json::from_value(r.result?).ok()?;
                    return result.answer;
                }
            }
        }
    }
}
