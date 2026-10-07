//! sushiAI task orchestration pipeline (was the `orchd` daemon). It has no process, socket or
//! lock of its own: a host starts [`Orch`], forwards `orch.*` calls to [`Orch::call`] and sends
//! every value passed to the `emit` callback to its clients as `orch.event`.

pub mod ab;
pub mod brief;
pub mod costs;
pub mod engine;
pub mod eval;
pub mod events;
pub mod evolve;
pub mod git;
pub mod harness;
pub mod hook;
pub mod legacy;
pub mod loop_detect;
pub mod mcp;
pub mod model;
pub mod prompts;
pub mod report;
pub mod skill;
pub mod store;
pub mod timeline;

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

use engine::{App, UNKNOWN_METHOD};

/// How long [`Orch::shutdown`] waits for cancelled loops, chat turns and audits to end.
const DRAIN: Duration = Duration::from_secs(10);

/// Where one orchestrator lives and how it talks to its host.
pub struct Config {
    /// The orchestrator's own folder, normally `<home>/orchestrator`.
    pub data_dir: PathBuf,
    /// The sushiAI home. Agents started by the pipeline get it as `SUSHIAI_HOME`, so their
    /// `sushiai mcp` and `sushiai orch hook` reach this host's daemon.
    pub home: PathBuf,
    /// The `sushiai` executable agents run as `mcp` and `orch hook`.
    pub exe: PathBuf,
    /// Receives every event the pipeline broadcasts, as JSON tagged by `event`. Must not block.
    pub emit: Arc<dyn Fn(Value) + Send + Sync>,
}

/// Why a call did not return a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The method does not exist.
    UnknownMethod(String),
    /// The pipeline refused or failed the call; the text is for the person.
    Failed(String),
    /// The call panicked. The pipeline keeps running.
    Panicked(String),
}

pub type CallFuture = Pin<Box<dyn Future<Output = Result<Value, CallError>> + Send + 'static>>;

/// A running orchestration pipeline. It owns a tokio runtime, so its blocking git and process
/// work never takes threads from the host's runtime.
pub struct Orch {
    app: Arc<App>,
    runtime: Mutex<Option<tokio::runtime::Runtime>>,
    handle: tokio::runtime::Handle,
}

impl Orch {
    /// Stops a legacy `orchd`, opens the store, requeues what a restart interrupted and starts
    /// forwarding events. Blocks (a legacy process gets up to 10 s to stop): call it from a
    /// thread of its own, never from an async task.
    pub fn start(config: Config) -> io::Result<Orch> {
        legacy::stop(&config.data_dir);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("orch")
            .enable_all()
            .build()?;
        let handle = runtime.handle().clone();
        let _inside = runtime.enter();
        let app = App::new(
            config.data_dir,
            config.home,
            config.exe.to_string_lossy().into_owned(),
        )?;
        // Subscribe before recovery so its events are not missed.
        let mut events = app.subscribe();
        let emit = config.emit;
        handle.spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => {
                        if let Ok(value) = serde_json::to_value(&event) {
                            emit(value);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        if let Err(e) = app.recover_on_start() {
            eprintln!("orch: restart recovery failed: {e}");
        }
        Ok(Orch {
            app,
            runtime: Mutex::new(Some(runtime)),
            handle,
        })
    }

    /// Runs `<method>` (the text after `orch.`) on the pipeline's runtime. The returned future
    /// does not borrow `self` and may be awaited on any runtime.
    pub fn call(&self, method: &str, params: Value) -> CallFuture {
        let app = self.app.clone();
        let method = method.to_string();
        let task = self
            .handle
            .spawn(async move { app.dispatch(&method, params).await });
        Box::pin(async move {
            match task.await {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(message)) => match message.strip_prefix(UNKNOWN_METHOD) {
                    Some(name) => Err(CallError::UnknownMethod(name.to_string())),
                    None => Err(CallError::Failed(message)),
                },
                Err(e) => Err(CallError::Panicked(e.to_string())),
            }
        })
    }

    /// Cancels task loops, chat turns and audits, waits up to 10 s for them to end, then stops
    /// the runtime. Agent runs the pipeline may re-adopt after a restart are not killed here.
    pub async fn shutdown(&self) {
        self.app.shutdown();
        let app = self.app.clone();
        let drained = self.handle.spawn(async move {
            let deadline = tokio::time::Instant::now() + DRAIN;
            while (app.any_task_loop_running()
                || app.any_chat_turn_running()
                || app.any_audit_running())
                && tokio::time::Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        let _ = drained.await;
        self.stop_runtime();
    }

    fn stop_runtime(&self) {
        if let Some(runtime) = self.runtime.lock().ok().and_then(|mut r| r.take()) {
            // Dropping a runtime inside an async task panics; this returns at once.
            runtime.shutdown_background();
        }
    }
}

impl Drop for Orch {
    fn drop(&mut self) {
        self.stop_runtime();
    }
}
