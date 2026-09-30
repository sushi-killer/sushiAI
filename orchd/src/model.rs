//! Wire types for the orchestrator protocol and stored state. Field names
//! are camelCase to match `artifacts/tasks/orchestrator-mvp.md` exactly,
//! since the Electron/UI lane (Lane B) serializes/deserializes the same
//! shapes independently.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub id: String,
    pub label: String,
    pub harness: Harness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Claude only: a named model-profile (env/apiKeyHelper) merged into the
    /// run's `settings.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// 1 (cheap, mechanical) to 3 (hard). Review routing never picks a route
    /// weaker than the implementer's; unset uses `route_strength`'s default
    /// for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<u32>,
}

/// The route's explicit strength, else a default from its model:
/// haiku/luna/mini -> 1, opus -> 3, anything else (or no model) -> 2.
pub fn route_strength(route: &Route) -> u32 {
    if let Some(s) = route.strength {
        return s;
    }
    let model = route.model.as_deref().unwrap_or_default().to_lowercase();
    if ["haiku", "luna", "mini"].iter().any(|k| model.contains(k)) {
        1
    } else if model.contains("opus") {
        3
    } else {
        2
    }
}

/// Input + output price per million tokens of the route's model: `price_for`,
/// else the cheapest price key holding the bare alias (`sonnet`) as a
/// dash-separated segment. `None` without a model or a match.
pub fn route_cost(
    prices: &std::collections::BTreeMap<String, Price>,
    route: &Route,
) -> Option<f64> {
    let model = route.model.as_deref()?;
    let cost = |p: &Price| p.input + p.output;
    if let Some(p) = price_for(prices, model) {
        return Some(cost(&p));
    }
    prices
        .iter()
        .filter(|(key, _)| key.split('-').any(|seg| seg == model))
        .map(|(_, p)| cost(p))
        .min_by(|a, b| a.total_cmp(b))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Mechanical,
    Standard,
    Hard,
}

impl Tier {
    /// mechanical -> standard -> hard -> hard (caps out).
    pub fn up(self) -> Tier {
        match self {
            Tier::Mechanical => Tier::Standard,
            Tier::Standard => Tier::Hard,
            Tier::Hard => Tier::Hard,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Mechanical => "mechanical",
            Tier::Standard => "standard",
            Tier::Hard => "hard",
        }
    }
}

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
    /// Route id; "" = off, "auto" = first configured route whose harness
    /// differs from the implement attempt's.
    pub review: String,
    pub sandbox: SandboxMode,
    pub allowed_domains: Vec<String>,
    /// Codex's sandbox has no per-domain filter, just a network on/off
    /// switch (spec assumption); this is that switch. Default `false` --
    /// `allowedDomains` only ever governs Claude's allowlist.
    #[serde(default)]
    pub codex_network: bool,
    pub protected_paths: Vec<String>,
    pub max_attempts: u32,
    pub parallel: u32,
    /// Dollars a UTC day may spend across every repo and stage before orchd
    /// asks (a `daily_budget` question) instead of starting a plan run or
    /// implement attempt; 0 = off.
    #[serde(default)]
    pub daily_budget_usd: f64,
    /// At most this many subtasks of one parent run at once (`parallel`
    /// stays the global cap).
    #[serde(default = "default_child_parallel")]
    pub child_parallel: u32,
    /// Route id used for the drafting/plan stage of a `{repo, request}`
    /// `task.create`; `""` turns planning off (that create form is then
    /// rejected -- there is nothing to run it with).
    #[serde(default = "default_planner")]
    pub planner: String,
    /// Route id of the read-only run that checks a brief's goal and criteria
    /// for contradictions after drafting and before the first implement
    /// attempt; `""` turns the check off.
    #[serde(default = "default_brief_check_route")]
    pub brief_check_route: String,
    /// Route the orchestrator agent runs on: its chat with the owner and,
    /// with `auto_answer`, its triage of stuck questions. `""` (or an id that
    /// matches no route) means the standard tier's route.
    #[serde(default)]
    pub orchestrator: String,
    /// Whether the orchestrator answers a stuck agent's question itself
    /// before it reaches the owner. Off by default: the owner opts in to
    /// answers given on their behalf.
    #[serde(default)]
    pub auto_answer: bool,
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
    /// repo root. Existing tasks keep the path they were created with.
    #[serde(default = "default_worktree_root")]
    pub worktree_root: String,
    /// Land tasks may move the repo's default branch (origin/HEAD, else
    /// main/master). Off: such a landing is refused and the task stays done
    /// on its own branch.
    #[serde(default)]
    pub land_on_default: bool,
    /// Commands run in the repo's main checkout after a task lands.
    #[serde(default)]
    pub after_land: Vec<AfterLand>,
    /// Start ready `next`-bucket backlog tasks by themselves while live task
    /// loops are below `parallel`.
    #[serde(default)]
    pub autopilot: bool,
    /// Commands that only run when the task's diff touches their `paths`.
    #[serde(default = "default_scoped_checks")]
    pub scoped_checks: Vec<ScopedCheck>,
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

fn default_scoped_checks() -> Vec<ScopedCheck> {
    vec![ScopedCheck {
        repo: None,
        command: "npm run test:desktop".to_string(),
        paths: [
            "src/app/**",
            "src/extensions/**",
            "electron/**",
            "src/styles/**",
            "*.html",
        ]
        .iter()
        .map(|p| p.to_string())
        .collect(),
    }]
}

/// One `settings.afterLand` entry: `run` (a shell command) runs in the main
/// checkout `repo` after a landing there, e.g. to rebuild a binary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AfterLand {
    pub repo: String,
    pub run: String,
}

/// Thresholds for the evolution loop (`engine/evolution`): what counts as
/// enough evidence and how many proposals one round may make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EvolutionSettings {
    /// Earlier tasks a repo needs before per-repo baselines mean anything.
    pub min_tasks: u32,
    /// Tool calls a cluster of signals must have wasted to be worth a proposal.
    pub min_wasted_calls: u32,
    pub min_wasted_usd: f64,
    pub max_proposals: u32,
    /// Route id that writes proposals; "" means the hard tier's route.
    pub proposer_route: String,
    /// Tasks after which an accepted change is judged and possibly reverted.
    pub revert_after_tasks: u32,
}

impl Default for EvolutionSettings {
    fn default() -> Self {
        EvolutionSettings {
            min_tasks: 3,
            min_wasted_calls: 20,
            min_wasted_usd: 1.0,
            max_proposals: 3,
            proposer_route: String::new(),
            revert_after_tasks: 10,
        }
    }
}

/// Which side of the line a proposed change lands on: orchd's own
/// harness/settings, or the audited repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    Harness,
    Repo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalForm {
    Script,
    Test,
    Lint,
    Doc,
    Command,
    Skill,
    Prompt,
    Gate,
    Routing,
    Default,
}

/// What a proposer run hands back in its ```sushi-proposal block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalReply {
    pub track: Track,
    pub form: ProposalForm,
    pub change: String,
    pub evidence: String,
    pub metric: String,
    pub test: String,
    /// The experiment flags of the harness-track change, as an `orchd eval`
    /// arm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Proposed,
    Rejected,
    Approved,
    Adopted,
    RevertSuggested,
}

/// Finished tasks of a repo on one side of an adoption, and the signals of
/// the proposal's cluster those tasks left.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalCounts {
    pub tasks: u32,
    pub signals: u32,
}

/// One evolution proposal, stored at `<data>/evolution/proposals/<id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    pub cluster_key: String,
    pub kind: String,
    pub repo: String,
    pub track: Track,
    pub form: ProposalForm,
    pub change: String,
    pub evidence: String,
    pub metric: String,
    pub test: String,
    pub status: ProposalStatus,
    pub created_at: i64,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ProposalCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ProposalCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One owner-approved standing note about a repo, stored in
/// `<data>/repo-notes.json` and shown to the planner. `source` is `owner`
/// or `proposal:<id>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoNote {
    pub id: String,
    pub text: String,
    pub source: String,
    pub created_at: i64,
}

