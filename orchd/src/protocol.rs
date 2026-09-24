//! NDJSON server over the unix socket: one request per line, one response
//! per line, `subscribe` pushes extra event lines on the same connection.
//! The actual method implementations live behind the [`Dispatcher`] trait so
//! this module only knows about framing, not engine/store internals.

use crate::model::Task;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event")]
pub enum Event {
    #[serde(rename = "task")]
    Task { task: Box<Task> },
    #[serde(rename = "log")]
    Log {
        #[serde(rename = "taskId")]
        task_id: String,
        attempt: u32,
        line: String,
    },
    /// The orchestrator chat thread of one repo, whole, after any change.
    #[serde(rename = "chat")]
    Chat { thread: Box<serde_json::Value> },
}

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    id: Option<String>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
    /// The control token (spec item A): required on every method except
    /// `ping`/`hook.stop`, which `Dispatcher::check_auth` exempts.
    #[serde(default)]
    auth: Option<String>,
}

pub type CallFuture<'a> =
    Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send + 'a>>;

/// Implemented by the engine's `App`: one async method call, a way to
/// subscribe to the task/log event broadcast, and the control-token check
/// gating both.
pub trait Dispatcher: Send + Sync {
    fn call<'a>(&'a self, method: String, params: serde_json::Value) -> CallFuture<'a>;
    fn subscribe(&self) -> broadcast::Receiver<Event>;
    fn check_auth(&self, method: &str, auth: Option<&str>) -> bool;
}

fn response_line(id: Option<&str>, result: Result<serde_json::Value, String>) -> String {
    let v = match result {
        Ok(result) => serde_json::json!({"id": id, "result": result}),
        Err(message) => serde_json::json!({"id": id, "error": {"message": message}}),
    };
    v.to_string()
}

/// Remove a stale socket file before binding (spec: "Remove a stale socket
/// file before binding").
pub fn remove_stale_socket(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub async fn serve(
    socket_path: &Path,
    dispatcher: Arc<dyn Dispatcher>,
    mut shutdown: broadcast::Receiver<()>,
) -> std::io::Result<()> {
    remove_stale_socket(socket_path)?;
    let listener = UnixListener::bind(socket_path)?;
    // Only this user may connect; the control token is the second gate.
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    }
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _addr) = accepted?;
                let dispatcher = dispatcher.clone();
                tokio::spawn(async move {
                    handle_connection(stream, dispatcher).await;
                });
            }
            _ = shutdown.recv() => {
                break;
            }
        }
    }
    let _ = std::fs::remove_file(socket_path);
    Ok(())
}

async fn handle_connection(stream: UnixStream, dispatcher: Arc<dyn Dispatcher>) {
    let (read_half, mut write_half) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    let writer_task = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if write_half.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            if write_half.write_all(b"\n").await.is_err() {
                break;
            }
            let _ = write_half.flush().await;
        }
    });

    let mut lines = BufReader::new(read_half).lines();
    loop {
        let next = lines.next_line().await;
        let line = match next {
            Ok(Some(l)) => l,
            _ => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(response_line(None, Err(format!("bad request: {e}"))));
                continue;
            }
        };

        if !dispatcher.check_auth(&req.method, req.auth.as_deref()) {
            let _ = tx.send(response_line(
                req.id.as_deref(),
                Err("unauthorized".to_string()),
            ));
            continue;
        }

        if req.method == "subscribe" {
            let mut events = dispatcher.subscribe();
            let tx_events = tx.clone();
            tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            if let Ok(s) = serde_json::to_string(&event) {
                                if tx_events.send(s).is_err() {
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            continue;
        }

        let result = dispatcher
            .call(req.method.clone(), req.params.clone())
            .await;
        let _ = tx.send(response_line(req.id.as_deref(), result));
    }

    drop(tx);
    let _ = writer_task.await;
}

