//! Which skills the agents really used: an incremental index of the Claude Code and Codex
//! transcripts, kept in a JSON cache, and the per-skill summary the catalog shows.
//!
//! The crate reads no environment and no clock: the folders, the cache path and `now` come in
//! as parameters. A file is read once; a grown file continues from the last complete line.
//!
//! What counts as a use. Claude: an `assistant` line with a `tool_use` named `Skill`
//! (`input.skill`), or a `user` line that is a slash command (`<command-name>/NAME`). Codex:
//! a `response_item` function or custom tool call that reads a path ending `/NAME/SKILL.md`
//! (`cat`, `sed`, `head`, an explicit read tool, ...). Listings (`ls`, `find`, `rg --files`,
//! `test -e`) and writes (`apply_patch`) do not count, nor do developer messages, environment
//! context, event mirrors and tool outputs.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::skills::{normalize, Harness, Homes, Skill};

const WINDOW_MS: i64 = 90 * 24 * 60 * 60 * 1000;
const MAX_EVENTS: usize = 200;
const VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub name: String,
    pub at: String,
    pub project: String,
    pub session: String,
    /// Codex: the skill folder named by the transcript's `SKILL.md` path.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dir: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct FileEntry {
    harness: String,
    size: u64,
    mtime_ms: u64,
    offset: u64,
    #[serde(default)]
    project: String,
    #[serde(default)]
    session: String,
    #[serde(default)]
    events: Vec<Event>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    version: u32,
    indexed_at: Option<String>,
    files: HashMap<String, FileEntry>,
}

/// What one index run did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStats {
    pub scanned_files: usize,
    pub read_bytes: u64,
    pub skipped_files: usize,
    pub events: usize,
}

fn iso(ms: i64) -> Option<String> {
    Some(DateTime::<Utc>::from_timestamp_millis(ms)?.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// The timestamp as fixed-width UTC text (so text order is time order).
fn normal_time(text: &str) -> Option<String> {
    iso(DateTime::parse_from_rfc3339(text).ok()?.timestamp_millis())
}

fn load_cache(path: &Path) -> Cache {
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Cache>(&t).ok())
        .filter(|c| c.version == VERSION)
        .unwrap_or_default()
}

fn save_cache(path: &Path, cache: &Cache) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let text = serde_json::to_vec(cache).map_err(std::io::Error::other)?;
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default()
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "jsonl")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for p in entries(dir) {
        if p.is_dir() {
            walk(&p, out);
        } else if is_jsonl(&p) {
            out.push(p);
        }
    }
}

/// Every transcript file of both harnesses.
fn transcript_files(homes: &Homes) -> Vec<(Harness, PathBuf)> {
    let mut found = Vec::new();
    for project in entries(&homes.claude.join("projects")) {
        for p in entries(&project) {
            if p.is_dir() {
                for sub in entries(&p.join("subagents")) {
                    if is_jsonl(&sub) {
                        found.push((Harness::Claude, sub));
                    }
                }
            } else if is_jsonl(&p) {
                found.push((Harness::Claude, p));
            }
        }
    }
    let mut codex = Vec::new();
    walk(&homes.codex.join("sessions"), &mut codex);
    walk(&homes.codex.join("archived_sessions"), &mut codex);
    found.extend(codex.into_iter().map(|p| (Harness::Codex, p)));
    found
}

/// `NAME` or `plugin:NAME` in the form skills are compared by.
fn event_name(raw: &str) -> String {
    let raw = raw.trim().trim_start_matches('/');
    match raw.split_once(':') {
        Some((plugin, name)) => format!("{}:{}", normalize(plugin), normalize(name)),
        None => normalize(raw),
    }
}

