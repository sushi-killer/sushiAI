use serde::{Deserialize, Serialize};
use sushiai_protocol::SessionInfo;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("state file schema version {0:?} is not supported (expected {SCHEMA_VERSION})")]
    UnsupportedSchema(Option<u64>),
    #[error("state file is not valid: {0}")]
    Invalid(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateFile {
    pub schema_version: u32,
    pub sessions: Vec<SessionInfo>,
}

impl StateFile {
    pub fn new(sessions: Vec<SessionInfo>) -> Self {
        StateFile {
            schema_version: SCHEMA_VERSION,
            sessions,
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, StateError> {
        Ok(serde_json::to_vec_pretty(self)?)
    }

    /// Checks the schema version before reading anything else.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, StateError> {
        let value: serde_json::Value = serde_json::from_slice(bytes)?;
        let version = value.get("schemaVersion").and_then(|v| v.as_u64());
        if version != Some(u64::from(SCHEMA_VERSION)) {
            return Err(StateError::UnsupportedSchema(version));
        }
        Ok(serde_json::from_value(value)?)
    }
}
