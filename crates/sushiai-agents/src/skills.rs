//! Skills for the agents in [`crate::harness`]: the scan of their skill folders and the install
//! and removal of a skill sushiAI ships. Each agent declares its own folders, so this module
//! names no agent; the bin wires it to `sushiai skills` and `sushiai orch register`, and the app
//! runs that binary.
//!
//! Reads no environment: the caller resolves `CLAUDE_CONFIG_DIR` and `CODEX_HOME` into
//! [`Homes`].
//!
//! The folders, in scan order (the first copy of a file an agent sees is its row): every agent's
//! user folders in registry order, then the project's. Claude Code adds its plugins
//! (`<claude>/plugins/installed_plugins.json`, switched on in `<claude>/settings.json`), Codex
//! its plugins (`<codex>/config.toml`).
//!
//! Codex reads both of its user folders, so a skill sushiAI installs goes into `<codex>/skills`
//! only, and a copy of ours left in `<home>/.agents/skills` is removed. Checked against
//! codex-cli 0.149.0: its bundled skill installer writes `$CODEX_HOME/skills` (default
//! `~/.codex/skills`), and the binary reads `.agents/skills` too.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::harness::{
    self, registry, DisabledBy, HarnessRef, Homes, Scope, SkillConfigs, SkillFacts, Source,
};

const SKILL_FILE: &str = "SKILL.md";
/// How deep a plugin's `skills` folder is searched.
const MAX_PLUGIN_DEPTH: usize = 8;
const MAX_SKILLS: usize = 10_000;
/// Scripts listed per skill.
const MAX_SCRIPTS: usize = 20;
/// Folders a plugin search never enters.
const SCAN_SKIP: [&str; 10] = [
    ".git",
    ".cache",
    "node_modules",
    "dist",
    "build",
    "file-history",
    "paste-cache",
    "projects",
    "logs",
    "debug",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Availability {
    Active,
    Disabled,
}

/// One skill folder of one agent, with the agent that reads it.
struct Root {
    harness: HarnessRef,
    scope: Scope,
    dir: PathBuf,
    hidden: bool,
}

/// Every skill folder to scan: the user folders of every agent in registry order first (the
/// first is where [`install`] writes), then the project's. The order is load-bearing — the
/// first copy of a file wins the row.
fn skill_folders(homes: &Homes, project: Option<&Path>) -> Vec<Root> {
    let mut all: Vec<Root> = Vec::new();
    for harness in registry() {
        for root in harness.skills_roots(homes, project) {
            all.push(Root {
                harness: *harness,
                scope: root.scope,
                dir: root.dir,
                hidden: root.hidden,
            });
        }
    }
    let (user, project): (Vec<Root>, Vec<Root>) =
        all.into_iter().partition(|r| r.scope == Scope::User);
    user.into_iter().chain(project).collect()
}

/// One skill as a harness sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub harness: HarnessRef,
    pub provider: &'static str,
    pub source: Source,
    pub scope: Scope,
    pub availability: Availability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_by: Option<DisabledBy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_scope: Option<String>,
    /// The `SKILL.md`, as found under its folder.
    pub file: String,
    /// The skill's folder.
    pub dir: String,
    /// The skill folder (or plugin folder) it was found under.
    pub root: String,
    /// Files under its `scripts` folder, absolute.
    pub scripts: Vec<String>,
    pub size: u64,
    /// Milliseconds since the epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
    pub content_hash: String,
    /// The harness sees another active skill of this name.
    pub is_duplicate: bool,
    /// `exact` when a copy has the same content, else `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicate_kind: Option<&'static str>,
    pub duplicate_count: usize,
    /// The other copies' files.
    pub duplicate_with: Vec<String>,
    #[serde(skip)]
    real: PathBuf,
}

impl Skill {
    /// The first line of the description, at most 160 characters.
    pub fn summary(&self) -> String {
        self.description
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(160)
            .collect()
    }

    /// The name the harness calls it by: a plugin's skills are `plugin:name`.
    pub(crate) fn call_name(&self) -> String {
        let name = normalize(&self.name);
        match &self.plugin {
            Some(key) => format!("{}:{name}", key.split('@').next().unwrap_or(key)),
            None => name,
        }
    }
}

