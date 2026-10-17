//! The composition point for the orchestration module: the only place that knows both the
//! daemon's `Module` trait and the `sushiai-orch` library. The daemon and the protocol never
//! learn what `orch` is.
//!
//! The module is opt-in. The daemon hosts it only when `<home>/modules/orch.enabled` exists
//! when the daemon starts; whoever enables the orchestrator (the desktop's extension toggle,
//! `sushiai orch register`) writes that file with [`set_enabled`] and restarts the daemon. A
//! daemon without the file has no `orch` capability and answers `orch.*` as unknown.
//!
//! It also carries the relay a client uses to reach another host's orchestrator: the daemon
//! holds no host list or connection of its own, so `orch.relay.call` becomes one `orch.relay`
//! notification to the listening desktop app, whose `orch.relay.reply` is the answer.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sushiai_daemon::{BoxFuture, Home, Module, ModuleNotify, ModuleSlot, Reply};
use sushiai_orch::{CallError, Config, Orch};
use sushiai_protocol::code;
use tokio::sync::{oneshot, watch};

const NAMESPACE: &str = "orch";
/// How long a call waits for the pipeline to finish starting.
const STARTUP_WAIT: Duration = Duration::from_secs(10);
/// How long a relayed call waits for the app's answer; opening a pull request pushes first.
const RELAY_WAIT: Duration = Duration::from_secs(60);
const RELAY_PR_WAIT: Duration = Duration::from_secs(660);

/// What the start thread reports: the running pipeline, or why it did not start.
type Started = Option<Result<Arc<Orch>, String>>;

fn flag(home: &Path) -> PathBuf {
    home.join("modules").join(format!("{NAMESPACE}.enabled"))
}

/// Whether the daemon of `home` hosts the orchestrator.
pub fn enabled(home: &Path) -> bool {
    flag(home).is_file()
}

/// Turns hosting on or off for the next daemon start.
/// Used by `sushiai orch register` and `unregister`.
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

/// The modules `main` passes to the daemon: the orchestrator only when it is enabled.
pub fn slots(home: &Home) -> Vec<ModuleSlot> {
    if !enabled(home.dir()) {
        return Vec::new();
    }
    let home = home.dir().to_path_buf();
    vec![ModuleSlot::new(NAMESPACE, move |notify| {
        Arc::new(OrchModule::start(home, notify))
    })]
}

struct OrchModule {
    home: PathBuf,
    started: watch::Receiver<Started>,
    relay: Arc<Relay>,
}

/// The relayed calls waiting for the app's `orch.relay.reply`, by relay id.
struct Relay {
    /// Sends the `orch.relay` notification.
    send: Box<dyn Fn(Value) + Send + Sync>,
    /// How long a call waits: `task.pr`, and every other method.
    pr_wait: Duration,
    wait: Duration,
    next: AtomicU64,
    waiting: Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
}

impl Relay {
    fn new(
        send: impl Fn(Value) + Send + Sync + 'static,
        wait: Duration,
        pr_wait: Duration,
    ) -> Relay {
        Relay {
            send: Box::new(send),
            wait,
            pr_wait,
            next: AtomicU64::new(1),
            waiting: Mutex::new(HashMap::new()),
        }
    }

    /// A relay id no earlier call of this daemon used, nor one of a daemon before it.
    fn fresh_id(&self) -> String {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        format!("{}-{nanos:x}-{n}", std::process::id())
    }

