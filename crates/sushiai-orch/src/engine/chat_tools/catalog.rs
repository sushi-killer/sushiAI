use super::server::read_json;
use crate::clock::now_ms;
use crate::engine::state::App;
use crate::store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
}

/// What the last look at a server found. `status` is `ok`, `needs-auth`,
/// `failed` or `unavailable` (could not be resolved).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub tools: Vec<KnownTool>,
    pub checked_at: i64,
}

static CACHE_LOCK: StdMutex<()> = StdMutex::new(());

fn cache_path(app: &App) -> PathBuf {
    app.data_dir.join("chat-tools.json")
}

pub(in crate::engine) fn load_cache(app: &App) -> BTreeMap<String, Discovery> {
    read_json(&cache_path(app))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

pub(super) fn update_cache(app: &App, change: impl FnOnce(&mut BTreeMap<String, Discovery>)) {
    let _guard = CACHE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut cache = load_cache(app);
    change(&mut cache);
    let _ = store::files::write_json_atomic(&cache_path(app), &cache);
}

pub(super) fn failed(status: &str, reason: impl Into<String>) -> Discovery {
    Discovery {
        status: status.to_string(),
        reason: Some(reason.into()),
        tools: Vec::new(),
        checked_at: now_ms(),
    }
}