/// Where a skill was found and what switches it on.
struct Origin<'a> {
    harness: HarnessRef,
    scope: Scope,
    source: Source,
    root: &'a Path,
    plugin: Option<&'a Plugin>,
    /// Keep a hidden top-level folder inside this one (Codex's `.system`).
    hidden: bool,
}

/// An installed plugin folder.
struct Plugin {
    harness: HarnessRef,
    key: String,
    root: PathBuf,
    enabled: bool,
    scope: Option<String>,
}

/// Every skill the agents in the registry see here: their skill folders (and the project's,
/// when given) and their plugins. A file linked into several folders of one agent is one row;
/// a row is a duplicate only when its agent sees another active skill of the same name.
pub fn scan(homes: &Homes, project: Option<&Path>) -> Vec<Skill> {
    let mut found = Vec::new();
    for root in skill_folders(homes, project) {
        let origin = Origin {
            harness: root.harness,
            scope: root.scope,
            source: Source::Local,
            root: &root.dir,
            plugin: None,
            hidden: root.hidden,
        };
        collect_folder(&root.dir, &origin, &mut found, true);
    }
    let claude_settings = read_json(&homes.claude.join("settings.json"));
    let codex_config = read_toml(&homes.codex.join("config.toml"));
    let mut plugins = claude_plugins(homes, &claude_settings);
    plugins.extend(codex_plugins(homes, &codex_config));
    for plugin in &plugins {
        let origin = Origin {
            harness: plugin.harness,
            scope: Scope::User,
            source: Source::Plugin,
            root: &plugin.root,
            plugin: Some(plugin),
            hidden: false,
        };
        collect_file(&plugin.root.join(SKILL_FILE), &origin, &mut found);
        collect_tree(&plugin.root.join("skills"), &origin, &mut found);
    }

    let mut seen = HashSet::new();
    found.retain(|s| seen.insert((s.harness, s.real.clone())));

    let configs = SkillConfigs {
        codex: codex_skill_configs(&codex_config, homes),
        claude: claude_overrides(&claude_settings),
    };
    for s in &mut found {
        if s.availability == Availability::Disabled {
            continue;
        }
        let name = normalize(&s.name);
        let off = s.harness.disabled_by(
            &SkillFacts {
                name: &name,
                file: &s.file,
                real: &s.real,
                source: s.source,
            },
            &configs,
        );
        if let Some(by) = off {
            s.availability = Availability::Disabled;
            s.disabled_by = Some(by);
        }
    }

    mark_duplicates(&mut found);
    found.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.file.cmp(&b.file))
    });
    found
}

