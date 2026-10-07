//! Modules: libraries the daemon hosts behind one method prefix.
//!
//! The daemon knows only this trait. A request `<namespace>.<rest>` goes to the module with
//! that namespace as `call(rest, params)`; the module announces itself with its capability in
//! `hello` and pushes notifications through [`ModuleNotify`]. Nothing here knows what a module
//! does: without one the daemon behaves as before.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use sushiai_protocol::Notification;
use tokio::sync::broadcast;

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// A module's answer: the result, or `(code, message)`.
pub type Reply = Result<Value, (i64, String)>;

pub trait Module: Send + Sync + 'static {
    /// The method prefix, without the dot ("tasks" for `tasks.list`).
    fn namespace(&self) -> &'static str;
    /// The entry the module adds to `hello.capabilities`.
    fn capability(&self) -> &'static str;
    /// Handles `<namespace>.<method>`. A panic becomes an INTERNAL error for that request.
    fn call(&self, method: &str, params: Value) -> BoxFuture<Reply>;
    /// Called once when the daemon stops, after the socket is gone. Bounded by the daemon.
    fn shutdown(&self) -> BoxFuture<()>;
}

/// Sends notifications to every connected non-hook client. It is the daemon's own bounded
/// broadcast (256 entries): a client that falls behind gets `session.resync`, and a module
/// never waits for a client. Every method is prefixed with the module's namespace.
#[derive(Clone)]
pub struct ModuleNotify {
    namespace: &'static str,
    events: broadcast::Sender<Notification>,
}

impl ModuleNotify {
    pub(crate) fn new(namespace: &'static str, events: broadcast::Sender<Notification>) -> Self {
        ModuleNotify { namespace, events }
    }

    /// Sends `<namespace>.<name>` with `params`. Never blocks; no listener is fine.
    pub fn send(&self, name: &str, params: impl serde::Serialize) {
        let method = format!("{}.{name}", self.namespace);
        let _ = self.events.send(Notification::new(&method, params));
    }
}

/// How the daemon creates one module: the factory runs once, after the socket is bound, and
/// must not block (start slow work on a task of its own).
pub struct ModuleSlot {
    pub(crate) namespace: &'static str,
    pub(crate) build: Box<dyn FnOnce(ModuleNotify) -> Arc<dyn Module> + Send>,
}

impl ModuleSlot {
    pub fn new(
        namespace: &'static str,
        build: impl FnOnce(ModuleNotify) -> Arc<dyn Module> + Send + 'static,
    ) -> Self {
        ModuleSlot {
            namespace,
            build: Box::new(build),
        }
    }
}
