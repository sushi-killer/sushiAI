use super::evolution::EvolutionSettings;
use super::presets::Preset;
use super::pricing::{default_prices, Price};
use super::routing::{Harness, Route, Tier};
use super::variant::Variant;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    Native,
    Host,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub routes: Vec<Route>,
    pub tiers: HashMap<Tier, String>,
    /// Route id; "" = off, "auto" = never the implementer's route nor one
    /// with its harness and model: a priced route of known strength at the
    /// floor on another harness first, else the strongest remaining route;
    /// the implementer's own route only when no other route exists.
    pub review: String,
    pub sandbox: SandboxMode,
    #[serde(default)]
    pub sandbox_repos: BTreeMap<String, SandboxMode>,
    pub allowed_domains: Vec<String>,
    /// Network switch for Codex's workspace sandbox. Task implement shells
    /// use the outer task profile with open network even when this is false.
    /// `allowedDomains` only governs Claude's allowlist.
    #[serde(default)]
    pub codex_network: bool,
    pub protected_paths: Vec<String>,
    pub max_attempts: u32,
    pub parallel: u32,
    /// None = core-based automatic limit; 0 = unlimited. Applied live.
    #[serde(default)]
    pub heavy_checks: Option<u32>,
    /// Dollars a UTC day may spend across every repo and stage before orchd
    /// asks (a `daily_budget` question) instead of starting a plan run or
    /// implement attempt; 0 = off.
    #[serde(default)]
    pub daily_budget_usd: f64,
    /// At most this many subtasks of one parent run at once (`parallel`
    /// stays the global cap).
    #[serde(default = "default_child_parallel")]
    pub child_parallel: u32,
    /// Seconds one verify command may run before it is killed. The Stop
    /// hook's budget derives from it, capped under Claude's 600 s hook limit.
    #[serde(default = "default_verify_timeout_secs")]
    pub verify_timeout_secs: u64,
    /// Seconds the planner may run before it is stopped; the one resumed
    /// retry gets twice this. `0` or a missing key means the default.
    #[serde(default = "default_plan_timeout_secs")]
    pub plan_timeout_secs: u64,
    /// Minutes an implement attempt may run before it is stopped. An eligible
    /// retry continues its session (failure kind `time_budget`); `0` = no limit.
    #[serde(default = "default_implement_budget_mins")]
    pub implement_budget_mins: f64,
    /// Route id used for the drafting/plan stage of a `{repo, request}`
    /// `task.create`; `""` turns planning off (that create form is then
    /// rejected -- there is nothing to run it with).
    #[serde(default = "default_planner")]
    pub planner: String,
    /// Maximum Unicode characters for standard-tier planning; 0 disables
    /// the entire small-request check and keeps every request on the planner.
    #[serde(default = "default_small_plan_max_chars")]
    pub small_plan_max_chars: usize,
    /// Maximum distinct repo paths in a small request.
    #[serde(default = "default_small_plan_max_paths")]
    pub small_plan_max_paths: usize,
    /// Route id of the read-only run that checks a brief's goal and criteria
    /// for contradictions after drafting and before the first implement
    /// attempt; `""` turns the check off.
    #[serde(default = "default_brief_check_route")]
    pub brief_check_route: String,
    /// Route id of the plan review: one read-only run that judges the
    /// planner's draft once per task. The review always runs: a `""`, missing
    /// or unavailable route falls back to the planner's route, with a
    /// `Plan gate:` decision line that says why.
    #[serde(default = "default_plan_review")]
    pub plan_review: String,
    /// Route the orchestrator agent runs on: its chat with the owner and,
    /// with `auto_answer`, its decisions on stuck questions. `""` (or an id that
    /// matches no route) means the standard tier's route.
    #[serde(default)]
    pub orchestrator: String,
    /// Whether the orchestrator answers a stuck agent's question itself
    /// before it reaches the owner. Off by default: the owner opts in to
    /// answers given on their behalf.
    #[serde(default)]
    pub auto_answer: bool,
    /// Local hour (0-23) at which the orchestrator chat is woken once a day
    /// to write the owner a digest; `None` turns the digest off.
    #[serde(default)]
    pub digest_hour: Option<u8>,
    /// While a question waits, wake the orchestrator chat again every this
    /// many hours with a reminder (needs `auto_answer`); 0 = off.
    #[serde(default)]
    pub wait_remind_every_hours: u32,
    /// Local `HH:MM-HH:MM` window in which the overseer tells the owner
    /// nothing (a notice waits for the digest or the first tick after it) and
    /// starts no reminder; `""` turns it off.
    #[serde(default = "default_quiet_hours")]
    pub quiet_hours: String,
    /// A question that waited longer than this many hours is answered with
    /// its cautious option, recorded as an assumption the owner can overturn;
    /// 0 = off. Owner-only kinds and questions with no cautious option keep waiting.
    #[serde(default)]
    pub wait_default_after_hours: u32,
    /// Routine task questions are answered by fixed rules and a cheap judge
    /// (at most three per task), each recorded as an assumption the owner can
    /// overturn. `false` sends every question to the owner as before.
    #[serde(default = "default_answer_policy")]
    pub answer_policy: bool,
    /// Flags new tasks start with unless `task.create` overrides them.
    #[serde(
        default = "default_experiments",
        deserialize_with = "experiments_or_default"
    )]
    pub experiments: Variant,
    /// Model id -> price, for harnesses that report no cost.
    #[serde(default = "default_prices")]
    pub prices: std::collections::BTreeMap<String, Price>,
    /// Repository-specific path fragments for `orchd ab`'s work breakdown.
    #[serde(default)]
    pub work_buckets: WorkBuckets,
    #[serde(default)]
    pub evolution: EvolutionSettings,
    /// Where new task worktrees go; a relative value is resolved against the
    /// repo root. Empty (the default) means `<home>/worktrees.noindex/<repo-id>`.
    /// Existing tasks keep the path they were created with.
    #[serde(default)]
    pub worktree_root: String,
    /// Land tasks may move the repo's default branch (origin/HEAD, else
    /// main/master). Off: such a landing is refused and the task stays done
    /// on its own branch.
    #[serde(default)]
    pub land_on_default: bool,
    /// Per repo path: whether landing on that repo's default branch is
    /// allowed. `land_on_default` decides for repos not in the map.
    #[serde(default)]
    pub land_on_default_repos: std::collections::BTreeMap<String, bool>,
    /// Turns off the pull request orchd opens by itself for a finished
    /// top-level task whose work did not land on its base: a read-only helper
    /// drafts it by the repo's own rules and orchd opens it. On by default.
    #[serde(default)]
    pub pr_on_done_off: bool,
    /// Per repo path: overrides `pr_on_done_off` (`false` turns it off for
    /// that repo, `true` on).
    #[serde(default)]
    pub pr_on_done_repos: std::collections::BTreeMap<String, bool>,
    /// Route that drafts the pull request. Empty: the `mechanical` tier's
    /// route, else the planner route.
    #[serde(default)]
    pub pr_route: String,
    /// Per repo path: where the whole-repo checks a subtask shares with its
    /// parent run. A repo not in the map uses `Parent`.
    #[serde(default)]
    pub gate_at: std::collections::BTreeMap<String, GateAt>,
    /// Commands run in the repo's main checkout after a task lands.
    #[serde(default)]
    pub after_land: Vec<AfterLand>,
    /// Start ready `next`-bucket backlog tasks by themselves while live task
    /// loops are below `parallel`.
    #[serde(default)]
    pub autopilot: bool,
    /// Commands that only run when the task's diff touches their `paths`.
    #[serde(default)]
    pub scoped_checks: Vec<ScopedCheck>,
    /// MCP servers the orchestrator chat may use, each toggled in Settings.
    #[serde(default)]
    pub chat_tools: Vec<ChatToolConfig>,
    /// Exact repo path -> connected tool ids enabled for task runs.
    #[serde(default)]
    pub mcp_enable: HashMap<String, Vec<String>>,
    /// Exact repo path -> server names excluded from task runs.
    #[serde(default)]
    pub mcp_disable: HashMap<String, Vec<String>>,
    /// Custom presets; the built-in ones live in code.
    #[serde(default)]
    pub presets: Vec<Preset>,
    /// Repo path -> preset id a new task there starts from.
    #[serde(default)]
    pub repo_presets: std::collections::BTreeMap<String, String>,
}