    fn waiting(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Sender<Result<Value, String>>>> {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `orch.relay.call {host?, method, params?}`: one `orch.relay` notification, then the
    /// app's answer, or an error once nobody answered in time.
    async fn call(self: Arc<Self>, params: Value) -> Reply {
        let method = match params.get("method") {
            Some(Value::String(m)) if !m.is_empty() => m.clone(),
            _ => {
                return Err((
                    code::INVALID_PARAMS,
                    "orch.relay.call needs a method".into(),
                ))
            }
        };
        let host = params.get("host").cloned().unwrap_or(Value::Null);
        let inner = params.get("params").cloned().unwrap_or_else(|| json!({}));
        let wait = if method == "task.pr" {
            self.pr_wait
        } else {
            self.wait
        };
        let id = self.fresh_id();
        let (tx, rx) = oneshot::channel();
        self.waiting().insert(id.clone(), tx);
        // Taken back whatever happens to this call, so a late reply is refused.
        let _forget = Forget(&self, &id);
        let note = json!({"id": id, "host": host, "method": method, "params": inner});
        (self.send)(note);
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(message))) => Err((code::INVALID_REQUEST, message)),
            Ok(Err(_)) | Err(_) => Err((
                code::INVALID_REQUEST,
                format!(
                    "no answer for {method} on another host: the sushiai desktop app must be \
                     running, with the Orchestrator on, to reach it"
                ),
            )),
        }
    }

    /// `orch.relay.reply {id, result | error}`: hands the answer to the call waiting for it.
    fn reply(&self, params: Value) -> Reply {
        let Some(id) = params.get("id").and_then(Value::as_str) else {
            return Err((code::INVALID_PARAMS, "orch.relay.reply needs an id".into()));
        };
        let Some(tx) = self.waiting().remove(id) else {
            return Err((
                code::INVALID_REQUEST,
                "no relayed call waits for this id".into(),
            ));
        };
        let answer = match params.get("error") {
            Some(Value::Null) | None => Ok(params.get("result").cloned().unwrap_or(Value::Null)),
            Some(Value::String(text)) => Err(text.clone()),
            Some(other) => Err(other.to_string()),
        };
        // The caller may be gone; its answer has nowhere to go then.
        let _ = tx.send(answer);
        Ok(Value::Null)
    }
}

/// Removes a relay id from the waiting calls when its call ends.
struct Forget<'a>(&'a Relay, &'a str);

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        self.0.waiting().remove(self.1);
    }
}

impl OrchModule {
    /// Returns at once; the pipeline starts on a thread of its own because starting stops a
    /// legacy process and opens the store, which block.
    fn start(home: PathBuf, notify: ModuleNotify) -> OrchModule {
        let relay = Arc::new(Relay::new(
            {
                let notify = notify.clone();
                move |note| notify.send("relay", note)
            },
            RELAY_WAIT,
            RELAY_PR_WAIT,
        ));
        let (tx, started) = watch::channel(None);
        let flag_home = home.clone();
        let spawned = std::thread::Builder::new()
            .name("orch-start".into())
            .spawn({
                let tx = tx.clone();
                move || {
                    let result = Orch::start(Config {
                        data_dir: home.join("orchestrator"),
                        exe: executable(&home),
                        emit: Arc::new(move |event| notify.send("event", event)),
                        home,
                    })
                    .map(Arc::new)
                    .map_err(|e| e.to_string());
                    if let Err(message) = &result {
                        eprintln!("orch: failed to start: {message}");
                    }
                    let _ = tx.send(Some(result));
                }
            });
        if let Err(e) = spawned {
            let _ = tx.send(Some(Err(e.to_string())));
        }
        OrchModule {
            home: flag_home,
            started,
            relay,
        }
    }

    /// The running pipeline, once it started.
    async fn ready(started: &watch::Receiver<Started>) -> Result<Arc<Orch>, (i64, String)> {
        let mut started = started.clone();
        let state = tokio::time::timeout(STARTUP_WAIT, started.wait_for(Option::is_some))
            .await
            .map_err(|_| {
                (
                    code::MODULE_STARTING,
                    "the orchestrator is still starting".to_string(),
                )
            })?
            .map_err(|_| (code::INTERNAL, "the orchestrator stopped".to_string()))?
            .clone();
        match state {
            Some(Ok(orch)) => Ok(orch),
            Some(Err(message)) => Err((
                code::INTERNAL,
                format!("the orchestrator failed to start: {message}"),
            )),
            None => Err((code::INTERNAL, "the orchestrator stopped".to_string())),
        }
    }
}

/// The `sushiai` agents run: the home's stable link when the daemon made one (it survives an
/// app update), else the running executable.
fn executable(home: &Path) -> PathBuf {
    let link = home.join("bin").join("sushiai");
    if link.exists() {
        return link;
    }
    std::env::current_exe().unwrap_or(link)
}