fn default_worktree_root() -> String {
    ".sushiai/worktrees".to_string()
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

fn default_answer_policy() -> bool {
    true
}

/// Route the brief consistency check runs on unless settings name another:
/// the route `Settings::default` gives the mechanical tier.
fn default_brief_check_route() -> String {
    "codex".to_string()
}

/// State of a task's brief consistency check (a cheap read-only model run
/// asking whether the goal and criteria contradict each other).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefCheck {
    /// The check has run (or been skipped for good); it never runs twice.
    #[serde(default)]
    pub done: bool,
    /// The planner already redrafted once because of a contradiction.
    #[serde(default)]
    pub redrafted: bool,
    /// The conflict the implement and review briefs must name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<String>,
    /// The command the planner named that captures the screenshot evidence;
    /// orchd runs it itself once verify has passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
    /// Every check command was run on the base once (see `base_check`).
    #[serde(default)]
    pub feasibility_done: bool,
    /// Commands that fail on the base for a reason unrelated to the task: kept
    /// out of the gates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub non_gating: Vec<String>,
    /// Criteria the repeated-unmet judge has already ruled on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub judged: Vec<String>,
    /// How many `decisions` the owner-answer check has already read.
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub answers_seen: usize,
    /// Findings that did not cost an attempt (P2, or unrelated to the task),
    /// for the report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub follow_ups: Vec<String>,
}

fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

impl BriefCheck {
    pub fn is_default(&self) -> bool {
        *self == BriefCheck::default()
    }
}

fn default_child_parallel() -> u32 {
    3
}

fn default_planner() -> String {
    "claude-opus".to_string()
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
                    strength: None,
                },
                Route {
                    id: "claude-opus".to_string(),
                    label: "Claude Opus".to_string(),
                    harness: Harness::Claude,
                    model: Some("opus".to_string()),
                    effort: Some("high".to_string()),
                    profile_id: None,
                    strength: None,
                },
                Route {
                    id: "codex".to_string(),
                    label: "Codex".to_string(),
                    harness: Harness::Codex,
                    model: None,
                    effort: None,
                    profile_id: None,
                    strength: None,
                },
            ],
            tiers,
            review: "auto".to_string(),
            sandbox: SandboxMode::Native,
            // Open network by default: the sandbox's job is keeping writes
            // inside the worktree, not blocking package registries.
            allowed_domains: vec!["*".to_string()],
            codex_network: false,
            protected_paths: vec![],
            max_attempts: 4,
            parallel: 2,
            daily_budget_usd: 0.0,
            child_parallel: default_child_parallel(),
            planner: default_planner(),
            brief_check_route: default_brief_check_route(),
            orchestrator: String::new(),
            auto_answer: false,
            answer_policy: true,
            experiments: default_experiments(),
            prices: default_prices(),
            work_buckets: WorkBuckets::default(),
            evolution: EvolutionSettings::default(),
            worktree_root: default_worktree_root(),
            land_on_default: false,
            after_land: vec![],
            scoped_checks: default_scoped_checks(),
            autopilot: false,
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

/// The experiment flags a task runs with. `Settings::experiments` is the
/// default; `task.create {variant}` overrides it per task, and the task
/// keeps its own copy, so an A/B pair can run side by side and `orchd ab`
/// groups results by it. A flag that wins becomes the plain behaviour and
/// leaves this struct.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Variant {
    /// Kill an implement attempt whose harness prints nothing for this
    /// long; 0 = off. The clock pauses during orchd's own Stop-hook verify,
    /// but not during an agent's long Bash call (up to 600s), so values
    /// under ~15 min can kill a busy agent.
    pub stall_timeout_secs: u64,
    /// The reviewer gets the screenshots the attempt saved (Codex as
    /// attachments, Claude as paths to open) and longer verify output.
    pub review_evidence: bool,
    /// After an implement attempt fails, one read-only call on the planner's
    /// route diagnoses it; the answer goes into the next attempt's brief.
    pub advisor: bool,
    /// Kill an implement attempt that repeats itself (the same tool call
    /// three times, three error results, or eight edits of one file with no
    /// command run between) and record failure kind `loop`. On in the settings defaults.
    pub loop_detect: bool,
    /// Route id the plan stage runs on instead of `settings.planner`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planner_route: Option<String>,
    /// Route ids the implement stage uses instead of `settings.tiers`, per
    /// tier; a tier missing here keeps the settings' route. A `BTreeMap` so
    /// the keys serialize in one order and `orchd ab` groups identical
    /// variants together.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub tier_routes: BTreeMap<Tier, String>,
    /// A dollar budget: once `task.cost_usd` reaches it, the task waits for
    /// the owner before its next plan, implement, advisor or review run
    /// (never mid-run); "raise" adds this amount again. 0 = no budget, and
    /// left out of the JSON so a budget-less variant serializes as before.
    #[serde(skip_serializing_if = "is_no_budget")]
    pub max_cost_usd: f64,
    /// A dollar cap on one implement attempt: past it the run is stopped and
    /// fails with kind `budget`. Checked while the run streams, unlike
    /// `max_cost_usd`. 0 = none, left out of the JSON.
    #[serde(skip_serializing_if = "is_no_budget")]
    pub max_attempt_cost_usd: f64,
    /// The planner marks each question with a recommended option and whether
    /// it is blocking (irreversible or consequential). Non-blocking ones are
    /// not asked: each becomes an assumption on the task and the task goes
    /// on; blocking ones go to triage and the owner as one question. Left
    /// out of the JSON while off.
    #[serde(skip_serializing_if = "is_false")]
    pub batch_questions: bool,
    /// The planner writes executable checks per criterion; the ones that
    /// fail on the base gate every attempt, and one hidden held-out check
    /// is run after verify without the implementer ever seeing it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub grounded_checks: bool,
    /// 2 = on, 0/1 = off. On the hard tier the first implement attempt runs
    /// twice at once (the task's worktree and a sibling `-b` one, on the
    /// tier route and on `best_of_route`); each candidate runs verify and
    /// the grounded checks, and the passing one wins, the other-family
    /// reviewer picking when both pass. Later attempts are single.
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub best_of: u32,
    /// Route id of the second candidate; default the first route on the
    /// other harness than the tier route's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_of_route: Option<String>,
    /// A finished top-level task lands on its base branch by itself: a
    /// landing queue carries it onto the branch's head, squashes it to one
    /// commit, re-runs the checks on that tree and moves the branch. On in the
    /// settings defaults, so off is always written to the JSON.
    pub land: bool,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

#[cfg(test)]
mod price_tests {
    use super::*;

