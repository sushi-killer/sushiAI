//! The composition point for the orchestration module: the only place that knows both the
//! daemon's `Module` trait and the `sushiai-orch` library. The daemon and the protocol never
//! learn what `orch` is.
//!
//! The module is opt-in. The daemon hosts it only when `<home>/modules/orch.enabled` exists
//! when the daemon starts; whoever enables the orchestrator (the desktop's extension toggle,
//! `sushiai orch register`) writes that file with [`set_enabled`] and restarts the daemon. A
//! daemon without the file has no `orch` capability and answers `orch.*` as unknown.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sushiai_daemon::{BoxFuture, Home, Module, ModuleNotify, ModuleSlot, Reply};
use sushiai_orch::{CallError, Config, Orch};
use sushiai_protocol::code;
use tokio::sync::watch;

const NAMESPACE: &str = "orch";
/// How long a call waits for the pipeline to finish starting.
const STARTUP_WAIT: Duration = Duration::from_secs(10);

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
}

impl OrchModule {
    /// Returns at once; the pipeline starts on a thread of its own because starting stops a
    /// legacy process and opens the store, which block.
    fn start(home: PathBuf, notify: ModuleNotify) -> OrchModule {
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