impl Module for OrchModule {
    fn namespace(&self) -> &'static str {
        NAMESPACE
    }

    fn capability(&self) -> &'static str {
        NAMESPACE
    }

    fn call(&self, method: &str, params: Value) -> BoxFuture<Reply> {
        // The relay never reaches the pipeline.
        match method {
            "relay.call" => return Box::pin(self.relay.clone().call(params)),
            "relay.reply" => {
                let reply = self.relay.reply(params);
                return Box::pin(async move { reply });
            }
            _ => {}
        }
        let method = method.to_string();
        let started = self.started.clone();
        Box::pin(async move {
            let orch = Self::ready(&started).await?;
            orch.call(&method, params).await.map_err(|e| match e {
                CallError::UnknownMethod(name) => (
                    code::METHOD_NOT_FOUND,
                    format!("unknown method {NAMESPACE}.{name}"),
                ),
                CallError::Failed(message) => (code::INVALID_REQUEST, message),
                CallError::Panicked(message) => (code::INTERNAL, message),
            })
        })
    }

    fn shutdown(&self) -> BoxFuture<()> {
        let started = self.started.clone();
        // The switch-off removes the flag before the daemon stops: no next daemon adopts a run.
        let disabling = !enabled(&self.home);
        Box::pin(async move {
            if let Ok(orch) = Self::ready(&started).await {
                orch.shutdown_with(disabling).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A relay whose notifications land in the returned list.
    fn relay(wait: Duration, pr_wait: Duration) -> (Arc<Relay>, Arc<Mutex<Vec<Value>>>) {
        let notes = Arc::new(Mutex::new(Vec::new()));
        let sink = notes.clone();
        let relay = Relay::new(move |note| sink.lock().unwrap().push(note), wait, pr_wait);
        (Arc::new(relay), notes)
    }

    async fn note(notes: &Mutex<Vec<Value>>) -> Value {
        loop {
            if let Some(note) = notes.lock().unwrap().first().cloned() {
                return note;
            }
            tokio::time::sleep(Duration::from_millis(5)).await; // wait-allow: the poll interval of an async wait on the first note; crates/test_wait.rs has only a blocking wait, which would stall this runtime
        }
    }

    #[tokio::test]
    async fn a_call_is_one_notification_and_the_reply_is_its_answer() {
        let (relay, notes) = relay(Duration::from_secs(5), Duration::from_secs(5));
        let params = json!({"host": "ssh:lab", "method": "task.get", "params": {"id": "t1"}});
        let call = tokio::spawn(relay.clone().call(params));
        let sent = note(&notes).await;
        assert_eq!(sent["host"], "ssh:lab");
        assert_eq!(sent["method"], "task.get");
        assert_eq!(sent["params"], json!({"id": "t1"}));
        let id = sent["id"].clone();
        relay
            .reply(json!({"id": id, "result": {"id": "t1"}}))
            .unwrap();
        assert_eq!(call.await.unwrap().unwrap(), json!({"id": "t1"}));
        // Answered once: a second reply is refused, as is one nobody asked for.
        assert!(relay.reply(json!({"id": id, "result": 1})).is_err());
        assert!(relay.reply(json!({"id": "no-such-relay"})).is_err());
        assert_eq!(notes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_error_reply_fails_the_call_with_its_text() {
        let (relay, notes) = relay(Duration::from_secs(5), Duration::from_secs(5));
        let call = tokio::spawn(relay.clone().call(json!({"method": "hosts.list"})));
        let id = note(&notes).await["id"].clone();
        relay
            .reply(json!({"id": id, "error": "Lab is not connected"}))
            .unwrap();
        let (_, message) = call.await.unwrap().unwrap_err();
        assert_eq!(message, "Lab is not connected");
    }

    #[tokio::test]
    async fn an_unanswered_call_ends_naming_the_desktop_app_and_task_pr_waits_longer() {
        let (relay, notes) = relay(Duration::from_millis(50), Duration::from_secs(5));
        let (_, message) = relay
            .clone()
            .call(json!({"host": "local", "method": "task.list"}))
            .await
            .unwrap_err();
        assert!(message.contains("desktop app must be running"), "{message}");
        let late = note(&notes).await["id"].clone();
        assert!(relay.reply(json!({"id": late, "result": []})).is_err());
        let pr = tokio::spawn(
            relay
                .clone()
                .call(json!({"host": "local", "method": "task.pr"})),
        );
        tokio::time::sleep(Duration::from_millis(200)).await; // wait-allow: absence window; the call must stay pending past it (no async holds_for in crates/test_wait.rs)
        assert!(!pr.is_finished());
        let id = notes.lock().unwrap()[1]["id"].clone();
        relay
            .reply(json!({"id": id, "result": {"url": "u"}}))
            .unwrap();
        assert_eq!(pr.await.unwrap().unwrap(), json!({"url": "u"}));
    }

    #[tokio::test]
    async fn a_call_without_a_method_sends_nothing() {
        let (relay, notes) = relay(Duration::from_secs(5), Duration::from_secs(5));
        assert!(relay.clone().call(json!({"host": "local"})).await.is_err());
        assert!(notes.lock().unwrap().is_empty());
    }
}