/// One use found in a line: the skill name and the line's own time, project and session.
struct Found {
    name: String,
    dir: String,
    at: String,
    project: Option<String>,
    session: Option<String>,
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter(|i| i["type"] == "text")
            .filter_map(|i| i["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn claude_uses(line: &str) -> Vec<Found> {
    if !(line.contains("\"name\":\"Skill\"") || line.contains("<command-name>/")) {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let Some(at) = v["timestamp"].as_str().and_then(normal_time) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    match v["type"].as_str() {
        Some("assistant") => {
            for item in v["message"]["content"].as_array().into_iter().flatten() {
                if item["type"] == "tool_use" && item["name"] == "Skill" {
                    if let Some(s) = item["input"]["skill"].as_str() {
                        names.push(event_name(s));
                    }
                }
            }
        }
        Some("user") => {
            let text = text_of(&v["message"]["content"]);
            let head = text.trim_start();
            if head.starts_with("<command-name>") || head.starts_with("<command-message>") {
                if let Some(rest) = text.split_once("<command-name>/").map(|x| x.1) {
                    if let Some((name, _)) = rest.split_once("</command-name>") {
                        names.push(event_name(name));
                    }
                }
            }
        }
        _ => {}
    }
    let field = |k: &str| v[k].as_str().map(str::to_string);
    names
        .into_iter()
        .filter(|n| !n.is_empty())
        .map(|name| Found {
            name,
            dir: String::new(),
            at: at.clone(),
            project: field("cwd"),
            session: field("sessionId"),
        })
        .collect()
}

const READ_WORDS: &[&str] = &[
    "cat",
    "sed",
    "head",
    "tail",
    "less",
    "more",
    "bat",
    "nl",
    "awk",
    "grep",
    "rg",
    "read",
    "read_file",
    "view_file",
];
const LIST_WORDS: &[&str] = &[
    "ls", "ll", "find", "test", "[", "[[", "tree", "stat", "fd", "exists", "--files",
];
const READ_TOOLS: &[&str] = &["read", "read_file", "view_file"];

fn is_token_break(c: char) -> bool {
    c.is_whitespace() || "\"'`{}()[],:=\\".contains(c)
}

/// Whether the command text before a path reads the file: the last known command word decides.
fn reads(prefix: &str) -> bool {
    let start = prefix.rfind(['\n', ';', '|', '&']).map_or(0, |i| i + 1);
    let tokens: Vec<String> = prefix[start..]
        .split(is_token_break)
        .filter(|t| !t.is_empty())
        .map(|t| t.rsplit('/').next().unwrap_or(t).to_lowercase())
        .collect();
    // `rg --files | grep NAME/SKILL.md` filters a listing.
    if prefix.contains("--files") {
        return false;
    }
    let decided = tokens
        .iter()
        .rev()
        .find(|t| READ_WORDS.contains(&t.as_str()) || LIST_WORDS.contains(&t.as_str()));
    match decided {
        Some(t) if READ_WORDS.contains(&t.as_str()) => {
            !(t == "sed"
                && tokens
                    .iter()
                    .any(|x| x.starts_with("-i") || x == "--in-place"))
        }
        _ => false,
    }
}

/// Every string in a tool call's arguments (a JSON text, or plain script text).
fn strings_of(text: &str, out: &mut Vec<String>) {
    fn leaves(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(items) if items.iter().all(Value::is_string) => out.push(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            Value::Array(items) => items.iter().for_each(|i| leaves(i, out)),
            Value::Object(map) => map.values().for_each(|i| leaves(i, out)),
            _ => {}
        }
    }
    match serde_json::from_str::<Value>(text) {
        Ok(v @ (Value::Object(_) | Value::Array(_))) => leaves(&v, out),
        _ => out.push(text.to_string()),
    }
}

/// The `(name, folder)` of each `/NAME/SKILL.md` path that `text` reads. `explicit_read` is a
/// call to a read tool, where any path in the arguments is a read.
fn skill_paths(text: &str, explicit_read: bool) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut parts = Vec::new();
    strings_of(text, &mut parts);
    for part in parts {
        if part.contains("*** Begin Patch") {
            continue;
        }
        for line in part.lines() {
            let trimmed = line.trim_start();
            if ["Add", "Update", "Delete"]
                .iter()
                .any(|k| trimmed.starts_with(&format!("*** {k} File:")))
            {
                continue;
            }
            for (i, _) in line.match_indices("/SKILL.md") {
                let head = &line[..i];
                if !explicit_read && !reads(head) {
                    continue;
                }
                let start = head.rfind(is_token_break).map_or(0, |p| p + 1);
                let dir = &head[start..];
                let name = dir.rsplit('/').next().unwrap_or_default();
                let plain = !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
                let name = event_name(name);
                let pair = (name, dir.trim_end_matches('/').to_string());
                if plain && !pair.0.is_empty() && !found.contains(&pair) {
                    found.push(pair);
                }
            }
        }
    }
    found
}

fn codex_uses(line: &str) -> Vec<Found> {
    if !(line.contains("SKILL.md") && line.contains("\"response_item\"")) {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return Vec::new();
    };
    let p = &v["payload"];
    if v["type"] != "response_item"
        || !matches!(
            p["type"].as_str(),
            Some("function_call" | "custom_tool_call")
        )
    {
        return Vec::new();
    }
    let tool = p["name"].as_str().unwrap_or_default().to_lowercase();
    if tool == "apply_patch" {
        return Vec::new();
    }
    let explicit_read = READ_TOOLS.contains(&tool.as_str());
    let Some(at) = v["timestamp"].as_str().and_then(normal_time) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for key in ["arguments", "input"] {
        match &p[key] {
            Value::Null => {}
            Value::String(s) => paths.extend(skill_paths(s, explicit_read)),
            other => paths.extend(skill_paths(&other.to_string(), explicit_read)),
        }
    }
    let mut seen = HashSet::new();
    paths.retain(|n| seen.insert(n.clone()));
    paths
        .into_iter()
        .map(|(name, dir)| Found {
            name,
            dir,
            at: at.clone(),
            project: None,
            session: None,
        })
        .collect()
}

/// The Codex `session_meta` line: the session id and its working folder.
fn codex_meta(line: &str) -> Option<(String, String)> {
    if !line.contains("\"session_meta\"") {
        return None;
    }
    let v = serde_json::from_str::<Value>(line).ok()?;
    if v["type"] != "session_meta" {
        return None;
    }
    let p = &v["payload"];
    Some((
        p["cwd"].as_str().unwrap_or_default().to_string(),
        p["id"].as_str().unwrap_or_default().to_string(),
    ))
}

/// Reads `path` from `entry.offset` up to the last complete line.
fn read_file(harness: Harness, path: &Path, entry: &mut FileEntry) -> std::io::Result<u64> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(entry.offset))?;
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut buf = Vec::new();
    let mut read = 0u64;
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf)?;
        read += n as u64;
        if n == 0 || buf.last() != Some(&b'\n') {
            break;
        }
        entry.offset += n as u64;
        let Ok(line) = std::str::from_utf8(&buf) else {
            continue;
        };
        let found = if harness == Harness::Claude {
            claude_uses(line)
        } else {
            if entry.session.is_empty() {
                if let Some((cwd, id)) = codex_meta(line) {
                    entry.project = cwd;
                    entry.session = id;
                }
            }
            codex_uses(line)
        };
        for f in found {
            entry.events.push(Event {
                name: f.name,
                dir: f.dir,
                at: f.at,
                project: f.project.unwrap_or_else(|| entry.project.clone()),
                session: f
                    .session
                    .filter(|s| !s.is_empty())
                    .or_else(|| Some(entry.session.clone()).filter(|s| !s.is_empty()))
                    .unwrap_or_else(|| stem.clone()),
            });
        }
    }
    Ok(read)
}