fn mark_duplicates(found: &mut [Skill]) {
    let mut groups: HashMap<(HarnessRef, String), Vec<usize>> = HashMap::new();
    for (i, s) in found.iter().enumerate() {
        if s.availability == Availability::Active {
            groups
                .entry((s.harness, s.call_name()))
                .or_default()
                .push(i);
        }
    }
    for members in groups.values().filter(|m| m.len() > 1) {
        for &i in members {
            let peers: Vec<usize> = members.iter().copied().filter(|&j| j != i).collect();
            let exact = peers
                .iter()
                .any(|&j| found[j].content_hash == found[i].content_hash);
            let with = peers.iter().map(|&j| found[j].file.clone()).collect();
            let s = &mut found[i];
            s.is_duplicate = true;
            s.duplicate_kind = Some(if exact { "exact" } else { "name" });
            s.duplicate_count = members.len();
            s.duplicate_with = with;
        }
    }
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    paths.sort();
    paths
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// `<dir>/<name>/SKILL.md` for each folder in a skill folder. Hidden folders are skipped, except
/// a hidden top-level folder of a root that declares one (Codex's `.system`, which holds skills
/// of its own).
fn collect_folder(dir: &Path, origin: &Origin, found: &mut Vec<Skill>, top: bool) {
    for entry in sorted_entries(dir) {
        let name = file_name(&entry);
        if !entry.is_dir() {
            continue;
        }
        if name.starts_with('.') {
            if top && origin.hidden && name == ".system" {
                collect_folder(&entry, origin, found, false);
            }
            continue;
        }
        collect_file(&entry.join(SKILL_FILE), origin, found);
    }
}

/// Every `SKILL.md` under a plugin's `skills` folder, each real folder once.
fn collect_tree(dir: &Path, origin: &Origin, found: &mut Vec<Skill>) {
    fn walk(
        dir: &Path,
        depth: usize,
        origin: &Origin,
        found: &mut Vec<Skill>,
        seen: &mut HashSet<PathBuf>,
    ) {
        if depth > MAX_PLUGIN_DEPTH || found.len() >= MAX_SKILLS {
            return;
        }
        if !fs::canonicalize(dir).is_ok_and(|real| seen.insert(real)) {
            return;
        }
        for entry in sorted_entries(dir) {
            let name = file_name(&entry);
            if SCAN_SKIP.contains(&name.as_str()) {
                continue;
            }
            if name.eq_ignore_ascii_case(SKILL_FILE) {
                collect_file(&entry, origin, found);
            } else if entry.is_dir() {
                walk(&entry, depth + 1, origin, found, seen);
            }
        }
    }
    walk(dir, 0, origin, found, &mut HashSet::new());
}

fn millis(time: std::io::Result<SystemTime>) -> Option<u64> {
    let ms = time.ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis();
    u64::try_from(ms).ok().filter(|ms| *ms > 0)
}

fn collect_file(file: &Path, origin: &Origin, found: &mut Vec<Skill>) {
    if found.len() >= MAX_SKILLS {
        return;
    }
    let Ok(meta) = fs::metadata(file) else {
        return;
    };
    if !meta.is_file() {
        return;
    }
    let Ok(text) = fs::read_to_string(file) else {
        return;
    };
    let Ok(real) = fs::canonicalize(file) else {
        return;
    };
    let dir = file.parent().unwrap_or(file);
    let fields = front_matter(&text);
    let name = fields
        .get("name")
        .filter(|n| !n.is_empty())
        .cloned()
        .unwrap_or_else(|| file_name(dir));
    let mut scripts = Vec::new();
    script_files(&dir.join("scripts"), 1, &mut scripts);
    let plugin_off = origin.plugin.is_some_and(|p| !p.enabled);
    found.push(Skill {
        name,
        description: fields.get("description").cloned().unwrap_or_default(),
        harness: origin.harness,
        // Only an agent with skill folders has rows, and each of those names a provider.
        provider: origin.harness.provider().unwrap_or_default(),
        source: origin.source,
        scope: origin.scope,
        availability: if plugin_off {
            Availability::Disabled
        } else {
            Availability::Active
        },
        disabled_by: plugin_off.then_some(DisabledBy::Provider),
        plugin: origin.plugin.map(|p| p.key.clone()),
        plugin_scope: origin.plugin.and_then(|p| p.scope.clone()),
        file: file.to_string_lossy().into_owned(),
        dir: dir.to_string_lossy().into_owned(),
        root: origin.root.to_string_lossy().into_owned(),
        scripts,
        size: meta.len(),
        updated_at: millis(meta.modified()),
        content_hash: format!("{:x}", Sha256::digest(text.as_bytes())),
        is_duplicate: false,
        duplicate_kind: None,
        duplicate_count: 1,
        duplicate_with: Vec::new(),
        real,
    });
}

/// Files under `dir` (at most three levels), absolute, sorted, at most [`MAX_SCRIPTS`].
fn script_files(dir: &Path, depth: u32, out: &mut Vec<String>) {
    for p in sorted_entries(dir) {
        if out.len() >= MAX_SCRIPTS {
            return;
        }
        if p.is_dir() {
            if depth < 3 {
                script_files(&p, depth + 1, out);
            }
        } else if p.is_file() {
            out.push(p.to_string_lossy().into_owned());
        }
    }
}

fn read_json(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

fn read_toml(path: &Path) -> toml_edit::DocumentMut {
    fs::read_to_string(path)
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or_default()
}

/// The names Claude's `skillOverrides` switches off.
fn claude_overrides(settings: &Value) -> HashSet<String> {
    settings["skillOverrides"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, v)| v.as_str() == Some("off"))
        .map(|(k, _)| normalize(k))
        .collect()
}

/// Claude's installed plugins: the newest installation of each, on when `enabledPlugins` says so.
fn claude_plugins(homes: &Homes, settings: &Value) -> Vec<Plugin> {
    let claude = harness::CLAUDE;
    let installed = read_json(&homes.claude.join("plugins").join("installed_plugins.json"));
    let mut out = Vec::new();
    for (key, installs) in installed["plugins"].as_object().into_iter().flatten() {
        let stamp = |i: &Value| {
            i["lastUpdated"]
                .as_str()
                .or_else(|| i["installedAt"].as_str())
                .unwrap_or_default()
                .to_string()
        };
        let newest = installs
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["installPath"].is_string())
            .max_by_key(|i| stamp(i));
        let Some(install) = newest else { continue };
        let root = PathBuf::from(install["installPath"].as_str().unwrap_or_default());
        if !root.is_dir() {
            continue;
        }
        out.push(Plugin {
            harness: claude,
            key: key.clone(),
            root,
            enabled: settings["enabledPlugins"][key] == true,
            scope: Some(install["scope"].as_str().unwrap_or("user").to_string()),
        });
    }
    out
}