/// Send one request over `socket_path` and return its `result` (or an
/// `Err` built from the response's `error.message`). Used by `orchd hook
/// stop` and by tests; opens a fresh connection per call, which is fine at
/// this call volume.
pub async fn client_request(
    socket_path: &Path,
    method: &str,
    params: serde_json::Value,
) -> std::io::Result<serde_json::Value> {
    let stream = UnixStream::connect(socket_path).await?;
    let (read_half, mut write_half) = stream.into_split();
    let id = uuid::Uuid::new_v4().to_string();
    let req = serde_json::json!({"id": id, "method": method, "params": params});
    write_half.write_all(req.to_string().as_bytes()).await?;
    write_half.write_all(b"\n").await?;
    write_half.flush().await?;

    let mut lines = BufReader::new(read_half).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(&line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if let Some(err) = v.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("classifier error")
                .to_string();
            return Err(std::io::Error::other(msg));
        }
        return Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        "connection closed before a response arrived",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_line_shapes_ok_and_error() {
        let ok = response_line(Some("1"), Ok(serde_json::json!({"pid": 1})));
        let v: serde_json::Value = serde_json::from_str(&ok).unwrap();
        assert_eq!(v["id"], "1");
        assert_eq!(v["result"]["pid"], 1);
        assert!(v.get("error").is_none());

        let err = response_line(Some("2"), Err("boom".to_string()));
        let v: serde_json::Value = serde_json::from_str(&err).unwrap();
        assert_eq!(v["id"], "2");
        assert_eq!(v["error"]["message"], "boom");
        assert!(v.get("result").is_none());
    }

    #[test]
    fn event_task_serializes_with_tag_and_task_field() {
        let task = crate::model::Task {
            id: "t1".into(),
            title: "T".into(),
            goal: "G".into(),
            criteria: vec![],
            verify: vec![],
            request: None,
            repo: "/r".into(),
            worktree: "/r-t".into(),
            branch: "task/t".into(),
            base_sha: "abc".into(),
            status: crate::model::TaskStatus::Running,
            tier: crate::model::Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            archived: false,
            created_at: 1,
            updated_at: 1,
        };
        let ev = Event::Task {
            task: Box::new(task),
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "task");
        assert_eq!(v["task"]["id"], "t1");
    }

    #[test]
    fn event_log_serializes_with_camel_case_task_id() {
        let ev = Event::Log {
            task_id: "t1".into(),
            attempt: 2,
            line: "building...".into(),
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "log");
        assert_eq!(v["taskId"], "t1");
        assert_eq!(v["attempt"], 2);
    }

    /// A fake dispatcher whose `check_auth` matches a single fixed token,
    /// exempting only `ping`/`hook.stop` -- exactly the real `App`'s rule,
    /// exercised here without any of its engine/store dependencies.
    struct FakeDispatcher {
        token: &'static str,
    }

    impl Dispatcher for FakeDispatcher {
        fn call<'a>(&'a self, method: String, _params: serde_json::Value) -> CallFuture<'a> {
            Box::pin(async move { Ok(serde_json::json!({"echo": method})) })
        }
        fn subscribe(&self) -> broadcast::Receiver<Event> {
            broadcast::channel(1).1
        }
        fn check_auth(&self, method: &str, auth: Option<&str>) -> bool {
            if method == "ping" || method == "hook.stop" {
                return true;
            }
            auth == Some(self.token)
        }
    }

    async fn request_line(socket_path: &Path, line: &str) -> serde_json::Value {
        let stream = UnixStream::connect(socket_path).await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        write_half.write_all(line.as_bytes()).await.unwrap();
        write_half.write_all(b"\n").await.unwrap();
        write_half.flush().await.unwrap();
        let mut lines = BufReader::new(read_half).lines();
        let response = lines.next_line().await.unwrap().unwrap();
        serde_json::from_str(&response).unwrap()
    }

    #[tokio::test]
    async fn a_request_without_the_control_token_is_rejected_as_unauthorized() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("orchd.sock");
        let dispatcher: Arc<dyn Dispatcher> = Arc::new(FakeDispatcher { token: "secret" });
        let (_shutdown_tx, shutdown_rx) = broadcast::channel(1);
        let sp = socket_path.clone();
        tokio::spawn(async move {
            let _ = serve(&sp, dispatcher, shutdown_rx).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // No `auth` field at all: rejected.
        let v = request_line(
            &socket_path,
            r#"{"id":"1","method":"task.list","params":{}}"#,
        )
        .await;
        assert_eq!(v["error"]["message"], "unauthorized");

        // Wrong token: rejected.
        let v = request_line(
            &socket_path,
            r#"{"id":"2","method":"task.list","params":{},"auth":"wrong"}"#,
        )
        .await;
        assert_eq!(v["error"]["message"], "unauthorized");

        // Right token: allowed through to the dispatcher.
        let v = request_line(
            &socket_path,
            r#"{"id":"3","method":"task.list","params":{},"auth":"secret"}"#,
        )
        .await;
        assert_eq!(v["result"]["echo"], "task.list");

        // `ping` needs no token at all.
        let v = request_line(&socket_path, r#"{"id":"4","method":"ping","params":{}}"#).await;
        assert_eq!(v["result"]["echo"], "ping");
    }
}