/// Builds or updates the usage cache at `cache_path` from the transcripts under `homes`.
/// Events older than 90 days before `now_ms` are dropped.
pub fn index(homes: &Homes, cache_path: &Path, now_ms: i64) -> std::io::Result<IndexStats> {
    let cutoff_ms = now_ms - WINDOW_MS;
    let cutoff = iso(cutoff_ms).unwrap_or_default();
    let mut old = load_cache(cache_path);
    let mut next = Cache {
        version: VERSION,
        indexed_at: iso(now_ms),
        files: HashMap::new(),
    };
    let mut stats = IndexStats::default();
    for (harness, path) in transcript_files(homes) {
        let key = path.to_string_lossy().into_owned();
        let prior = old.files.remove(&key).filter(|e| e.harness == harness.id());
        let Ok(meta) = fs::metadata(&path) else {
            stats.skipped_files += 1;
            next.files.extend(prior.map(|e| (key, e)));
            continue;
        };
        stats.scanned_files += 1;
        let size = meta.len();
        let mtime_ms = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_millis() as u64);
        let had_prior = prior.is_some();
        // Grown (or touched): continue from the offset. Shrunk, or new: from the start.
        let (mut entry, unchanged) = match prior {
            Some(e) if e.size <= size => {
                let same = e.size == size && e.mtime_ms == mtime_ms;
                (e, same)
            }
            _ => (FileEntry::default(), false),
        };
        entry.harness = harness.id().to_string();
        // Untouched for the whole window: it cannot hold a recent use.
        if !unchanged && (mtime_ms as i64) >= cutoff_ms {
            let backup = entry.clone();
            match read_file(harness, &path, &mut entry) {
                Ok(read) => stats.read_bytes += read,
                Err(_) => {
                    stats.skipped_files += 1;
                    stats.scanned_files -= 1;
                    // Keep what the last run knew about this file.
                    if had_prior {
                        stats.events += backup.events.len();
                        next.files.insert(key, backup);
                    }
                    continue;
                }
            }
        }
        entry.size = size;
        entry.mtime_ms = mtime_ms;
        entry.events.retain(|e| e.at >= cutoff);
        stats.events += entry.events.len();
        next.files.insert(key, entry);
    }
    save_cache(cache_path, &next)?;
    Ok(stats)
}

