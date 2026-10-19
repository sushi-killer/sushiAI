//! The one table of agent CLIs: what each one is called, where its config home
//! is, which skill folders it reads, which dialogs its screen shows and whether
//! hooks report it at all.
//!
//! Every other module in this crate asks the registry and names no agent.
//! (The app, orchd and Remote still carry their own tables; they read this one
//! in later steps.) Adding an agent is one file here plus one line in
//! [`registry`]. `shell` is the template to copy — it has no production caller
//! yet.
//!
//! This crate is core (`scripts/check-crate-deps.mjs`): no tokio, no process,
//! no clock. The registry is a static table.

use std::collections::{HashMap, HashSet};
use std::ops::Deref;
use std::path::{Path, PathBuf};

use serde::{Serialize, Serializer};

pub mod screen;

mod claude;
mod codex;
mod cursor;
mod gemini;
mod shell;

#[cfg(test)]
mod tests;

/// An agent whose session the daemon launches with hooks. The two shapes argv
/// has; `launch.rs` matches on it. Its string id comes from [`id_of`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
    Codex,
}

/// The single table mapping the launch id to the agent's name. An adapter for a
/// launchable agent takes its [`Harness::id`] from here, so the two cannot drift.
pub fn id_of(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    }
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        id_of(self)
    }
}

/// The folders the skill folders hang off. The caller resolves the tool's own
/// settings (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`) into this; the crate reads no
/// environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Homes {
    /// The user's home, for `~/.agents/skills`.
    pub home: PathBuf,
    /// Claude Code's folder.
    pub claude: PathBuf,
    /// Codex's home.
    pub codex: PathBuf,
}

impl Homes {
    /// The default folders under `home`, each replaced by the tool's own setting when given.
    pub fn new(home: &Path, claude: Option<PathBuf>, codex: Option<PathBuf>) -> Homes {
        Homes {
            home: home.to_path_buf(),
            claude: claude.unwrap_or_else(|| home.join(".claude")),
            codex: codex.unwrap_or_else(|| home.join(".codex")),
        }
    }
}

/// Whether a skill folder belongs to the user or to the checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    User,
    Project,
}

/// Where a skill came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Source {
    Local,
    Plugin,
}

/// What switched a skill off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisabledBy {
    /// Its plugin is off.
    Provider,
    /// A Codex `[[skills.config]]` entry with `enabled = false`.
    SkillConfig,
    /// A Claude `skillOverrides` entry `"off"`.
    SkillOverride,
}

/// One skill folder of one agent, as the agent declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    pub scope: Scope,
    pub dir: PathBuf,
    /// Keep a hidden top-level folder inside it (Codex's `.system`).
    pub hidden: bool,
}

/// What the agents' own config files say about skills, read once by the caller
/// and handed to [`Harness::disabled_by`], so no adapter reads a file.
#[derive(Debug, Default)]
pub struct SkillConfigs {
    /// Codex `[[skills.config]]`: `SKILL.md` path (as written and real) to `enabled`.
    pub codex: HashMap<PathBuf, bool>,
    /// Claude `skillOverrides` names that are off, normalised.
    pub claude: HashSet<String>,
}

/// The facts of one skill row, for [`Harness::disabled_by`].
#[derive(Debug, Clone, Copy)]
pub struct SkillFacts<'a> {
    /// The skill's name, normalised.
    pub name: &'a str,
    /// Its `SKILL.md` path, as found.
    pub file: &'a str,
    /// Its real path.
    pub real: &'a Path,
    pub source: Source,
}

/// One agent CLI. A variant axis has one interface: everything sushiAI knows
/// about an agent is a method here, and a new agent is a new implementation.
pub trait Harness: Send + Sync {
    /// The name on the wire and in settings: `claude`, `codex`, `gemini`,
    /// `cursor-agent`, `shell`.
    fn id(&self) -> &'static str;

    /// The name a person reads: `Claude Code`.
    fn label(&self) -> &'static str;

    /// The skills provider name in `sushiai skills --json`. `None` for an agent
    /// with no skills of its own.
    fn provider(&self) -> Option<&'static str>;

    /// The launch id, for an agent whose sessions the daemon starts with hooks.
    fn agent(&self) -> Option<Agent>;

    /// The process name `foreground` matches a hand-started agent by.
    fn process_name(&self) -> Option<&'static str>;

    /// The agent's config folder, when it has one. Its presence is what makes
    /// `sushiai skills install` touch this agent at all.
    fn home<'a>(&self, homes: &'a Homes) -> Option<&'a Path>;

    /// The folders this agent reads skills from, in scan order.
    fn skills_roots(&self, homes: &Homes, project: Option<&Path>) -> Vec<SkillRoot>;

    /// Why this agent has a skill switched off, beyond the skill being disabled
    /// already (the caller checks that first).
    fn disabled_by(&self, skill: &SkillFacts, configs: &SkillConfigs) -> Option<DisabledBy>;

    /// The dialogs this agent shows that no hook reports. `None` when hooks
    /// cover it and the screen is never read.
    fn screen(&self) -> Option<screen::Screen>;
}

/// A registered agent. `Copy` and comparable by id, so it can be a map key and
/// a field; method calls go through to the adapter.
#[derive(Clone, Copy)]
pub struct HarnessRef(&'static dyn Harness);

impl std::fmt::Debug for HarnessRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("HarnessRef").field(&self.0.id()).finish()
    }
}

impl HarnessRef {
    const fn of(harness: &'static dyn Harness) -> HarnessRef {
        HarnessRef(harness)
    }
}

impl Deref for HarnessRef {
    type Target = dyn Harness;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl PartialEq for HarnessRef {
    fn eq(&self, other: &Self) -> bool {
        self.0.id() == other.0.id()
    }
}

impl Eq for HarnessRef {}

impl std::hash::Hash for HarnessRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.id().hash(state);
    }
}

/// A skill row names its agent by the label a person reads (`"Claude Code"`).
impl Serialize for HarnessRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.label())
    }
}

/// Every agent sushiAI knows, in the order they are listed anywhere.
pub fn registry() -> &'static [HarnessRef] {
    static ALL: &[HarnessRef] = &[
        HarnessRef::of(&claude::Claude),
        HarnessRef::of(&codex::Codex),
        HarnessRef::of(&gemini::Gemini),
        HarnessRef::of(&cursor::Cursor),
        HarnessRef::of(&shell::Shell),
    ];
    ALL
}

/// The agent with this name, or `None` for a plain command.
pub fn find(id: &str) -> Option<HarnessRef> {
    registry().iter().copied().find(|h| h.id() == id)
}

/// The names of the agents `foreground` matches a hand-started process by. An
/// agent without a process name (`shell`) is not one of them.
pub fn process_names() -> impl Iterator<Item = &'static str> {
    registry().iter().filter_map(|h| h.process_name())
}

/// The agent of a launch id, for a caller that holds an [`Agent`] rather than a
/// name.
pub fn of_agent(agent: Agent) -> Option<HarnessRef> {
    registry()
        .iter()
        .copied()
        .find(|h| h.agent() == Some(agent))
}

/// The screen rules of a named agent: `None` when hooks cover it, when the name
/// is unknown, and for a plain command.
pub fn screen_of(name: Option<&str>) -> Option<screen::Screen> {
    find(name?).and_then(|h| h.screen())
}
