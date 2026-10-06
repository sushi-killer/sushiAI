//! Codex hook trust. Codex runs a hook from a user `hooks.json` only when
//! `config.toml` holds `[hooks.state."<key>"] trusted_hash = "sha256:..."`
//! for it (see `artifacts/SPEC-codex-hook-trust.md`; verified against 12 real
//! entries). This module computes those entries for OUR handlers and writes
//! them without touching anything else.
//!
//! - key: `<abs hooks.json path>:<event_snake>:<group index>:<handler index>`
//!   (the path is used as given: no canonicalize, no `~`);
//! - hash: SHA-256 of the compact, key-sorted JSON of
//!   `{event_name, matcher?, hooks: [normalized handler]}`.

use std::fs;
use std::path::Path;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use toml_edit::{value, DocumentMut, Item, Table};

use crate::codex_hooks::{backup_file, io_err, is_ours, write_text_atomic, HooksFileError};

#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    #[error(transparent)]
    File(#[from] HooksFileError),
    #[error("hooks path must be absolute, got {0:?}")]
    NotAbsolute(String),
    #[error("config.toml is not valid TOML: {0}")]
    Toml(String),
    #[error("config.toml has an unexpected shape: {0}")]
    Shape(&'static str),
    #[error("hooks.json is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("trust entry for {0:?} is missing after the write")]
    NotWritten(String),
}

/// (hooks.json event name, snake_case label).
const EVENTS: &[(&str, &str)] = &[
    ("PreToolUse", "pre_tool_use"),
    ("PermissionRequest", "permission_request"),
    ("PostToolUse", "post_tool_use"),
    ("PreCompact", "pre_compact"),
    ("PostCompact", "post_compact"),
    ("SessionStart", "session_start"),
    ("SessionEnd", "session_end"),
    ("UserPromptSubmit", "user_prompt_submit"),
    ("SubagentStart", "subagent_start"),
    ("SubagentStop", "subagent_stop"),
    ("Stop", "stop"),
    ("Interrupt", "interrupt"),
];

/// Events whose group matcher takes part in the hash.
const WITH_MATCHER: &[&str] = &[
    "pre_tool_use",
    "permission_request",
    "post_tool_use",
    "session_start",
    "session_end",
    "subagent_start",
    "subagent_stop",
    "pre_compact",
    "post_compact",
];

const CONTEXT_LIMIT_EVENTS: &[&str] = &[
    "pre_tool_use",
    "post_tool_use",
    "session_start",
    "user_prompt_submit",
    "subagent_start",
];
const DEFAULT_CONTEXT_LIMIT: i64 = 2500;

fn normalized_timeout(event: &str, timeout: Option<i64>) -> i64 {
    match event {
        "session_end" | "interrupt" => timeout.unwrap_or(1).clamp(1, 3),
        _ => timeout.unwrap_or(600).max(1),
    }
}

/// Recursively key-sorted copy (explicit, so `preserve_order` does not matter).
fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), sorted(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// Hash of one command handler, as Codex computes it.
pub fn handler_hash(event_snake: &str, matcher: Option<&str>, handler: &Value) -> String {
    let mut hook = Map::new();
    hook.insert("type".into(), json!("command"));
    hook.insert(
        "command".into(),
        json!(handler.get("command").and_then(Value::as_str).unwrap_or("")),
    );
    hook.insert(
        "timeout".into(),
        json!(normalized_timeout(
            event_snake,
            handler.get("timeout").and_then(Value::as_i64)
        )),
    );
    hook.insert(
        "async".into(),
        json!(handler
            .get("async")
            .and_then(Value::as_bool)
            .unwrap_or(false)),
    );
    if let Some(m) = handler.get("statusMessage").and_then(Value::as_str) {
        hook.insert("statusMessage".into(), json!(m));
    }
    if let Some(limit) = handler
        .get("additionalContextLimit")
        .and_then(Value::as_i64)
    {
        if CONTEXT_LIMIT_EVENTS.contains(&event_snake) && limit != DEFAULT_CONTEXT_LIMIT {
            hook.insert("additionalContextLimit".into(), json!(limit));
        }
    }
    let mut ident = Map::new();
    ident.insert("event_name".into(), json!(event_snake));
    if let (true, Some(m)) = (WITH_MATCHER.contains(&event_snake), matcher) {
        ident.insert("matcher".into(), json!(m));
    }
    ident.insert("hooks".into(), json!([Value::Object(hook)]));
    let bytes = serde_json::to_vec(&sorted(&Value::Object(ident))).unwrap_or_default();
    let digest = Sha256::digest(&bytes);
    let mut hex = String::with_capacity(71);
    hex.push_str("sha256:");
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// `(key, hash)` for every handler of ours in `hooks_json`, with indices taken
/// from the layout as it is now. `hooks_path_abs` is the path Codex will see.
pub fn trust_entries(hooks_json: &Value, hooks_path_abs: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(hooks) = hooks_json.get("hooks").and_then(Value::as_object) else {
        return out;
    };
    for (pascal, snake) in EVENTS {
        let Some(groups) = hooks.get(*pascal).and_then(Value::as_array) else {
            continue;
        };
        for (g, group) in groups.iter().enumerate() {
            let matcher = group.get("matcher").and_then(Value::as_str);
            let Some(list) = group.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for (i, handler) in list.iter().enumerate() {
                let ours = handler
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(is_ours);
                if ours {
                    out.push((
                        format!("{hooks_path_abs}:{snake}:{g}:{i}"),
                        handler_hash(snake, matcher, handler),
                    ));
                }
            }
        }
    }
    out
}

fn parse_doc(text: &str) -> Result<DocumentMut, TrustError> {
    text.parse::<DocumentMut>()
        .map_err(|e| TrustError::Toml(e.to_string()))
}

fn implicit_table(item: &mut Item) -> Result<&mut Table, TrustError> {
    if item.is_none() {
        let mut t = Table::new();
        t.set_implicit(true);
        *item = Item::Table(t);
    }
    item.as_table_mut()
        .ok_or(TrustError::Shape("`hooks` or `hooks.state` is not a table"))
}

fn state_table(doc: &mut DocumentMut) -> Result<&mut Table, TrustError> {
    let hooks = implicit_table(doc.as_table_mut().entry("hooks").or_insert(Item::None))?;
    implicit_table(hooks.entry("state").or_insert(Item::None))
}

/// Write `trusted_hash` for each entry. Keys under `hooks_path_abs` that are
/// not in `entries` but hold the hash of one of our handlers are stale (an
/// index shift left them behind) and are removed. Nothing else is touched.
pub fn apply_trust(
    config_toml: &str,
    hooks_path_abs: &str,
    entries: &[(String, String)],
) -> Result<String, TrustError> {
    let mut doc = parse_doc(config_toml)?;
    let state = state_table(&mut doc)?;
    for (key, hash) in entries {
        let item = state.entry(key).or_insert(Item::Table(Table::new()));
        let tbl = item
            .as_table_mut()
            .ok_or(TrustError::Shape("a trust entry is not a table"))?;
        if tbl.get("trusted_hash").and_then(Item::as_str) != Some(hash) {
            tbl["trusted_hash"] = value(hash.as_str());
        }
    }
    let prefix = format!("{hooks_path_abs}:");
    let stale: Vec<String> = state
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix) && !entries.iter().any(|(e, _)| e == k))
        .filter(|(_, v)| {
            v.get("trusted_hash")
                .and_then(Item::as_str)
                .is_some_and(|h| entries.iter().any(|(_, eh)| eh == h))
        })
        .map(|(k, _)| k.to_owned())
        .collect();
    for key in stale {
        drop_trusted_hash(state, &key);
    }
    Ok(doc.to_string())
}

