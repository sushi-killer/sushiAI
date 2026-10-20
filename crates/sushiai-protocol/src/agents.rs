//! The agent catalog on the wire. `sushiai-agents` builds it.

use serde::{Deserialize, Serialize};

/// One model of a CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub id: String,
    pub label: String,
    pub description: String,
    /// The reasoning levels this model accepts, in the CLI's own order.
    pub efforts: Vec<String>,
    /// The level the CLI uses when the caller names none; empty when it has none.
    pub default_effort: String,
    /// The context window in tokens; 0 when the CLI does not say.
    pub context: u64,
}

/// Everything one agent CLI offers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCatalog {
    pub id: String,
    pub label: String,
    /// The models in offer order. Empty when the CLI lists none.
    pub models: Vec<CatalogModel>,
    /// The levels the chat composer offers.
    pub efforts: Vec<String>,
    /// The smaller set the routine builder offers.
    pub builder_efforts: Vec<String>,
    /// The permission modes. Empty for a CLI without interactive approvals.
    pub permissions: Vec<String>,
    /// The model the CLI uses when the caller names none: the id to pass back to
    /// it. Empty when the CLI has no default of its own.
    pub default_model: String,
    /// How that model is written for a person; the id again when the CLI lists
    /// no label for it.
    pub default_model_label: String,
    /// The level the CLI uses when the caller names none.
    pub default_effort: String,
    /// The window of the default model; 0 when it is unknown.
    pub default_context: u64,
}