/// One use as the catalog lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageEvent {
    pub at: String,
    pub project: String,
    pub session: String,
}

/// The `usage` field of a catalog row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUsage {
    pub uses90d: usize,
    pub last_used_at: Option<String>,
    pub events: Vec<UsageEvent>,
    pub indexed_at: Option<String>,
}

/// The cache, read for the catalog. Never indexes.
#[derive(Debug, Default)]
pub struct Usage {
    indexed_at: Option<String>,
    events: Vec<Event>,
    by_name: HashMap<(Harness, String), Vec<usize>>,
    by_dir: HashMap<String, Vec<usize>>,
}

impl Usage {
    /// The cache at `path`; empty when it is missing or unreadable.
    pub fn load(path: &Path) -> Usage {
        let cache = load_cache(path);
        let mut usage = Usage {
            indexed_at: cache.indexed_at,
            ..Usage::default()
        };
        for entry in cache.files.into_values() {
            let harness = if entry.harness == Harness::Codex.id() {
                Harness::Codex
            } else {
                Harness::Claude
            };
            for e in entry.events {
                let i = usage.events.len();
                usage
                    .by_name
                    .entry((harness, e.name.clone()))
                    .or_default()
                    .push(i);
                if harness == Harness::Codex && !e.dir.is_empty() {
                    usage.by_dir.entry(e.dir.clone()).or_default().push(i);
                }
                usage.events.push(e);
            }
        }
        usage
    }

    /// The uses of `skill` within 90 days of `now_ms`, newest first, at most 200 listed.
    pub fn of(&self, skill: &Skill, now_ms: i64) -> SkillUsage {
        let mut keys = Vec::new();
        if skill.plugin.is_some() {
            keys.push(skill.call_name());
        } else {
            keys.push(normalize(&skill.name));
            if let Some(folder) = Path::new(&skill.dir).file_name() {
                keys.push(normalize(&folder.to_string_lossy()));
            }
        }
        let mut seen = HashSet::new();
        keys.retain(|k| seen.insert(k.clone()));
        let cutoff = iso(now_ms - WINDOW_MS).unwrap_or_default();
        let mut picked: HashSet<usize> = keys
            .iter()
            .filter_map(|k| self.by_name.get(&(skill.harness, k.clone())))
            .flatten()
            .copied()
            .collect();
        if skill.harness == Harness::Codex {
            // A Codex transcript names the skill by its file, not by `plugin:name`.
            if let Some(ids) = self.by_dir.get(skill.dir.trim_end_matches('/')) {
                picked.extend(ids);
            }
            if let Some(plugin) = &skill.plugin {
                let plugin = plugin.split('@').next().unwrap_or(plugin);
                let ids = self
                    .by_name
                    .get(&(Harness::Codex, normalize(&skill.name)))
                    .into_iter()
                    .flatten();
                picked.extend(
                    ids.filter(|&&i| self.events[i].dir.split('/').any(|part| part == plugin)),
                );
            }
        }
        let mut events: Vec<&Event> = picked
            .into_iter()
            .map(|i| &self.events[i])
            .filter(|e| e.at >= cutoff)
            .collect();
        events.sort_by(|a, b| b.at.cmp(&a.at));
        SkillUsage {
            uses90d: events.len(),
            last_used_at: events.first().map(|e| e.at.clone()),
            events: events
                .into_iter()
                .take(MAX_EVENTS)
                .map(|e| UsageEvent {
                    at: e.at.clone(),
                    project: e.project.clone(),
                    session: e.session.clone(),
                })
                .collect(),
            indexed_at: self.indexed_at.clone(),
        }
    }
}