/// Remove `trusted_hash` of `key`; drop the table when nothing else is in it.
fn drop_trusted_hash(state: &mut Table, key: &str) {
    let only_hash = state
        .get(key)
        .and_then(Item::as_table)
        .is_some_and(|t| t.len() == 1);
    if only_hash {
        state.remove(key);
    } else if let Some(t) = state.get_mut(key).and_then(Item::as_table_mut) {
        t.remove("trusted_hash");
    }
}

/// Remove exactly these keys (used on uninstall).
pub fn remove_trust(config_toml: &str, keys: &[String]) -> Result<String, TrustError> {
    let mut doc = parse_doc(config_toml)?;
    if doc.get("hooks").and_then(|h| h.get("state")).is_none() {
        return Ok(config_toml.to_owned());
    }
    let state = state_table(&mut doc)?;
    for k in keys {
        drop_trusted_hash(state, k);
    }
    Ok(doc.to_string())
}

fn read_hooks(hooks_path: &Path) -> Result<Value, TrustError> {
    match fs::read(hooks_path) {
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(io_err(hooks_path)(e).into()),
    }
}

fn abs_str(hooks_path: &Path) -> Result<String, TrustError> {
    let s = hooks_path.to_string_lossy().into_owned();
    if hooks_path.is_absolute() {
        Ok(s)
    } else {
        Err(TrustError::NotAbsolute(s))
    }
}

