//! Runtime ownership and startup/shutdown sequencing for the host facade.

use crate::api::dispatch::UNKNOWN_METHOD;
use crate::api::{CallError, Config};
use crate::engine::state::App;
use crate::legacy;
use serde_json::Value;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DRAIN: Duration = Duration::from_secs(10);

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

    /// Cancels task loops, chat turns, audits and background base runs, waits up to 10 s for them to end, then stops
    /// the runtime. Agent runs the pipeline may re-adopt after a restart are not killed here.
    pub async fn shutdown(&self) {
        self.shutdown_with(false).await;
    }

    /// Like `shutdown`; with `disabling` every agent run is killed instead of left for the
    /// next daemon.
    pub async fn shutdown_with(&self, disabling: bool) {
        self.app.shutdown_for(disabling);
        let app = self.app.clone();
        let drained = self.handle.spawn(async move {
            let deadline = tokio::time::Instant::now() + DRAIN;
            while (app.any_task_loop_running()
                || app.any_chat_turn_running()
                || app.any_audit_running()
                || app.any_base_job_running())
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