/// The newest folder in `dir`.
fn newest_dir(dir: &Path) -> Option<PathBuf> {
    sorted_entries(dir)
        .into_iter()
        .filter(|p| !SCAN_SKIP.contains(&file_name(p).as_str()))
        .filter_map(|p| {
            let meta = fs::metadata(&p).ok().filter(|m| m.is_dir())?;
            Some((millis(meta.modified()).unwrap_or(0), p))
        })
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)))
        .map(|(_, p)| p)
}

/// Codex's plugins in `config.toml`: its cached copy, else its marketplace folder.
fn codex_plugins(homes: &Homes, config: &toml_edit::DocumentMut) -> Vec<Plugin> {
    let codex = harness::CODEX;
    let marketplaces = config.get("marketplaces");
    let mut out = Vec::new();
    let Some(plugins) = config.get("plugins").and_then(|p| p.as_table_like()) else {
        return out;
    };
    for (key, table) in plugins.iter() {
        let Some(enabled) = table.get("enabled").and_then(|e| e.as_bool()) else {
            continue;
        };
        let Some((plugin, market)) = key.rsplit_once('@').filter(|(p, _)| !p.is_empty()) else {
            continue;
        };
        let mut candidates: Vec<PathBuf> = newest_dir(
            &homes
                .codex
                .join("plugins")
                .join("cache")
                .join(market)
                .join(plugin),
        )
        .into_iter()
        .collect();
        let source = marketplaces
            .and_then(|m| m.get(market))
            .and_then(|m| m.get("source"))
            .and_then(|s| s.as_str())
            .map(PathBuf::from)
            .filter(|s| s.is_absolute());
        if let Some(source) = source {
            candidates.push(source.join("plugins").join(plugin));
            candidates.push(source.join(plugin));
        }
        let Some(root) = candidates.into_iter().find(|c| c.is_dir()) else {
            continue;
        };
        out.push(Plugin {
            harness: codex,
            key: key.to_string(),
            root,
            enabled,
            scope: None,
        });
    }
    out
}

/// Codex's `[[skills.config]]` entries: `SKILL.md` path (as written and real) to `enabled`.
fn codex_skill_configs(config: &toml_edit::DocumentMut, homes: &Homes) -> HashMap<PathBuf, bool> {
    let mut out = HashMap::new();
    let entries = config
        .get("skills")
        .and_then(|s| s.get("config"))
        .and_then(|c| c.as_array_of_tables());
    for entry in entries.into_iter().flat_map(|a| a.iter()) {
        let (Some(path), Some(enabled)) = (
            entry.get("path").and_then(|p| p.as_str()),
            entry.get("enabled").and_then(|e| e.as_bool()),
        ) else {
            continue;
        };
        let path = match path.strip_prefix("~/") {
            Some(rest) => homes.home.join(rest),
            None => homes.home.join(path),
        };
        if let Ok(real) = fs::canonicalize(&path) {
            out.insert(real, enabled);
        }
        out.insert(path, enabled);
    }
    out
}

