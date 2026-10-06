//! Merge our hook entries into Codex's global `hooks.json` and take them out
//! again. `merge` and `remove` are pure; `install` and `uninstall` add a
//! backup and an atomic write.
//!
//! Our handlers are recognised by their command: it runs `<...>/sushiai hook
//! <event>` (the path may be quoted). Every other entry is preserved.
//! `serde_json` runs with `preserve_order`, so user key order survives.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::launch::{hook_command, permission_timeout, require_absolute, LaunchError};
use crate::Agent;

/// (Codex event, hook argument, timeout seconds; 0 = the permission timeout). SessionEnd and Interrupt
/// have a 1-3 s budget on the Codex side, so the entry asks for little.
const ENTRIES: &[(&str, &str, u64)] = &[
    ("SessionStart", "session-start", 10),
    ("UserPromptSubmit", "prompt", 10),
    ("PreToolUse", "tool-start", 10),
    ("PostToolUse", "tool-end", 10),
    ("PermissionRequest", "permission", 0),
    ("Stop", "stop", 10),
    ("Interrupt", "interrupt", 2),
    ("SessionEnd", "session-end", 2),
];

#[derive(Debug, thiserror::Error)]
pub enum HooksFileError {
    #[error(transparent)]
    Launch(#[from] LaunchError),
    #[error("hooks.json has an unexpected shape: {0}")]
    Shape(&'static str),
    #[error("hooks.json is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Split the first words of a shell command (single quotes, double quotes and
/// backslashes; no expansion).
fn shell_words(s: &str, max: usize) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    cur.push(q);
                }
            }
            '"' => {
                in_word = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => cur.extend(chars.next()),
                        _ => cur.push(q),
                    }
                }
            }
            '\\' => {
                in_word = true;
                cur.extend(chars.next());
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                    if words.len() == max {
                        return words;
                    }
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    words
}

/// True when a hook command is one of ours: the first word is a path whose
/// basename is `sushiai`, followed by `hook <event>`.
pub fn is_ours(command: &str) -> bool {
    let w = shell_words(command, 3);
    w.len() >= 3 && w[1] == "hook" && is_sushiai_bin(&w[0])
}

fn is_sushiai_bin(path: &str) -> bool {
    Path::new(path).file_name().is_some_and(|n| n == "sushiai")
}

fn handler_is_ours(h: &Value) -> bool {
    h.get("command")
        .and_then(Value::as_str)
        .is_some_and(is_ours)
}

fn root_of(existing: &Value) -> Result<Map<String, Value>, HooksFileError> {
    match existing {
        Value::Null => Ok(Map::new()),
        Value::Object(m) => Ok(m.clone()),
        _ => Err(HooksFileError::Shape("top level is not an object")),
    }
}

fn hooks_of(
    root: &mut Map<String, Value>,
) -> Result<Option<&mut Map<String, Value>>, HooksFileError> {
    match root.get_mut("hooks") {
        None => Ok(None),
        Some(Value::Object(m)) => Ok(Some(m)),
        Some(_) => Err(HooksFileError::Shape("`hooks` is not an object")),
    }
}

/// Add our entries. Idempotent: an existing handler of ours for an event is
/// updated in place (and duplicates dropped), never added twice.
pub fn merge(
    existing: &Value,
    hook_bin: &str,
    permission_wait_secs: u64,
) -> Result<Value, HooksFileError> {
    require_absolute(hook_bin)?;
    if !is_sushiai_bin(hook_bin) {
        return Err(LaunchError::HookBinName(hook_bin.to_owned()).into());
    }
    let mut root = root_of(existing)?;
    root.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks_of(&mut root)?.ok_or(HooksFileError::Shape("no hooks object"))?;
    for (event, arg, timeout) in ENTRIES {
        let ours = json!({
            "type": "command",
            "command": hook_command(hook_bin, arg, Agent::Codex),
            "timeout": if *timeout == 0 { permission_timeout(permission_wait_secs) } else { *timeout },
        });
        let groups = hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or(HooksFileError::Shape("an event entry is not an array"))?;
        let mut placed = false;
        let mut emptied = Vec::new();
        for (gi, group) in groups.iter_mut().enumerate() {
            let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                continue;
            };
            let mut i = 0;
            let mut dropped = false;
            while i < list.len() {
                if handler_is_ours(&list[i]) {
                    if placed {
                        list.remove(i);
                        dropped = true;
                        continue;
                    }
                    list[i] = ours.clone();
                    placed = true;
                }
                i += 1;
            }
            if dropped && list.is_empty() {
                emptied.push(gi);
            }
        }
        // A group emptied by dropping our duplicates goes with them.
        let mut gi = 0;
        groups.retain(|_| {
            gi += 1;
            !emptied.contains(&(gi - 1))
        });
        if !placed {
            groups.push(json!({ "hooks": [ours] }));
        }
    }
    Ok(Value::Object(root))
}

