use super::polling::{deadline, wait_until};
use super::process::{has_exited, spawned_pids, DAEMON_LOG};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

/// Why a request got no answer: the tail of the daemon's log, when there is one.
fn no_answer(socket: &Path, method: &str) -> String {
    format!(
        "the daemon closed the connection without answering {method}; {}",
        log_tail(socket)
    )
}

fn log_tail(socket: &Path) -> String {
    let log = std::fs::read_to_string(socket.with_file_name(DAEMON_LOG)).unwrap_or_default();
    let tail: Vec<&str> = log.lines().rev().take(40).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    format!("log tail:\n{}", tail.join("\n"))
}

/// Waits until the daemon of `socket` accepts a connection. A daemon that exits first fails
/// the wait at once, with its log tail.
pub fn wait_for_socket(socket: &Path) {
    let pid = spawned_pids().lock().unwrap().get(socket).copied();
    wait_until("the daemon to open its socket", || {
        if socket.exists() && UnixStream::connect(socket).is_ok() {
            return Some(());
        }
        if pid.is_some_and(has_exited) {
            panic!(
                "the daemon exited before opening its socket; {}",
                log_tail(socket)
            );
        }
        None
    });
}

/// One JSON frame: a 4-byte big-endian length (kind byte included), the kind `J`, the text.
fn frame(text: &str) -> Vec<u8> {
    let mut out = ((text.len() + 1) as u32).to_be_bytes().to_vec();
    out.push(b'J');
    out.extend_from_slice(text.as_bytes());
    out
}

/// Opens a connection, says `hello`, sends `orch.<method>` and returns the response envelope
/// (`result` or `error`). `None` when the daemon closed the connection first.
fn exchange(socket: &Path, method: &str, params: serde_json::Value) -> Option<serde_json::Value> {
    let mut stream = UnixStream::connect(socket).expect("connect to the daemon socket");
    let hello = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "hello",
        "params": {"protocol": 1, "client": "orch-test"}});
    let call = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": format!("orch.{method}"),
        "params": params});
    stream.write_all(&frame(&hello.to_string())).unwrap();
    stream.write_all(&frame(&call.to_string())).unwrap();
    stream.flush().unwrap();
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        while pending.len() >= 4 {
            let len = u32::from_be_bytes(pending[..4].try_into().unwrap()) as usize;
            if pending.len() < 4 + len {
                break;
            }
            let body: Vec<u8> = pending.drain(..4 + len).skip(5).collect();
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
                if v["id"] == 2 {
                    return Some(v);
                }
            }
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(n) => pending.extend_from_slice(&buf[..n]),
        }
    }
}

/// Asks the daemon to stop. It may exit before it answers.
pub fn stop_daemon(socket: &Path) {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return;
    };
    let hello = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "hello",
        "params": {"protocol": 1, "client": "orch-test"}});
    let stop = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "daemon.shutdown",
        "params": {}});
    let _ = stream.set_read_timeout(Some(deadline()));
    let _ = stream.write_all(&frame(&hello.to_string()));
    let _ = stream.write_all(&frame(&stop.to_string()));
    let _ = stream.flush();
    let mut sink = [0u8; 4096];
    while matches!(stream.read(&mut sink), Ok(n) if n > 0) {}
}

/// The `result` of `orch.<method>`; panics on an error answer.
pub fn request_on(socket: &Path, method: &str, params: serde_json::Value) -> serde_json::Value {
    let Some(v) = exchange(socket, method, params) else {
        panic!("{}", no_answer(socket, method));
    };
    if let Some(err) = v.get("error") {
        panic!("{method} failed: {err}");
    }
    v.get("result").cloned().unwrap_or(serde_json::Value::Null)
}

/// The whole response envelope of `orch.<method>`, error or not.
pub fn raw_request_with_params(
    socket: &Path,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    exchange(socket, method, params).unwrap_or_else(|| panic!("{}", no_answer(socket, method)))
}