/// One connected tool of the orchestrator chat. `server` is an MCP server
/// definition (`{command, args, env}` or `{type: "http", url}`) or a
/// reference read at turn time, so no secret ever lands in these settings:
/// `{"ref": "claude-json:<name>"}` names an entry of `~/.claude.json`
/// `mcpServers`, `{"ref": "plugin:<plugin>/<server>"}` a server of an
/// installed Claude Code plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatToolConfig {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub enabled: bool,
    pub server: serde_json::Value,
    /// Per-tool override of the read/write guess: tool name -> "read" | "write".
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub overrides: std::collections::BTreeMap<String, String>,
}

/// One `settings.scopedChecks` entry: a command that is skipped unless a
/// changed file matches one of `paths`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScopedCheck {
    /// The repo it applies to; none = every repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// A glob over the whole command string.
    pub command: String,
    /// Globs over repo-relative changed files.
    pub paths: Vec<String>,
}

/// One `settings.afterLand` entry: `run` (a shell command) runs in the main
/// checkout `repo` after a landing there, e.g. to rebuild a binary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AfterLand {
    pub repo: String,
    pub run: String,
}

/// Tool-call inputs containing one of these count as process work (lesson
/// files, changelogs, convention scripts) or as evidence work (screenshot
/// helpers), on top of the generic rules in `ab.rs`. Empty by default: they
/// describe one repository, not orchd.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkBuckets {
    #[serde(default)]
    pub process: Vec<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// Where a subtask's finalVerify commands that its parent also runs are run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GateAt {
    /// Once, at the parent, on the final tree.
    Parent,
    /// In every subtask as well.
    Every,
}