/// Lower case, every run of other characters one `-`.
pub(crate) fn normalize(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return v[1..v.len() - 1].to_string();
        }
    }
    v.to_string()
}

/// The `key: value` fields of a SKILL.md frontmatter. A `|` block keeps its lines, a `>`
/// block joins them with spaces.
pub fn front_matter(text: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    let mut lines = text.trim_start_matches('\u{feff}').lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return fields;
    }
    let body: Vec<&str> = lines.take_while(|l| l.trim_end() != "---").collect();
    fn key_of(line: &str) -> Option<(String, &str)> {
        let (key, value) = line.split_once(':')?;
        let ok = key.starts_with(|c: char| c.is_ascii_alphabetic())
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        ok.then(|| (key.to_string(), value.trim()))
    }
    let mut i = 0;
    while i < body.len() {
        let Some((key, value)) = key_of(body[i]) else {
            i += 1;
            continue;
        };
        i += 1;
        let block = value.chars().next().filter(|c| *c == '|' || *c == '>');
        match block {
            Some(style) if value.chars().skip(1).all(|c| "-+".contains(c)) => {
                let mut parts = Vec::new();
                while i < body.len() && key_of(body[i]).is_none() {
                    let line = body[i].trim();
                    if !line.is_empty() {
                        parts.push(line);
                    }
                    i += 1;
                }
                let joiner = if style == '>' { " " } else { "\n" };
                fields.insert(key, parts.join(joiner));
            }
            _ => {
                fields.insert(key, unquote(value));
            }
        }
    }
    fields
}

/// True when `text` is a SKILL.md whose frontmatter names it `name`.
pub fn declares(name: &str, text: &str) -> bool {
    front_matter(text).get("name").map(String::as_str) == Some(name)
}

/// A skill name sushiAI installs: lower-case letters, digits and `-`, at most 64.
pub fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("{0:?} is not a skill name (lower-case letters, digits and -)")]
    Name(String),
    #[error("the skill text does not declare `name: {0}` in its frontmatter")]
    Text(String),
}

/// What [`install`] or [`remove`] did to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Written(PathBuf),
    Removed(PathBuf),
    Failed(PathBuf, String),
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Change::Written(p) => write!(f, "written {}", p.display()),
            Change::Removed(p) => write!(f, "removed {}", p.display()),
            Change::Failed(p, e) => write!(f, "failed {}: {e}", p.display()),
        }
    }
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Removes `<root>/<name>/SKILL.md` when it declares `name` and is not the file `keep`, then
/// its folder when that is empty. A linked skill folder is someone else's and stays.
fn remove_ours(root: &Path, name: &str, keep: Option<&Path>, changes: &mut Vec<Change>) {
    let dir = root.join(name);
    let file = dir.join(SKILL_FILE);
    if is_link(&dir) {
        return;
    }
    let Ok(text) = fs::read_to_string(&file) else {
        return;
    };
    if !declares(name, &text) {
        return;
    }
    if keep.is_some() && fs::canonicalize(&file).ok().as_deref() == keep {
        return;
    }
    match fs::remove_file(&file) {
        Ok(()) => {
            // A folder with other files in it stays.
            let _ = fs::remove_dir(&dir);
            changes.push(Change::Removed(file));
        }
        Err(e) => changes.push(Change::Failed(file, e.to_string())),
    }
}

/// Installs the skill `name` (its SKILL.md `text`) for each harness whose folder exists: into
/// that harness's install folder, then removes a copy of ours from its other user folders, so
/// the harness loads it once. A SKILL.md there that declares another name, or a linked skill
/// folder, is left alone. Writes only when the content changed.
pub fn install(homes: &Homes, name: &str, text: &str) -> Result<Vec<Change>, Error> {
    if !valid_name(name) {
        return Err(Error::Name(name.into()));
    }
    if !declares(name, text) {
        return Err(Error::Text(name.into()));
    }
    let mut changes = Vec::new();
    for harness in registry() {
        if !harness.home(homes).is_some_and(Path::is_dir) {
            continue;
        }
        let user: Vec<PathBuf> = harness
            .skills_roots(homes, None)
            .into_iter()
            .map(|root| root.dir)
            .collect();
        let Some((target, others)) = user.split_first() else {
            continue;
        };
        let dir = target.join(name);
        let file = dir.join(SKILL_FILE);
        if is_link(&dir) {
            continue;
        }
        match fs::read_to_string(&file) {
            Ok(current) if current == text => {}
            Ok(current) if !declares(name, &current) => continue,
            _ => match crate::file::write_atomic(&file, text.as_bytes(), true) {
                Ok(()) => changes.push(Change::Written(file.clone())),
                Err(e) => {
                    changes.push(Change::Failed(file, e.to_string()));
                    continue;
                }
            },
        }
        let keep = fs::canonicalize(&file).ok();
        for other in others {
            remove_ours(other, name, keep.as_deref(), &mut changes);
        }
    }
    Ok(changes)
}