/// Outcome of `trust` / `untrust`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustOutcome {
    pub changed: bool,
    pub backup: Option<std::path::PathBuf>,
}

fn rewrite_config(
    config_path: &Path,
    ts: u64,
    change: impl Fn(&str) -> Result<String, TrustError>,
    verify: impl Fn(&DocumentMut) -> Result<(), TrustError>,
) -> Result<TrustOutcome, TrustError> {
    // Write through a symlink: the link stays, its target changes.
    let resolved = fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_owned());
    let path = resolved.as_path();
    let current = match fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io_err(path)(e).into()),
    };
    let next = change(current.as_deref().unwrap_or(""))?;
    if current.as_deref() == Some(next.as_str()) || (current.is_none() && next.is_empty()) {
        return Ok(TrustOutcome {
            changed: false,
            backup: None,
        });
    }
    let backup = match current {
        Some(_) => Some(backup_file(path, ts)?),
        None => None,
    };
    write_text_atomic(path, &next, ts)?;
    let written = fs::read_to_string(path).map_err(io_err(path))?;
    verify(&parse_doc(&written)?)?;
    Ok(TrustOutcome {
        changed: true,
        backup,
    })
}

/// Trust our handlers in `hooks_path` by writing `config_path`. Call after
/// `codex_hooks::install` (indices come from the file as it is now). Backs up
/// config.toml (newest 3 kept), writes atomically and checks the keys.
pub fn trust(config_path: &Path, hooks_path: &Path, ts: u64) -> Result<TrustOutcome, TrustError> {
    let abs = abs_str(hooks_path)?;
    let entries = trust_entries(&read_hooks(hooks_path)?, &abs);
    rewrite_config(
        config_path,
        ts,
        |text| apply_trust(text, &abs, &entries),
        |doc| {
            for (key, hash) in &entries {
                let got = doc
                    .get("hooks")
                    .and_then(|h| h.get("state"))
                    .and_then(|s| s.get(key.as_str()))
                    .and_then(|t| t.get("trusted_hash"))
                    .and_then(Item::as_str);
                if got != Some(hash.as_str()) {
                    return Err(TrustError::NotWritten(key.clone()));
                }
            }
            Ok(())
        },
    )
}

/// Remove the trust entries of our handlers. Call BEFORE
/// `codex_hooks::uninstall`, while `hooks_path` still holds them.
pub fn untrust(config_path: &Path, hooks_path: &Path, ts: u64) -> Result<TrustOutcome, TrustError> {
    let abs = abs_str(hooks_path)?;
    let keys: Vec<String> = trust_entries(&read_hooks(hooks_path)?, &abs)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    if !config_path.exists() {
        return Ok(TrustOutcome {
            changed: false,
            backup: None,
        });
    }
    rewrite_config(config_path, ts, |t| remove_trust(t, &keys), |_| Ok(()))
}