fn default_quiet_hours() -> String {
    "23:00-08:00".to_string()
}

fn default_answer_policy() -> bool {
    true
}

/// Route the brief consistency check runs on unless settings name another:
/// `auto` is the cheapest configured route whose harness CLI is installed.
fn default_brief_check_route() -> String {
    "auto".to_string()
}

fn default_plan_review() -> String {
    "codex".to_string()
}

fn default_child_parallel() -> u32 {
    3
}

fn default_verify_timeout_secs() -> u64 {
    1200
}

fn default_plan_timeout_secs() -> u64 {
    1800
}

fn default_implement_budget_mins() -> f64 {
    45.0
}

fn default_planner() -> String {
    "claude-opus".to_string()
}

fn default_small_plan_max_chars() -> usize {
    1200
}

fn default_small_plan_max_paths() -> usize {
    3
}

impl Default for Settings {
    fn default() -> Self {
        let mut tiers = HashMap::new();
        tiers.insert(Tier::Mechanical, "codex".to_string());
        tiers.insert(Tier::Standard, "claude-sonnet".to_string());
        tiers.insert(Tier::Hard, "claude-opus".to_string());
        Settings {
            routes: vec![
                Route {
                    id: "claude-sonnet".to_string(),
                    label: "Claude Sonnet".to_string(),
                    harness: Harness::Claude,
                    model: Some("sonnet".to_string()),
                    effort: None,
                    profile_id: None,
                    account_id: None,
                    strength: None,
                },
                Route {
                    id: "claude-opus".to_string(),
                    label: "Claude Opus".to_string(),
                    harness: Harness::Claude,
                    model: Some("opus".to_string()),
                    effort: Some("high".to_string()),
                    profile_id: None,
                    account_id: None,
                    strength: None,
                },
                Route {
                    id: "codex".to_string(),
                    label: "Codex".to_string(),
                    harness: Harness::Codex,
                    model: None,
                    effort: None,
                    profile_id: None,
                    account_id: None,
                    strength: None,
                },
            ],
            tiers,
            review: "auto".to_string(),
            sandbox: SandboxMode::Native,
            sandbox_repos: BTreeMap::new(),
            // Task filesystem isolation leaves package registries reachable.
            allowed_domains: vec!["*".to_string()],
            codex_network: false,
            protected_paths: vec![],
            max_attempts: 4,
            parallel: 2,
            heavy_checks: None,
            daily_budget_usd: 0.0,
            child_parallel: default_child_parallel(),
            verify_timeout_secs: default_verify_timeout_secs(),
            plan_timeout_secs: default_plan_timeout_secs(),
            implement_budget_mins: default_implement_budget_mins(),
            planner: default_planner(),
            small_plan_max_chars: default_small_plan_max_chars(),
            small_plan_max_paths: default_small_plan_max_paths(),
            brief_check_route: default_brief_check_route(),
            plan_review: default_plan_review(),
            orchestrator: String::new(),
            auto_answer: false,
            digest_hour: None,
            wait_remind_every_hours: 0,
            quiet_hours: default_quiet_hours(),
            wait_default_after_hours: 0,
            answer_policy: true,
            experiments: default_experiments(),
            prices: default_prices(),
            work_buckets: WorkBuckets::default(),
            evolution: EvolutionSettings::default(),
            worktree_root: String::new(),
            land_on_default: false,
            land_on_default_repos: Default::default(),
            pr_on_done_off: false,
            pr_on_done_repos: Default::default(),
            pr_route: String::new(),
            gate_at: Default::default(),
            after_land: vec![],
            scoped_checks: vec![],
            autopilot: false,
            chat_tools: vec![],
            mcp_enable: HashMap::new(),
            mcp_disable: HashMap::new(),
            presets: vec![],
            repo_presets: Default::default(),
        }
    }
}

fn default_experiments() -> Variant {
    Variant {
        loop_detect: true,
        land: true,
        ..Variant::default()
    }
}

/// A settings file written before `loopDetect` existed keeps the detector on:
/// a missing flag in `experiments` reads as the settings default, not false.
fn experiments_or_default<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Variant, D::Error> {
    let mut v = serde_json::Value::deserialize(d)?;
    if let Some(o) = v.as_object_mut() {
        o.entry("loopDetect")
            .or_insert(serde_json::Value::Bool(true));
        o.entry("land").or_insert(serde_json::Value::Bool(true));
    }
    serde_json::from_value(v).map_err(serde::de::Error::custom)
}

impl Settings {
    pub fn auto_heavy_checks(cores: u32) -> u32 {
        (cores / 5).max(2)
    }
}

#[cfg(test)]
#[path = "tests/settings.rs"]
mod tests;