/// Removes the skill `name` from every user folder of every agent, wherever its SKILL.md
/// declares that name.
pub fn remove(homes: &Homes, name: &str) -> Result<Vec<Change>, Error> {
    if !valid_name(name) {
        return Err(Error::Name(name.into()));
    }
    let mut changes = Vec::new();
    for root in skill_folders(homes, None) {
        remove_ours(&root.dir, name, None, &mut changes);
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn skill(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\nbody\n")
    }

    fn homes(dir: &Path) -> Homes {
        Homes::new(dir, None, None)
    }

    #[test]
    fn front_matter_reads_plain_quoted_and_block_values() {
        let f = front_matter(
            "---\nname: \"x\"\ndescription: >-\n  One line\n  and more.\nother: |\n  a\n  b\n---\nbody",
        );
        assert_eq!(f["name"], "x");
        assert_eq!(f["description"], "One line and more.");
        assert_eq!(f["other"], "a\nb");
        assert!(front_matter("no frontmatter").is_empty());
        assert!(declares("x", "---\nname: x\n---\n"));
        assert!(!declares("x", "---\nname: y\n---\n"));
        assert!(!declares("x", "name: x\n"));
    }

    #[test]
    fn names_are_plain_tokens() {
        assert!(valid_name("sushiai-artifacts"));
        for bad in ["", "-x", "a/b", "..", "A", "a b"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn custom_tool_folders_replace_the_defaults() {
        let h = Homes::new(
            Path::new("/u"),
            Some(PathBuf::from("/c")),
            Some(PathBuf::from("/x")),
        );
        let dirs: Vec<PathBuf> = skill_folders(&h, None).into_iter().map(|r| r.dir).collect();
        assert_eq!(
            dirs,
            [
                PathBuf::from("/c/skills"),
                PathBuf::from("/x/skills"),
                PathBuf::from("/u/.agents/skills"),
            ]
        );
    }

    /// The order is load-bearing: every user folder in registry order (the first of each agent
    /// is where `install` writes), then the project's.
    #[test]
    fn every_user_folder_comes_before_the_project() {
        let h = homes(Path::new("/u"));
        let folders: Vec<(Scope, PathBuf)> = skill_folders(&h, Some(Path::new("/p")))
            .into_iter()
            .map(|r| (r.scope, r.dir))
            .collect();
        assert_eq!(
            folders,
            [
                (Scope::User, PathBuf::from("/u/.claude/skills")),
                (Scope::User, PathBuf::from("/u/.codex/skills")),
                (Scope::User, PathBuf::from("/u/.agents/skills")),
                (Scope::Project, PathBuf::from("/p/.claude/skills")),
                (Scope::Project, PathBuf::from("/p/.agents/skills")),
            ]
        );
    }

    #[test]
    fn codex_system_skills_and_project_folders_are_scanned() {
        let home = tempfile::tempdir().unwrap();
        let h = homes(home.path());
        write(
            &h.codex.join("skills/.system/imagegen/SKILL.md"),
            &skill("imagegen", "Images"),
        );
        write(
            &h.claude.join("skills/.hidden/SKILL.md"),
            &skill("hidden", "x"),
        );
        // Codex's `.system` counts at the top of every root it reads, not only its own home.
        write(
            &h.home.join(".agents/skills/.system/agentsys/SKILL.md"),
            &skill("agentsys", "Agent skills"),
        );
        let project = home.path().join("app");
        write(
            &project.join(".claude/skills/local/SKILL.md"),
            &skill("local", "Project"),
        );
        write(
            &project.join(".agents/skills/.system/projsys/SKILL.md"),
            &skill("projsys", "Project system"),
        );
        let rows = scan(&h, Some(&project));
        let names: Vec<(&str, &str, Scope)> = rows
            .iter()
            .map(|r| (r.name.as_str(), r.harness.id(), r.scope))
            .collect();
        assert_eq!(
            names,
            [
                ("agentsys", "codex", Scope::User),
                ("imagegen", "codex", Scope::User),
                ("local", "claude", Scope::Project),
                ("projsys", "codex", Scope::Project),
            ]
        );
    }

    #[test]
    fn a_codex_skill_config_and_a_claude_override_switch_a_skill_off() {
        let home = tempfile::tempdir().unwrap();
        let h = homes(home.path());
        let file = h.codex.join("skills/off/SKILL.md");
        write(&file, &skill("off", "x"));
        write(
            &h.claude.join("skills/quiet/SKILL.md"),
            &skill("quiet", "x"),
        );
        write(
            &h.codex.join("config.toml"),
            &format!(
                "[[skills.config]]\npath = {:?}\nenabled = false\n",
                file.display().to_string()
            ),
        );
        write(
            &h.claude.join("settings.json"),
            r#"{"skillOverrides":{"Quiet":"off"}}"#,
        );
        let rows = scan(&h, None);
        let by = |n: &str| rows.iter().find(|r| r.name == n).unwrap().disabled_by;
        assert_eq!(by("off"), Some(DisabledBy::SkillConfig));
        assert_eq!(by("quiet"), Some(DisabledBy::SkillOverride));
    }

    #[test]
    fn a_codex_plugin_skill_comes_from_its_newest_cached_copy() {
        let home = tempfile::tempdir().unwrap();
        let h = homes(home.path());
        write(
            &h.codex
                .join("plugins/cache/market/tool/1.0/skills/deep/one/SKILL.md"),
            &skill("one", "Plugin skill"),
        );
        write(
            &h.codex.join("config.toml"),
            "[marketplaces.market]\nsource = \"/missing\"\n\n[plugins.\"tool@market\"]\nenabled = true\n",
        );
        let rows = scan(&h, None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, Source::Plugin);
        assert_eq!(rows[0].plugin.as_deref(), Some("tool@market"));
        assert_eq!(rows[0].availability, Availability::Active);
    }

    #[test]
    fn remove_takes_only_ours_and_leaves_a_linked_folder() {
        let home = tempfile::tempdir().unwrap();
        let h = homes(home.path());
        let ours = h.claude.join("skills/mine/SKILL.md");
        write(&ours, &skill("mine", "x"));
        let foreign = h.codex.join("skills/mine/SKILL.md");
        write(&foreign, &skill("other", "x"));
        let elsewhere = home.path().join("dev/mine");
        write(&elsewhere.join(SKILL_FILE), &skill("mine", "x"));
        fs::create_dir_all(home.path().join(".agents/skills")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, home.path().join(".agents/skills/mine")).unwrap();

        let changes = remove(&h, "mine").unwrap();
        assert_eq!(changes, [Change::Removed(ours.clone())]);
        assert!(!ours.parent().unwrap().exists());
        assert!(foreign.exists());
        assert!(elsewhere.join(SKILL_FILE).exists());
    }

    #[test]
    fn install_keeps_the_codex_copy_when_both_folders_are_one() {
        let home = tempfile::tempdir().unwrap();
        let h = homes(home.path());
        fs::create_dir_all(h.codex.join("skills")).unwrap();
        fs::create_dir_all(home.path().join(".agents")).unwrap();
        std::os::unix::fs::symlink(h.codex.join("skills"), home.path().join(".agents/skills"))
            .unwrap();
        let text = skill("mine", "x");
        let changes = install(&h, "mine", &text).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(
            fs::read_to_string(h.codex.join("skills/mine/SKILL.md")).unwrap(),
            text
        );
        assert!(install(&h, "mine", &text).unwrap().is_empty());
        assert_eq!(
            install(&h, "mine", "no frontmatter"),
            Err(Error::Text("mine".into()))
        );
    }
}