    #[test]
    fn codex_cost_bills_cached_tokens_at_the_cached_rate_once() {
        let p = default_prices()["gpt-5.6-luna"];
        // 359,428 input of which 324,864 cached, 3,869 output (a real run).
        let cost = p.codex_cost(359_428, 324_864, 3_869);
        let expected = (34_564.0 * 0.20 + 324_864.0 * 0.02 + 3_869.0 * 1.20) / 1e6;
        assert!((cost - expected).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn claude_prices_match_a_dated_model_id_and_the_longest_family() {
        let prices = default_prices();
        let haiku = price_for(&prices, "claude-haiku-4-5-20251001").unwrap();
        assert_eq!(haiku.input, 1.0);
        assert_eq!(price_for(&prices, "claude-sonnet-5-5").unwrap().input, 2.0);
        assert_eq!(
            price_for(&prices, "claude-opus-5-5-20260901")
                .unwrap()
                .output,
            20.0
        );
        assert!(price_for(&prices, "claude-opus-5").is_none());
        assert!(price_for(&prices, "claude-sonnet-50").is_none());
        let opus = prices["claude-opus-5-5"];
        let cost = opus.claude_cost(1_000_000, 1_000_000, 1_000_000, 1_000_000);
        assert!((cost - (4.0 + 5.0 + 0.20 + 20.0)).abs() < 1e-9, "{cost}");
        // Codex prices keep no cache-write price and stay off the wire.
        let v = serde_json::to_value(prices["gpt-5.6-luna"]).unwrap();
        assert!(v.get("cacheWrite").is_none());
        assert_eq!(serde_json::to_value(opus).unwrap()["cacheWrite"], 5.0);
    }
}

/// Where a task can start: the orchestrator chat, the panel, an MCP client,
/// the CLI, the planner splitting a parent, an eval run, or another task's
/// handoff.
pub const TASK_SOURCES: [&str; 7] = ["chat", "ui", "mcp", "cli", "planner", "eval", "handoff"];

impl Task {
    pub fn variant(&self) -> Variant {
        self.variant.clone().unwrap_or_default()
    }

    /// The dollar budget in force, with the owner's raises; `None` = none.
    pub fn cost_budget(&self) -> Option<f64> {
        let step = self.variant().max_cost_usd;
        (step > 0.0).then(|| step * (1 + self.budget_raises) as f64)
    }
}

/// Per-million-token list prices for a model whose harness reports tokens
/// but no cost (Codex), or a Claude run that ended before its `result`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub input: f64,
    /// Cache reads.
    pub cached_input: f64,
    pub output: f64,
    /// Cache writes (Claude's `cache_creation_input_tokens`); `None` bills
    /// them at the input price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

/// The price of `model`: an exact key, else the longest key it extends by a
/// `-suffix` (`claude-haiku-4-5` prices `claude-haiku-4-5-20251001`).
pub fn price_for(prices: &std::collections::BTreeMap<String, Price>, model: &str) -> Option<Price> {
    if let Some(p) = prices.get(model) {
        return Some(*p);
    }
    prices
        .iter()
        .filter(|(key, _)| {
            model
                .strip_prefix(key.as_str())
                .is_some_and(|rest| rest.starts_with('-'))
        })
        .max_by_key(|(key, _)| key.len())
        .map(|(_, p)| *p)
}

impl Price {
    /// Claude's `input_tokens` excludes cache writes and reads.
    pub fn claude_cost(&self, input: u64, cache_write: u64, cache_read: u64, output: u64) -> f64 {
        (input as f64 * self.input
            + cache_write as f64 * self.cache_write.unwrap_or(self.input)
            + cache_read as f64 * self.cached_input
            + output as f64 * self.output)
            / 1_000_000.0
    }

    /// Codex's `input_tokens` already includes the cached ones.
    pub fn codex_cost(&self, input: u64, cached: u64, output: u64) -> f64 {
        let fresh = input.saturating_sub(cached) as f64;
        (fresh * self.input + cached as f64 * self.cached_input + output as f64 * self.output)
            / 1_000_000.0
    }
}

// ponytail: list prices as of 2026-09; `settings.prices` overrides them.
fn default_prices() -> std::collections::BTreeMap<String, Price> {
    let codex = [
        ("gpt-5.3-codex", 1.75, 0.175, 14.0),
        ("gpt-5.6-luna", 0.20, 0.02, 1.20),
    ]
    .map(|(m, input, cached_input, output)| (m, input, None, cached_input, output));
    let claude = [
        ("claude-opus-5-5", 4.0, 5.0, 0.20, 20.0),
        ("claude-sonnet-5-5", 2.0, 2.50, 0.20, 10.0),
        ("claude-sonnet-5", 2.0, 2.50, 0.20, 10.0),
        ("claude-haiku-4-5", 1.0, 1.25, 0.10, 5.0),
    ]
    .map(|(m, input, write, read, output)| (m, input, Some(write), read, output));
    codex
        .into_iter()
        .chain(claude)
        .map(|(m, input, cache_write, cached_input, output)| {
            (
                m.to_string(),
                Price {
                    input,
                    cached_input,
                    output,
                    cache_write,
                },
            )
        })
        .collect()
}

fn is_no_budget(v: &f64) -> bool {
    *v == 0.0
}

/// Keeps `Instant + timeout` from overflowing.
const MAX_STALL_TIMEOUT_SECS: u64 = 24 * 3600;

impl Variant {
    pub fn check(&self) -> Result<(), String> {
        if !self.max_cost_usd.is_finite() || self.max_cost_usd < 0.0 {
            return Err("maxCostUsd must be a non-negative number".to_string());
        }
        if !self.max_attempt_cost_usd.is_finite() || self.max_attempt_cost_usd < 0.0 {
            return Err("maxAttemptCostUsd must be a non-negative number".to_string());
        }
        let s = self.stall_timeout_secs;
        if s > MAX_STALL_TIMEOUT_SECS {
            return Err(format!(
                "stallTimeoutSecs must be at most {MAX_STALL_TIMEOUT_SECS}"
            ));
        }
        Ok(())
    }

    /// Flags that won or lost and left the struct. An old `task.json` or
    /// `settings.json` naming one still loads (serde ignores it); a variant
    /// override naming one is rejected.
    pub const RETIRED_KEYS: [&'static str; 7] = [
        "retryMode",
        "plannerTier",
        "contract",
        "reviewOtherFamily",
        "deferHeavyChecks",
        "leanOutput",
        "reviewBlind",
    ];

    /// Keys a serialized default `Variant` leaves out, but a partial
    /// override object may still name.
    pub const OPTIONAL_KEYS: [&'static str; 9] = [
        "plannerRoute",
        "tierRoutes",
        "maxCostUsd",
        "maxAttemptCostUsd",
        "batchQuestions",
        "groundedChecks",
        "bestOf",
        "bestOfRoute",
        "land",
    ];

    /// Every route override names a route in `routes`.
    pub fn check_routes(&self, routes: &[Route]) -> Result<(), String> {
        let known = |id: &str| routes.iter().any(|r| r.id == id);
        if let Some(id) = self.planner_route.as_deref().filter(|id| !known(id)) {
            return Err(format!(
                "variant plannerRoute \"{id}\" is not a configured route"
            ));
        }
        if let Some(id) = self.best_of_route.as_deref().filter(|id| !known(id)) {
            return Err(format!(
                "variant bestOfRoute \"{id}\" is not a configured route"
            ));
        }
        for (tier, id) in &self.tier_routes {
            if !known(id) {
                return Err(format!(
                    "variant tierRoutes.{} \"{id}\" is not a configured route",
                    tier.as_str()
                ));
            }
        }
        Ok(())
    }

    /// The implement route for `tier`: this variant's override, else the
    /// settings' tier route, else `codex`. `true` when the override applied.
    pub fn implement_route_id(&self, settings: &Settings, tier: Tier) -> (String, bool) {
        match self.tier_routes.get(&tier) {
            Some(id) => (id.clone(), true),
            None => (
                settings
                    .tiers
                    .get(&tier)
                    .cloned()
                    .unwrap_or_else(|| "codex".to_string()),
                false,
            ),
        }
    }

    /// The plan stage's route id: this variant's override, else
    /// `settings.planner` (`""` = planning off).
    pub fn plan_route_id<'a>(&'a self, settings: &'a Settings) -> &'a str {
        self.planner_route.as_deref().unwrap_or(&settings.planner)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// A `{repo, request}` task.create is drafting its own title/goal/
    /// criteria/verify (and asking any clarifying questions) before it ever
    /// reaches `queued`.
    Drafting,
    Queued,
    Running,
    Waiting,
    /// Done and checked, waiting for its base branch's checkout to be clean
    /// so it can land (`variant.land`); retried every 2 minutes and on
    /// `task.start`.
    Landing,
    Done,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub text: String,
    pub options: Vec<String>,
    /// What the question is about; the answer policy picks its rule by it.
    #[serde(default)]
    pub kind: QuestionKind,
    /// The stage that asked it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_by: Option<AskedBy>,
    /// When it was asked (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<i64>,
}

impl Question {
    /// A question asked now by `asked_by`; every asking site goes through it.
    pub fn new(
        text: impl Into<String>,
        options: Vec<String>,
        kind: QuestionKind,
        asked_by: AskedBy,
    ) -> Self {
        Question {
            text: text.into(),
            options,
            kind,
            asked_by: Some(asked_by),
            asked_at: Some(now_ms()),
        }
    }
}

/// The stage that asked a question. `brief`, `advisor` and `land` are
/// reserved for the UI: nothing asks from them yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AskedBy {
    Brief,
    Plan,
    Implement,
    Verify,
    Review,
    Advisor,
    Land,
}

/// Who answered a question kept in the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnsweredBy {
    Owner,
    Policy,
    Judge,
    Orchestrator,
}

/// An answered question, kept in `Task.questionHistory`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnsweredQuestion {
    pub question: String,
    pub options: Vec<String>,
    pub kind: QuestionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_by: Option<AskedBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<i64>,
    pub answer: String,
    pub answered_at: i64,
    pub answered_by: AnsweredBy,
}

