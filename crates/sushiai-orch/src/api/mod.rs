//! Public host values and protocol adapters.

use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub mod events;
pub mod mcp;

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

pub(crate) mod validation;

pub(crate) mod dispatch;