/// Take our entries out; keep everything else.
pub fn remove(existing: &Value) -> Result<Value, HooksFileError> {
    let mut root = root_of(existing)?;
    let mut removed_any = false;
    if let Some(hooks) = hooks_of(&mut root)? {
        let mut emptied_events = Vec::new();
        for (event, groups) in hooks.iter_mut() {
            let Some(groups) = groups.as_array_mut() else {
                continue;
            };
            let mut removed_here = false;
            groups.retain_mut(|group| {
                let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                    return true;
                };
                let n = list.len();
                list.retain(|h| !handler_is_ours(h));
                let held_ours = list.len() != n;
                removed_any |= held_ours;
                removed_here |= held_ours;
                !(held_ours && list.is_empty())
            });
            if groups.is_empty() && removed_here {
                emptied_events.push(event.clone());
            }
        }
        for event in emptied_events {
            hooks.shift_remove(&event);
        }
        if removed_any && hooks.is_empty() {
            root.shift_remove("hooks");
        }
    }
    Ok(Value::Object(root))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOutcome {
    pub changed: bool,
    pub backup: Option<PathBuf>,
}

pub(crate) fn io_err(path: &Path) -> impl Fn(std::io::Error) -> HooksFileError + '_ {
    move |source| HooksFileError::Io {
        path: path.to_owned(),
        source,
    }
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || "hooks.json".to_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn read_existing(path: &Path) -> Result<Option<Value>, HooksFileError> {
    match fs::read(path) {
        Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Ok(Some(json!({}))),
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err(path)(e)),
    }
}

fn write_atomic(path: &Path, value: &Value, ts: u64) -> Result<(), HooksFileError> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    write_text_atomic(path, &text, ts)
}

/// Temp file in the same directory, fsync, mode copied from the old file,
/// rename over it.
pub(crate) fn write_text_atomic(path: &Path, text: &str, ts: u64) -> Result<(), HooksFileError> {
    let tmp = path.with_file_name(format!(".{}.tmp-{ts}", file_name(path)));
    let res = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map_err(io_err(path))
}

/// Copy `path` to `<name>.bak-<ts>` and keep the newest three.
pub(crate) fn backup_file(path: &Path, ts: u64) -> Result<PathBuf, HooksFileError> {
    let b = path.with_file_name(format!("{}.bak-{ts}", file_name(path)));
    fs::copy(path, &b).map_err(io_err(&b))?;
    prune_backups(path);
    Ok(b)
}

/// Keep the newest `KEEP_BACKUPS` files named `<name>.bak-<number>`.
const KEEP_BACKUPS: usize = 3;

pub(crate) fn prune_backups(path: &Path) {
    let prefix = format!("{}.bak-", file_name(path));
    let Some(dir) = path.parent() else { return };
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut found: Vec<(u64, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ts = name.strip_prefix(&prefix)?.parse::<u64>().ok()?;
            Some((ts, e.path()))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, old) in found.into_iter().skip(KEEP_BACKUPS) {
        let _ = fs::remove_file(old);
    }
}

fn rewrite(
    path: &Path,
    ts: u64,
    change: impl Fn(&Value) -> Result<Value, HooksFileError>,
    create: bool,
) -> Result<FileOutcome, HooksFileError> {
    let unchanged = FileOutcome {
        changed: false,
        backup: None,
    };
    // Write through a symlink: the link stays, its target changes.
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let path = resolved.as_path();
    let existing = read_existing(path)?;
    if existing.is_none() && !create {
        return Ok(unchanged);
    }
    let next = change(existing.as_ref().unwrap_or(&Value::Null))?;
    if existing.as_ref() == Some(&next) {
        return Ok(unchanged);
    }
    let backup = if existing.is_some() {
        Some(backup_file(path, ts)?)
    } else {
        None
    };
    write_atomic(path, &next, ts)?;
    Ok(FileOutcome {
        changed: true,
        backup,
    })
}

/// Install our entries into the file at `path` (created when missing).
/// `ts` names the backup; the caller supplies the clock.
pub fn install(
    path: &Path,
    hook_bin: &str,
    permission_wait_secs: u64,
    ts: u64,
) -> Result<FileOutcome, HooksFileError> {
    require_absolute(hook_bin)?;
    rewrite(path, ts, |v| merge(v, hook_bin, permission_wait_secs), true)
}

/// Remove our entries from the file at `path`. A missing file is a no-op.
pub fn uninstall(path: &Path, ts: u64) -> Result<FileOutcome, HooksFileError> {
    rewrite(path, ts, remove, false)
}