/// Why orchd asks a question, set where it is asked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    AttemptsFailing,
    ReviewNoVerdict,
    DependencyEnded,
    Impossible,
    PreexistingFailure,
    Budget,
    ProtectedPath,
    /// A review finding the implementer disputed and a judge dropped.
    ReviewDispute,
    PlanQuestion,
    /// Today's spend reached `dailyBudgetUsd`; never answered by policy.
    DailyBudget,
    #[default]
    AgentQuestion,
}

/// A review finding the implementer disputes, with its rebuttal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dispute {
    pub finding: String,
    pub rebuttal: String,
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// A choice the planner made itself instead of asking the owner
/// (`variant.batch_questions`). The owner can overturn it later with
/// `task.overturn`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assumption {
    pub question: String,
    /// The planner's recommended option.
    pub answer: String,
    pub evidence: String,
    /// `planner`, or for the answer policy `policy` (a rule) / `judge`.
    pub by: String,
    /// The kind of question the answer policy answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<QuestionKind>,
    /// The attempt it was asked in (answer policy only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    pub overturned: bool,
    /// What the owner answered instead, once overturned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_answer: Option<String>,
}

/// How a check ran on the base checkout, before any work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Baseline {
    /// Already passes: it proves nothing, so it does not gate.
    Pass,
    /// Fails as it should: it gates the attempt.
    Fail,
    /// Could not run (missing tool, spawn failure): left out.
    Env,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// Index into `Task::criteria`.
    pub criterion: usize,
    pub run: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<Baseline>,
}

/// Drops checks whose criterion is out of range or whose command is blank,
/// and clears any supplied baseline: only the base run may set it.
pub fn valid_checks(checks: Vec<Check>, criteria_len: usize) -> Vec<Check> {
    checks
        .into_iter()
        .filter(|c| c.criterion < criteria_len && !c.run.trim().is_empty())
        .map(|c| Check {
            baseline: None,
            ..c
        })
        .collect()
}

/// [`valid_checks`] for the one held-out check.
pub fn valid_check(check: Option<Check>, criteria_len: usize) -> Option<Check> {
    valid_checks(check.into_iter().collect(), criteria_len)
        .into_iter()
        .next()
}

/// The autonomy metric: a person or the lead session had to fix a task's
/// work after orchd said done. `by` is `owner` (`task.leadTouch`) or `auto`
/// (the landing was rewritten, or someone else edited its files soon after).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeadTouch {
    pub touched: bool,
    #[serde(default)]
    pub note: String,
    pub at: i64,
    pub by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub goal: String,
    pub criteria: Vec<String>,
    /// Criteria the planner marked `visual` (kept by text, so removing a
    /// criterion never shifts them); others are visual by their `check:`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visual_criteria: Vec<String>,
    pub verify: Vec<String>,
    /// Slow checks (full CI, desktop smoke) run once, after review passes
    /// and before the commit; `verify` runs after every attempt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub final_verify: Vec<String>,
    /// Executable checks, each tied to one criterion by index
    /// (`variant.grounded_checks`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    /// One extra check the implementer never sees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_out: Option<Check>,
    /// The owner's raw one-sentence ask, set only by the `{repo, request}`
    /// form of `task.create`; the drafting stage fills `title`/`goal`/
    /// `criteria`/`verify` from it and leaves this as the original text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    pub repo: String,
    pub worktree: String,
    /// The worktree directory was removed because the task no longer needs
    /// it (done, archived, or gc); it is recreated from the branch when the
    /// task runs again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub worktree_removed: bool,
    /// The commit a `variant.land` task put on its base branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed_sha: Option<String>,
    /// When the task landed (ms since epoch), set beside `landed_sha`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed_at: Option<i64>,
    /// The task's branch against its base (the landing commit against its
    /// parent once landed); refreshed after each implement attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_stat: Option<DiffStat>,
    /// The markdown report of a finished top-level task (`report.rs`), also
    /// kept in `<data>/tasks/<id>/report.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    /// When the report was first written; the panel raises its notice only
    /// for a fresh one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_at: Option<i64>,
    /// Whether the work needed a fix after orchd said done; absent = unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_touch: Option<LeadTouch>,
    /// The task whose owner mark ("needed a fix") created this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up_of: Option<String>,
    /// The follow-up tasks the owner's marks on this task created.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub follow_ups: Vec<String>,
    pub branch: String,
    pub base_sha: String,
    /// The branch the task was started from (`base`, or the repo's checked-out
    /// branch). When it moves ahead, the task's work is carried onto it before
    /// verify; `None` (a detached checkout, or a task from before this field)
    /// never rebases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    /// Ids of tasks that must be `done` before this one starts implementing
    /// (it may still draft its plan meanwhile).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Repo-relative files or directories the planner said this subtask
    /// edits; siblings that overlap are serialised.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// The task this one is a part of: it branches from that task's branch
    /// and, once done, its commit lands there. A task with children runs no
    /// implement attempt of its own; it is done when every child has landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub status: TaskStatus,
    pub tier: Tier,
    /// The tier the planner chose; `None` when the task was not planned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_tier: Option<Tier>,
    /// Set to `no planner tier` when the implement tier fell back to
    /// `Standard` because the planner chose none; cleared when the planner's
    /// tier is used, or a failure moves the task up a tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier_fallback: Option<String>,
    /// `None` only on tasks created before variants existed; `orchd ab`
    /// keeps those out of every arm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<Variant>,
    /// The eval set (`orchd eval run`) this task replays, and the task's name
    /// in it; `orchd ab --eval` reports by these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_name: Option<String>,
    /// Shell command that grades this eval task's final commit (from the
    /// set's `check`); run from the repo root in a throwaway worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_check_cmd: Option<String>,
    /// Where the work started, one of `TASK_SOURCES`; absent on tasks
    /// created before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// What that check returned; a task counts as a success only when it is
    /// done and this is `code == 0` (or there is no check command).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_check: Option<EvalCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<Question>,
    /// Where the brief consistency check stands for this task.
    #[serde(default, skip_serializing_if = "BriefCheck::is_default")]
    pub brief_check: BriefCheck,
    /// Where the task stands in orchd's file queue: why it waits, and the
    /// relay it belongs to. Flat in the JSON.
    #[serde(flatten)]
    pub queue: QueueState,
    #[serde(default)]
    pub decisions: Vec<String>,
    /// Non-blocking planner questions answered by their recommendation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<Assumption>,
    /// Every answered question, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub question_history: Vec<AnsweredQuestion>,
    /// Keys of the disputed review findings a judge already ruled on: each
    /// is judged at most once per task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub judged_findings: Vec<String>,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    #[serde(default)]
    pub cost_usd: f64,
    /// How many times the owner raised the `variant.max_cost_usd` budget;
    /// the budget in force is that amount times `1 + budget_raises`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub budget_raises: u32,
    /// The UTC day (`YYYY-MM-DD`) the owner answered "run anyway" to the
    /// daily budget question: no more asking that day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_budget_ok_day: Option<String>,
    /// Hides the task from the default `task.list` without deleting it.
    /// `#[serde(default)]` so a `task.json` written before this field
    /// existed still loads, with `archived: false`.
    #[serde(default)]
    pub archived: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A task's place in the file queue (`engine/leases.rs`, `engine/graph.rs`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueState {
    /// Why the task sits `queued`: it waits for another task's lease
    /// ("waits for <task> on <path>"). Cleared when it starts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_reason: Option<String>,
    /// The task waited for a lease: its worktree is carried onto the base
    /// head before its first attempt.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub waited_on_lease: bool,
    /// The sibling subtask whose branch this one continues (a relay): it
    /// starts from that subtask's commit instead of the parent's head.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_of: Option<String>,
    /// Where the relay's chain began; the last link lands the whole chain
    /// on the parent as one commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_base: Option<String>,
    /// The planning backlog the task sits in until `task.start` or the
    /// autopilot starts it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backlog: Option<Backlog>,
}

/// Which planning bucket a backlog task is in; the autopilot only starts
/// `next`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BacklogBucket {
    Next,
    Later,
}

