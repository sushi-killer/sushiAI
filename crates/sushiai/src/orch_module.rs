//! The composition point for the orchestration module: the only place that knows both the
//! daemon's `Module` trait and the `sushiai-orch` library. L1 replaces the stub with the
//! real pipeline; the daemon and the protocol never learn what `orch` is.

use std::sync::Arc;

use serde_json::{json, Value};
use sushiai_daemon::{BoxFuture, Module, ModuleNotify, ModuleSlot, Reply};
use sushiai_protocol::code;

const NAMESPACE: &str = "orch";

/// The slot `main` passes to the daemon.
pub fn slot() -> ModuleSlot {
    ModuleSlot::new(NAMESPACE, |notify| Arc::new(Stub { _notify: notify }))
}

struct Stub {
    _notify: ModuleNotify,
}

impl Module for Stub {
    fn namespace(&self) -> &'static str {
        NAMESPACE
    }

    fn capability(&self) -> &'static str {
        "orch"
    }

    fn call(&self, method: &str, params: Value) -> BoxFuture<Reply> {
        let method = method.to_string();
        Box::pin(async move {
            match method.as_str() {
                "echo" => Ok(json!({ "echo": params })),
                other => Err((
                    code::METHOD_NOT_FOUND,
                    format!("unknown method {NAMESPACE}.{other}"),
                )),
            }
        })
    }

    fn shutdown(&self) -> BoxFuture<()> {
        Box::pin(async {})
    }
}
