//! `sushiai hook <event>`: the command an agent runs for each hook. It forwards the hook
//! JSON from stdin to the daemon and fails open: anything wrong means exit 0 with no output,
//! so the agent carries on (and a permission ask falls back to its own terminal prompt).
//!
//! `SUSHIAI_HOOK_LOG=<file>` turns on a debug trail: one appended line per run with the
//! event, the agent argument, which variables are set (names only), the step the hook
//! reached, the connect result and the daemon reply code. No value and no payload is logged.

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
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
    let mut trail = Trail::default();
    let answer = forward(event, agent, &mut trail);
    trail.write(event, agent);
    if let Some(answer) = answer {
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{answer}");
        let _ = out.flush();
    }
}

/// Variables the hook reads, logged by name only.
const TRAIL_VARS: &[&str] = &[
    "SUSHIAI_AGENT",
    "SUSHIAI_SOCKET",
    "SUSHIAI_SESSION_ID",
    "SUSHIAI_SESSION_TOKEN",
    "SUSHIAI_HOME",
];

/// What one run did, for the opt-in debug trail.
#[derive(Default)]
struct Trail {
    /// The last step the hook reached (`replied` when the daemon answered).
    step: &'static str,
    connect: Option<String>,
    reply: Option<String>,
}

impl Trail {
    fn write(&self, event: &str, agent: &str) {
        let Some(path) = std::env::var_os("SUSHIAI_HOOK_LOG").filter(|p| !p.is_empty()) else {
            return;
        };
        let vars: Vec<String> = TRAIL_VARS
            .iter()
            .map(|name| {
                let set = std::env::var_os(name).is_some_and(|v| !v.is_empty());
                format!("{name}={}", if set { "set" } else { "unset" })
            })
            .collect();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let line = format!(
            "{ts} pid={} event={} agent={} {} step={} connect={} reply={}\n",
            std::process::id(),
            token(event),
            token(agent),
            vars.join(" "),
            if self.step.is_empty() {
                "start"
            } else {
                self.step
            },
            self.connect.as_deref().unwrap_or("-"),
            self.reply.as_deref().unwrap_or("-"),
        );
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path);
        if let Ok(mut file) = file {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

/// A command-line argument as a short, single-word token.
fn token(s: &str) -> String {
    s.chars()
        .take(40)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The JSON to print, if the daemon gave a decision.
fn forward(event: &str, agent: &str, trail: &mut Trail) -> Option<Value> {
    // A nested agent (a codex run by claude) inherits the session variables of its parent;
    // only the session's own agent may report.
    trail.step = "env";
    if std::env::var("SUSHIAI_AGENT").ok()? != agent {
        trail.step = "agent-mismatch";
        return None;
    }
    let socket = std::env::var("SUSHIAI_SOCKET").ok()?;
    let session = std::env::var("SUSHIAI_SESSION_ID").ok()?;
    let token = std::env::var("SUSHIAI_SESSION_TOKEN").ok()?;
    trail.step = "payload";
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
    trail.step = "connect";
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(e) => {
            trail.connect = Some(format!("{:?}", e.kind()));
            return None;
        }
    };
    trail.connect = Some("ok".into());
    trail.step = "send";
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

    trail.step = "wait";
    let mut decoder = Decoder::new();
    let mut buf = [0u8; 8192];
    loop {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            trail.reply = Some("timeout".into());
            return None;
        };
        stream.set_read_timeout(Some(left)).ok()?;
        let n = match stream.read(&mut buf) {
            Ok(0) => {
                trail.reply = Some("eof".into());
                return None;
            }
            Ok(n) => n,
            Err(e) => {
                trail.reply = Some(format!("{:?}", e.kind()));
                return None;
            }
        };
        for frame in decoder.push(&buf[..n]).ok()? {
            let Frame::Json(text) = frame else { continue };
            if let Ok(Message::Response(r)) = Message::parse(&text) {
                if r.id == 2 {
                    trail.step = "replied";
                    if let Some(e) = &r.error {
                        trail.reply = Some(format!("error:{}", e.code));
                        return None;
                    }
                    trail.reply = Some("ok".into());
                    let result: HookResult = serde_json::from_value(r.result?).ok()?;
                    return result.answer;
                }
            }
        }
    }
}