/// A task's place in the planning backlog: its bucket and its order within
/// it (ascending).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backlog {
    pub bucket: BacklogBucket,
    pub order: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    /// Drafting: a fresh read-only session that fills in title/goal/
    /// criteria/verify from the owner's one-sentence request.
    Plan,
    Implement,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttemptStatus {
    Running,
    Passed,
    Failed,
    Interrupted,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalCheck {
    pub code: Option<i32>,
    pub tail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyOutcome {
    pub command: String,
    pub code: Option<i32>,
    pub tail: String,
    pub ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Verdict {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewResult {
    pub verdict: Verdict,
    #[serde(default)]
    pub findings: Vec<String>,
    /// The findings the reviewer marked `repeat: true`: still present after
    /// the previous attempt's review reported them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repeated: Vec<String>,
    /// The severity (0-3) of each finding by position, `None` when it named
    /// none; findings past the end (criterion rulings) have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub severities: Vec<Option<u8>>,
    /// The reviewer's ruling on each criterion it named, `met: true` too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<CriterionRuling>,
}

/// One criterion ruling of a review: `met` is `None` when the reviewer could
/// not check it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CriterionRuling {
    pub criterion: String,
    #[serde(default)]
    pub met: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

/// Image extensions an evidence screenshot may have, with their MIME types.
pub const IMAGE_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
    ("svg", "image/svg+xml"),
    ("bmp", "image/bmp"),
    ("avif", "image/avif"),
];

/// The MIME type of an image path, if its extension is an evidence image type.
pub fn image_mime(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    IMAGE_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

/// Words that make an `artifacts/` path in a check an image.
const IMAGE_WORDS: &[&str] = &[
    "image",
    "img",
    "picture",
    "photo",
    "snapshot",
    "capture",
    "visual",
    "thumbnail",
    "render",
];

/// The text after `-- check:`, lowercased with `-` and `_` made spaces.
fn normalized_check(text: &str) -> Option<String> {
    let (_, check) = text.split_once("-- check:")?;
    Some(check.to_ascii_lowercase().replace(['-', '_'], " "))
}

/// Whether a normalized check mentions a screenshot, an image word or an
/// image extension.
fn mentions_image(check: &str) -> bool {
    check.contains("screenshot")
        || check.contains("screen shot")
        || IMAGE_WORDS.iter().any(|word| check.contains(word))
        || IMAGE_TYPES.iter().any(|(ext, _)| check.contains(ext))
}

/// A criterion is visual when its `-- check:` names a screenshot or an image
/// under `artifacts/`.
pub fn is_visual_criterion(text: &str) -> bool {
    let Some(check) = normalized_check(text) else {
        return false;
    };
    check.contains("screenshot")
        || check.contains("screen shot")
        || (check.contains("artifacts/") && IMAGE_WORDS.iter().any(|word| check.contains(word))
            || IMAGE_TYPES.iter().any(|(ext, _)| check.contains(ext)))
}

/// A criterion whose `-- check:` runs a test or build command and mentions no
/// image: its proof is the command's output, never a picture.
pub fn is_command_check(text: &str) -> bool {
    let Some(check) = normalized_check(text) else {
        return false;
    };
    if mentions_image(&check) {
        return false;
    }
    const TOOLS: &[&str] = &[
        "cargo", "npm", "npx", "pnpm", "yarn", "node", "pytest", "make", "jest", "vitest",
    ];
    let has_word = check
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| TOOLS.contains(&w));
    has_word
        || ["go test", "playwright test", "test runner"]
            .iter()
            .any(|p| check.contains(p))
}

impl Task {
    /// The criteria that need an image as evidence.
    pub fn visual_criteria_texts(&self) -> Vec<&str> {
        self.criteria
            .iter()
            .filter(|c| {
                !is_command_check(c) && (self.visual_criteria.contains(c) || is_visual_criterion(c))
            })
            .map(String::as_str)
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    NoDeliverable,
    /// The harness printed nothing for `variant.stall_timeout_secs`.
    Stall,
    /// The loop detector stopped the run (`variant.loop_detect`).
    Loop,
    /// The attempt's estimated cost passed `variant.max_attempt_cost_usd`.
    Budget,
    Verify,
    /// Landing carried the work onto a moved base and files conflict: the
    /// next attempt only resolves them.
    Conflict,
    /// The held-out check (`variant.grounded_checks`) failed after verify.
    Heldout,
    Review,
    /// A visual criterion, and the attempt saved no image under `artifacts/`.
    Evidence,
    Protected,
    Blocked,
    Error,
}

impl FailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureKind::NoDeliverable => "no_deliverable",
            FailureKind::Stall => "stall",
            FailureKind::Loop => "loop",
            FailureKind::Budget => "budget",
            FailureKind::Verify => "verify",
            FailureKind::Conflict => "conflict",
            FailureKind::Heldout => "heldout",
            FailureKind::Review => "review",
            FailureKind::Evidence => "evidence",
            FailureKind::Protected => "protected",
            FailureKind::Blocked => "blocked",
            FailureKind::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub kind: FailureKind,
    pub detail: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cached: u64,
}

/// What a run actually used, as the harness reported it: the alias in
/// `Attempt.model` is only what orchd asked for. `prompt_hash` covers the
/// inputs orchd controls (template version, argv and run settings minus
/// paths, ids and secrets), never the task-specific brief text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    #[serde(default)]
    pub models: Vec<String>,
    pub harness: Harness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_version: Option<String>,
    #[serde(default)]
    pub prompt_hash: String,
}

impl Fingerprint {
    /// `model (harness version)`, the form shown in reports and the panel.
    pub fn label(&self) -> String {
        let models = if self.models.is_empty() {
            "?".to_string()
        } else {
            self.models.join("+")
        };
        match &self.harness_version {
            Some(v) => format!("{models} ({v})"),
            None => models,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub n: u32,
    pub stage: Stage,
    pub route_id: String,
    pub harness: Harness,
    pub model: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The spawned child's process group id (== its pid, since it calls
    /// `setsid()`), persisted as soon as it's known so a killed daemon can
    /// `killpg` it back on recovery instead of leaving it orphaned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pgid: Option<i32>,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    pub status: AttemptStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The agent's note for whoever picks the task up next: what is done,
    /// what was tried, what to do next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<String>,
    /// Review findings the agent disputed in its report.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disputes: Vec<Dispute>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub verify: Vec<VerifyOutcome>,
    #[serde(default)]
    pub gate_blocks: u32,
    /// Tokens of the first turn's prompt (input + cache creation + cache
    /// read) of a fresh Claude implement attempt: the fixed prefix plus the
    /// brief. `None` for Codex attempts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// `cost_usd` was priced from the per-message token usage in the run's
    /// `events.jsonl` (`settings.prices`), because the run ended -- stopped,
    /// stalled, or the daemon restarted -- before the CLI reported a cost.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cost_estimated: bool,
    /// Cost of the review run(s) that reviewed this implement attempt. Kept
    /// separate from `cost_usd` on purpose: a review's cost is never part
    /// of the attempt's own. Always added to
    /// `task.cost_usd` too, at the point the review runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_cost_usd: Option<f64>,
    /// Absolute paths of the images this attempt saved, copied to
    /// `runs/<n>/evidence/` so they outlive the worktree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    /// Git tree id of the worktree (tracked + untracked, non-ignored) when
    /// this attempt saved images of its own; what a later attempt is diffed
    /// against to decide whether it may reuse this attempt's evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_tree: Option<String>,
    /// The task's base commit when `evidence_tree` was taken, so a later
    /// attempt can tell the task's own edits from carried-in base commits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_base: Option<String>,
    /// The attempt whose `evidence` this attempt reused because it changed
    /// only `artifacts/` or test files since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_from: Option<u32>,
    /// The advisor's short diagnosis of this attempt's failure, shown to the
    /// next attempt. Its cost is added to the task, not to this attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advice: Option<String>,
    /// Cost of the advisor run about this attempt's failure, already in
    /// `task.cost_usd` -- set the moment the run ends, so recovery can tell
    /// an advisor run the daemon died under (still `None`) from a counted
    /// one. An attempt is never advised twice once this is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisor_cost_usd: Option<f64>,
    /// What the implement or plan run actually used (see [`Fingerprint`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_fingerprint: Option<Fingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisor_fingerprint: Option<Fingerprint>,
    /// `variant.best_of`: the candidates this attempt ran, empty for a
    /// single run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<Candidate>,
    /// Where each task criterion stands after this implement attempt: one
    /// entry per criterion, recomputed after its checks and each review.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria_results: Vec<CriterionResult>,
    /// The worktree against the task's base when the attempt ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_stat: Option<DiffStat>,
}

/// Files changed and lines added and removed; a binary file counts as a
/// file with no lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffStat {
    pub files: u32,
    pub added: u64,
    pub removed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CriterionStatus {
    Met,
    Pending,
    Failing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CriterionResult {
    /// Index into `task.criteria`.
    pub criterion: usize,
    /// The criterion's text when the result was computed.
    pub text: String,
    pub status: CriterionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

/// One of the concurrent runs of a best-of attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub route: String,
    /// What the candidate's own run cost.
    pub cost: f64,
    /// It finished, changed files and every verify command exited 0.
    pub verify: bool,
    /// Grounded checks (and the held-out one) that passed and failed.
    pub checks: CheckTally,
    pub picked: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckTally {
    pub passed: u32,
    pub failed: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The address the orchestrator agent sends from and receives at; every
/// other address in a [`Message`] is a task id.
pub const ORCHESTRATOR: &str = "orchestrator";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageKind {
    Message,
    /// Addressed to the orchestrator: wakes it for a turn.
    Question,
    /// Answers the message named by `reply_to`.
    Reply,
}

/// One agent-to-agent message. The recipient reads it on its next turn --
/// a task's next attempt brief, the orchestrator's next chat turn -- which
/// is when `delivered` flips; nothing interrupts a turn already running.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub repo: String,
    pub from: String,
    pub to: String,
    pub kind: MessageKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub ts: i64,
    #[serde(default)]
    pub delivered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditStatus {
    Running,
    Done,
    /// The run failed or its reply had no parsable report: `error` says
    /// which, and no `report.json` exists.
    Failed,
    /// Cancelled (daemon shutdown) or interrupted by a daemon restart.
    Stopped,
}

/// One read-only `repo.audit` run, kept as `<data>/audits/<id>/audit.json`.
/// Its report lives next to it in `report.json`, only when the reply held
/// a parsable one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Audit {
    pub id: String,
    pub repo: String,
    pub route_id: String,
    pub harness: Harness,
    pub model: String,
    pub status: AuditStatus,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
    #[serde(default)]
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditGrade {
    Good,
    Weak,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditEffort {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditItem {
    pub area: String,
    pub grade: AuditGrade,
    pub evidence: Vec<String>,
    pub recommendation: String,
    pub effort: AuditEffort,
}

/// The agent's ```sushi-audit reply, as `report.json`. Every field is
/// required: `brief::parse_audit` rejects a report missing any of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditReport {
    pub summary: String,
    pub items: Vec<AuditItem>,
    pub top_fixes: Vec<String>,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_have_no_haiku_route_and_check_on_the_mechanical_route() {
        let settings = Settings::default();
        assert!(!settings
            .routes
            .iter()
            .any(|r| r.id.contains("haiku")
                || r.model.as_deref().is_some_and(|m| m.contains("haiku"))));
        assert_eq!(
            Some(&settings.brief_check_route),
            settings.tiers.get(&Tier::Mechanical)
        );
    }

    fn minimal_task_json() -> serde_json::Value {
        serde_json::json!({
            "id": "t", "title": "", "goal": "", "criteria": [], "verify": [], "repo": "",
            "worktree": "", "branch": "", "baseSha": "", "status": "waiting", "tier": "standard",
            "createdAt": 0, "updatedAt": 0, "variant": {}
        })
    }

    #[test]
    fn a_task_json_from_before_question_history_still_loads_and_serializes_without_the_keys() {
        let mut v = minimal_task_json();
        v["question"] = serde_json::json!({"text": "Ok?", "options": ["a"], "kind": "budget"});
        let task: Task = serde_json::from_value(v).unwrap();
        let q = task.question.as_ref().unwrap();
        assert_eq!((q.asked_by, q.asked_at), (None, None));
        assert!(task.question_history.is_empty());
        let out = serde_json::to_value(&task).unwrap();
        assert!(out.get("questionHistory").is_none(), "{out}");
        assert!(out["question"].get("askedBy").is_none(), "{out}");
        assert!(out["question"].get("askedAt").is_none(), "{out}");
    }

    #[test]
    fn a_stamped_question_and_its_history_entry_serialize_lowercase() {
        let q = Question::new(
            "Ok?",
            vec!["a".into()],
            QuestionKind::Budget,
            AskedBy::Review,
        );
        assert!(q.asked_at.is_some());
        let mut task: Task = serde_json::from_value(minimal_task_json()).unwrap();
        task.question_history.push(AnsweredQuestion {
            question: q.text.clone(),
            options: q.options.clone(),
            kind: q.kind,
            asked_by: q.asked_by,
            asked_at: q.asked_at,
            answer: "a".into(),
            answered_at: 5,
            answered_by: AnsweredBy::Orchestrator,
        });
        task.question = Some(q);
        let v = serde_json::to_value(&task).unwrap();
        assert_eq!(v["question"]["askedBy"], "review");
        assert!(v["question"]["askedAt"].is_i64());
        let e = &v["questionHistory"][0];
        assert_eq!(e["askedBy"], "review");
        assert_eq!(e["answeredBy"], "orchestrator");
        assert_eq!(e["answeredAt"], 5);
        assert_eq!(e["kind"], "budget");
        let back: Task = serde_json::from_value(v).unwrap();
        assert_eq!(back.question_history, task.question_history);
    }

    #[test]
    fn settings_without_evolution_load_with_evolution_defaults() {
        let mut v = serde_json::to_value(Settings::default()).unwrap();
        v.as_object_mut().unwrap().remove("evolution");
        let s: Settings = serde_json::from_value(v).unwrap();
        assert_eq!(s.evolution, EvolutionSettings::default());
        assert_eq!(s.evolution.min_tasks, 3);
        assert_eq!(s.evolution.min_wasted_calls, 20);
        assert_eq!(s.evolution.min_wasted_usd, 1.0);
        assert_eq!(s.evolution.max_proposals, 3);
        assert_eq!(s.evolution.proposer_route, "");
        assert_eq!(s.evolution.revert_after_tasks, 10);
        let partial: EvolutionSettings = serde_json::from_str(r#"{"minTasks":5}"#).unwrap();
        assert_eq!(partial.min_tasks, 5);
        assert_eq!(partial.max_proposals, 3);
    }

    #[test]
    fn settings_default_round_trips_through_json() {
        let s = Settings::default();
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["review"], "auto");
        assert_eq!(v["tiers"]["mechanical"], "codex");
        assert_eq!(v["tiers"]["standard"], "claude-sonnet");
        assert_eq!(v["tiers"]["hard"], "claude-opus");
        assert_eq!(v["maxAttempts"], 4);
        assert_eq!(v["parallel"], 2);
        assert_eq!(v["planner"], "claude-opus");
        assert_eq!(v["orchestrator"], "");
        let back: Settings = serde_json::from_value(v).unwrap();
        assert_eq!(back.max_attempts, 4);
        assert_eq!(back.tiers.get(&Tier::Hard).unwrap(), "claude-opus");
    }

    #[test]
    fn settings_default_without_a_planner_field_defaults_to_claude_opus() {
        // An older settings.json on disk (written before this field existed)
        // must still load, with planning on by default and on the hard
        // tier's route - the strongest model plans best.
        let old_json = serde_json::json!({
            "routes": [], "tiers": {}, "review": "",
            "sandbox": "host", "allowedDomains": [], "protectedPaths": [], "maxAttempts": 4, "parallel": 2,
        });
        let s: Settings = serde_json::from_value(old_json).unwrap();
        assert_eq!(s.planner, "claude-opus");
        assert_eq!(s.orchestrator, "");
        assert!(!s.auto_answer);
    }

    #[test]
    fn task_request_field_is_camel_case_and_omitted_when_absent() {
        let task = Task {
            id: "t1".into(),
            title: "Fix the thing".into(),
            goal: String::new(),
            criteria: vec![],
            verify: vec![],
            final_verify: vec![],
            checks: vec![],
            held_out: None,
            request: Some("fix the thing that's broken".into()),
            repo: "/repo".into(),
            worktree: "/repo-task".into(),
            worktree_removed: false,
            visual_criteria: vec![],
            landed_sha: None,
            landed_at: None,
            diff_stat: None,
            report: None,
            report_at: None,
            lead_touch: None,
            follow_up_of: None,
            follow_ups: vec![],
            branch: "task/x".into(),
            base_sha: "abc".into(),
            base_ref: None,
            depends_on: vec![],
            paths: vec![],
            parent: None,
            status: TaskStatus::Drafting,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            budget_raises: 0,
            daily_budget_ok_day: None,
            assumptions: vec![],
            question_history: vec![],
            judged_findings: vec![],
            archived: false,
            planned_tier: None,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            source: None,
            eval_check: None,
            brief_check: Default::default(),
            queue: Default::default(),
            created_at: 1,
            updated_at: 1,
        };
        let v = serde_json::to_value(&task).unwrap();
        assert_eq!(v["request"], "fix the thing that's broken");
        assert_eq!(v["status"], "drafting");

        let mut without_request = task;
        without_request.request = None;
        let v2 = serde_json::to_value(&without_request).unwrap();
        assert!(v2.get("request").is_none());
    }

    #[test]
    fn a_variant_written_with_retired_flags_still_loads_and_drops_them() {
        let old: Variant = serde_json::from_value(serde_json::json!({
            "retryMode": "resume", "leanOutput": true, "reviewBlind": true, "advisor": true
        }))
        .unwrap();
        assert!(old.advisor);
        let json = serde_json::to_string(&old).unwrap();
        for key in Variant::RETIRED_KEYS {
            assert!(!json.contains(key), "{json}");
        }
    }

    #[test]
    fn settings_written_with_retired_flags_still_load() {
        let retired = serde_json::json!({
            "retryMode": "fresh", "plannerTier": true, "contract": true,
            "reviewOtherFamily": true, "deferHeavyChecks": true,
            "leanOutput": true, "reviewBlind": true, "advisor": true
        });
        let mut settings = serde_json::to_value(Settings::default()).unwrap();
        settings["experiments"] = retired.clone();
        let s: Settings = serde_json::from_value(settings).unwrap();
        assert!(s.experiments.advisor && s.experiments.loop_detect);
    }

    #[test]
    fn old_settings_without_loop_detect_keep_it_on() {
        let parse = |experiments: Option<&str>| -> Settings {
            let mut v = serde_json::to_value(Settings::default()).unwrap();
            let o = v.as_object_mut().unwrap();
            match experiments {
                Some(e) => o.insert("experiments".into(), serde_json::from_str(e).unwrap()),
                None => o.remove("experiments"),
            };
            serde_json::from_value(v).unwrap()
        };
        let s = parse(Some(r#"{"advisor":true}"#));
        assert!(s.experiments.loop_detect && s.experiments.advisor);
        assert!(parse(None).experiments.loop_detect);
        assert!(
            !parse(Some(r#"{"loopDetect":false}"#))
                .experiments
                .loop_detect
        );
    }

    #[test]
    fn a_variant_without_route_overrides_serializes_as_before() {
        assert_eq!(
            serde_json::to_string(&Variant::default()).unwrap(),
            r#"{"stallTimeoutSecs":0,"reviewEvidence":false,"advisor":false,"loopDetect":false,"land":false}"#
        );
    }

    #[test]
    fn route_overrides_serialize_in_one_key_order_and_round_trip() {
        let mut a = Variant::default();
        a.tier_routes.insert(Tier::Hard, "claude-sonnet".into());
        a.tier_routes.insert(Tier::Mechanical, "codex".into());
        a.planner_route = Some("claude-sonnet".into());
        let mut b = Variant {
            planner_route: Some("claude-sonnet".into()),
            ..Variant::default()
        };
        b.tier_routes.insert(Tier::Mechanical, "codex".into());
        b.tier_routes.insert(Tier::Hard, "claude-sonnet".into());
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, serde_json::to_string(&b).unwrap());
        assert!(
            json.ends_with(r#""plannerRoute":"claude-sonnet","tierRoutes":{"mechanical":"codex","hard":"claude-sonnet"},"land":false}"#),
            "{json}"
        );
        let back: Variant = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn route_overrides_replace_the_settings_routes_and_escalation_follows_them() {
        let settings = Settings::default();
        let mut v = Variant::default();
        assert_eq!(
            v.implement_route_id(&settings, Tier::Hard),
            ("claude-opus".to_string(), false)
        );
        assert_eq!(v.plan_route_id(&settings), "claude-opus");
        v.tier_routes.insert(Tier::Hard, "claude-sonnet".into());
        v.planner_route = Some("claude-sonnet".into());
        // A standard-tier task that keeps failing moves up to hard, and
        // takes the overridden hard route.
        assert_eq!(
            v.implement_route_id(&settings, Tier::Standard.up()),
            ("claude-sonnet".to_string(), true)
        );
        assert_eq!(
            v.implement_route_id(&settings, Tier::Mechanical),
            ("codex".to_string(), false)
        );
        assert_eq!(v.plan_route_id(&settings), "claude-sonnet");
        assert!(v.check_routes(&settings.routes).is_ok());
        v.tier_routes.insert(Tier::Standard, "nope".into());
        let err = v.check_routes(&settings.routes).unwrap_err();
        assert!(err.contains("tierRoutes.standard \"nope\""), "{err}");
        v.tier_routes.clear();
        v.planner_route = Some("gone".into());
        let err = v.check_routes(&settings.routes).unwrap_err();
        assert!(err.contains("plannerRoute \"gone\""), "{err}");
    }

    #[test]
    fn an_attempt_without_a_fingerprint_loads_and_round_trips_without_one() {
        let a: Attempt = serde_json::from_value(serde_json::json!({
            "n": 1, "stage": "implement", "routeId": "r", "harness": "claude", "model": "sonnet",
            "reason": "x", "resumed": false, "startedAt": 0, "status": "passed"
        }))
        .unwrap();
        assert!(a.fingerprint.is_none());
        assert!(serde_json::to_value(&a)
            .unwrap()
            .get("fingerprint")
            .is_none());
    }

    #[test]
    fn attempt_prefix_tokens_is_optional_on_disk_and_a_removed_skills_field_still_loads() {
        let old = serde_json::json!({
            "n": 1, "stage": "implement", "routeId": "r", "harness": "claude", "model": "m",
            "reason": "x", "resumed": false, "startedAt": 0, "status": "passed",
            "skills": ["deslop"]
        });
        let mut a: Attempt = serde_json::from_value(old).unwrap();
        assert_eq!(a.prefix_tokens, None);
        let v = serde_json::to_value(&a).unwrap();
        assert!(v.get("prefixTokens").is_none() && v.get("skills").is_none());
        a.prefix_tokens = Some(19_629);
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["prefixTokens"], 19_629);
    }

    #[test]
    fn max_cost_usd_is_off_by_default_and_each_raise_adds_it_once_more() {
        let v: Variant = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(v.max_cost_usd, 0.0);
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": "t", "title": "", "goal": "", "criteria": [], "verify": [], "repo": "",
            "worktree": "", "branch": "", "baseSha": "", "status": "queued", "tier": "standard",
            "createdAt": 0, "updatedAt": 0, "variant": {}
        }))
        .unwrap();
        assert_eq!(task.cost_budget(), None);
        task.variant = Some(Variant {
            max_cost_usd: 2.5,
            ..Variant::default()
        });
        assert_eq!(task.cost_budget(), Some(2.5));
        task.budget_raises = 2;
        assert_eq!(task.cost_budget(), Some(7.5));
        assert_eq!(serde_json::to_value(&task).unwrap()["budgetRaises"], 2);
        assert_eq!(
            serde_json::to_value(task.variant()).unwrap()["maxCostUsd"],
            2.5
        );
        let negative = Variant {
            max_cost_usd: -1.0,
            ..Variant::default()
        };
        assert!(negative.check().is_err());
    }

    #[test]
    fn tier_up_caps_at_hard() {
        assert_eq!(Tier::Mechanical.up(), Tier::Standard);
        assert_eq!(Tier::Standard.up(), Tier::Hard);
        assert_eq!(Tier::Hard.up(), Tier::Hard);
    }

    #[test]
    fn task_field_names_are_camel_case() {
        let task = Task {
            id: "t1".into(),
            title: "Title".into(),
            goal: "Goal".into(),
            criteria: vec![],
            verify: vec![],
            final_verify: vec![],
            checks: vec![],
            held_out: None,
            request: None,
            repo: "/repo".into(),
            worktree: "/repo-task".into(),
            worktree_removed: false,
            visual_criteria: vec![],
            landed_sha: None,
            landed_at: None,
            diff_stat: None,
            report: None,
            report_at: None,
            lead_touch: None,
            follow_up_of: None,
            follow_ups: vec![],
            branch: "task/x".into(),
            base_sha: "abc".into(),
            base_ref: None,
            depends_on: vec![],
            paths: vec![],
            parent: None,
            status: TaskStatus::Queued,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            budget_raises: 0,
            daily_budget_ok_day: None,
            assumptions: vec![],
            question_history: vec![],
            judged_findings: vec![],
            archived: false,
            planned_tier: None,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            source: None,
            eval_check: None,
            brief_check: Default::default(),
            queue: Default::default(),
            created_at: 1,
            updated_at: 1,
        };
        let v = serde_json::to_value(&task).unwrap();
        assert_eq!(v["baseSha"], "abc");
        assert_eq!(v["createdAt"], 1);
        assert!(v.get("base_sha").is_none());
    }

    fn task_with_criteria(criteria: &[&str], flagged: &[&str]) -> Task {
        Task {
            id: "t1".into(),
            title: "Title".into(),
            goal: "Goal".into(),
            criteria: criteria.iter().map(|c| c.to_string()).collect(),
            verify: vec![],
            final_verify: vec![],
            checks: vec![],
            held_out: None,
            request: None,
            repo: "/repo".into(),
            worktree: "/repo-task".into(),
            worktree_removed: false,
            visual_criteria: flagged.iter().map(|c| c.to_string()).collect(),
            landed_sha: None,
            landed_at: None,
            diff_stat: None,
            report: None,
            report_at: None,
            lead_touch: None,
            follow_up_of: None,
            follow_ups: vec![],
            branch: "task/x".into(),
            base_sha: "abc".into(),
            base_ref: None,
            depends_on: vec![],
            paths: vec![],
            parent: None,
            status: TaskStatus::Done,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            budget_raises: 0,
            daily_budget_ok_day: None,
            assumptions: vec![],
            question_history: vec![],
            archived: false,
            planned_tier: None,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            eval_check: None,
            source: None,
            judged_findings: vec![],
            brief_check: Default::default(),
            queue: Default::default(),
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn a_criterion_checked_by_a_command_is_never_visual() {
        for check in [
            "x -- check: cargo test --bin orchd",
            "x -- check: npm run test:orchd",
            "x -- check: node --test tests/a.cjs",
            "x -- check: pytest tests/",
        ] {
            let task = task_with_criteria(&[check], &[check]);
            assert!(is_command_check(check), "{check}");
            assert!(task.visual_criteria_texts().is_empty(), "{check}");
        }
    }

    #[test]
    fn a_check_that_mentions_an_image_is_still_visual() {
        for check in [
            "x -- check: screenshot under artifacts/",
            "x -- check: npm run screenshot -- artifacts/x.png",
        ] {
            let task = task_with_criteria(&[check], &[check]);
            assert!(!is_command_check(check), "{check}");
            assert_eq!(task.visual_criteria_texts(), vec![check], "{check}");
        }
        assert!(!is_command_check("cargo test"));
        assert!(is_command_check("x -- check: go test ./..."));
        assert!(!is_command_check("x -- check: read the makefile"));
    }

    #[test]
    fn task_archived_field_is_camel_case_and_defaults_to_false_when_absent() {
        let mut task = Task {
            id: "t1".into(),
            title: "Title".into(),
            goal: "Goal".into(),
            criteria: vec![],
            verify: vec![],
            final_verify: vec![],
            checks: vec![],
            held_out: None,
            request: None,
            repo: "/repo".into(),
            worktree: "/repo-task".into(),
            worktree_removed: false,
            visual_criteria: vec![],
            landed_sha: None,
            landed_at: None,
            diff_stat: None,
            report: None,
            report_at: None,
            lead_touch: None,
            follow_up_of: None,
            follow_ups: vec![],
            branch: "task/x".into(),
            base_sha: "abc".into(),
            base_ref: None,
            depends_on: vec![],
            paths: vec![],
            parent: None,
            status: TaskStatus::Done,
            tier: Tier::Standard,
            question: None,
            decisions: vec![],
            attempts: vec![],
            cost_usd: 0.0,
            budget_raises: 0,
            daily_budget_ok_day: None,
            assumptions: vec![],
            question_history: vec![],
            judged_findings: vec![],
            archived: true,
            planned_tier: None,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            source: None,
            eval_check: None,
            brief_check: Default::default(),
            queue: Default::default(),
            created_at: 1,
            updated_at: 1,
        };
        let v = serde_json::to_value(&task).unwrap();
        assert_eq!(v["archived"], true);

        // A pre-existing task.json written before this field existed must
        // still load, defaulting to `archived: false`.
        let mut without_archived = v.clone();
        without_archived.as_object_mut().unwrap().remove("archived");
        let loaded: Task = serde_json::from_value(without_archived).unwrap();
        assert!(!loaded.archived);

        task.archived = false;
        let v2 = serde_json::to_value(&task).unwrap();
        assert_eq!(v2["archived"], false);
    }
}

#[cfg(test)]
mod grounded_checks_tests {
    use super::*;

    fn check(criterion: usize, run: &str) -> Check {
        Check {
            criterion,
            run: run.into(),
            baseline: None,
        }
    }

    #[test]
    fn grounded_checks_is_left_out_when_off_and_named_when_on() {
        assert!(!serde_json::to_string(&Variant::default())
            .unwrap()
            .contains("groundedChecks"));
        let on = Variant {
            grounded_checks: true,
            ..Variant::default()
        };
        assert!(serde_json::to_string(&on)
            .unwrap()
            .contains(r#""groundedChecks":true"#));
        assert!(Variant::OPTIONAL_KEYS.contains(&"groundedChecks"));
    }

    #[test]
    fn valid_checks_drop_out_of_range_and_blank_entries() {
        let kept = valid_checks(
            vec![
                check(0, "a"),
                check(2, "out of range"),
                check(1, "  \t"),
                check(1, " b "),
            ],
            2,
        );
        assert_eq!(kept, vec![check(0, "a"), check(1, " b ")]);
        assert_eq!(valid_check(Some(check(3, "x")), 3), None);
        assert_eq!(valid_check(Some(check(2, "x")), 3), Some(check(2, "x")));
        assert_eq!(valid_check(None, 3), None);
    }

    #[test]
    fn valid_checks_reset_a_supplied_baseline() {
        let mut c = check(0, "a");
        c.baseline = Some(Baseline::Fail);
        assert_eq!(valid_checks(vec![c.clone()], 1), vec![check(0, "a")]);
        assert_eq!(valid_check(Some(c), 1), Some(check(0, "a")));
    }

    #[test]
    fn the_default_scoped_check_gates_the_desktop_smoke_on_ui_paths() {
        let expected = ScopedCheck {
            repo: None,
            command: "npm run test:desktop".into(),
            paths: [
                "src/app/**",
                "src/extensions/**",
                "electron/**",
                "src/styles/**",
                "*.html",
            ]
            .map(String::from)
            .to_vec(),
        };
        assert_eq!(Settings::default().scoped_checks, vec![expected.clone()]);
        // A settings.json written before the key existed picks it up.
        let mut json = serde_json::to_value(Settings::default()).unwrap();
        json.as_object_mut().unwrap().remove("scopedChecks");
        let loaded: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.scoped_checks, vec![expected]);
    }

    #[test]
    fn a_check_serializes_its_baseline_only_once_known() {
        assert_eq!(
            serde_json::to_string(&check(0, "x")).unwrap(),
            r#"{"criterion":0,"run":"x"}"#
        );
        let with = Check {
            baseline: Some(Baseline::Env),
            ..check(1, "y")
        };
        assert_eq!(
            serde_json::to_string(&with).unwrap(),
            r#"{"criterion":1,"run":"y","baseline":"env"}"#
        );
        assert_eq!(
            serde_json::to_string(&FailureKind::Heldout).unwrap(),
            r#""heldout""#
        );
        assert_eq!(FailureKind::Heldout.as_str(), "heldout");
    }
}
