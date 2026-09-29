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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClassifierBackend {
    None,
    Openrouter,
    Typesafe,
    Openai,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierSettings {
    pub backend: ClassifierBackend,
    pub model: String,
    pub provider_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub routes: Vec<Route>,
    pub tiers: HashMap<Tier, String>,
    /// Route id; "" = off, "auto" = first configured route whose harness
    /// differs from the implement attempt's.
    pub review: String,
    pub classifier: ClassifierSettings,
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
    /// Route id used for the drafting/plan stage of a `{repo, request}`
    /// `task.create`; `""` turns planning off (that create form is then
    /// rejected -- there is nothing to run it with).
    #[serde(default = "default_planner")]
    pub planner: String,
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
                },
                Route {
                    id: "claude-opus".to_string(),
                    label: "Claude Opus".to_string(),
                    harness: Harness::Claude,
                    model: Some("opus".to_string()),
                    effort: Some("high".to_string()),
                    profile_id: None,
                },
                Route {
                    id: "codex".to_string(),
                    label: "Codex".to_string(),
                    harness: Harness::Codex,
                    model: None,
                    effort: None,
                    profile_id: None,
                },
            ],
            tiers,
            review: "auto".to_string(),
            classifier: ClassifierSettings {
                backend: ClassifierBackend::Openrouter,
                model: "typesafe/jev-1.13".to_string(),
                provider_id: String::new(),
            },
            sandbox: SandboxMode::Native,
            // Open network by default: the sandbox's job is keeping writes
            // inside the worktree, not blocking package registries.
            allowed_domains: vec!["*".to_string()],
            codex_network: false,
            protected_paths: vec![],
            max_attempts: 4,
            parallel: 2,
            planner: default_planner(),
            orchestrator: String::new(),
            auto_answer: false,
            experiments: default_experiments(),
            prices: default_prices(),
            work_buckets: WorkBuckets::default(),
        }
    }
}

/// How a retry after a failed attempt starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetryMode {
    /// Continue the failed attempt's session with only the failure.
    #[default]
    Resume,
    /// A new session with the full brief, the earlier attempts' handoffs
    /// and the last failure.
    Fresh,
}

fn default_experiments() -> Variant {
    Variant {
        loop_detect: true,
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
    pub retry_mode: RetryMode,
    /// Kill an implement attempt whose harness prints nothing for this
    /// long; 0 = off. The clock pauses during orchd's own Stop-hook verify,
    /// but not during an agent's long Bash call (up to 600s), so values
    /// under ~15 min can kill a busy agent.
    pub stall_timeout_secs: u64,
    /// Route by the tier the planner chose; Jev only when there is none.
    pub planner_tier: bool,
    /// The planner checks the request's claims against the code and pairs
    /// every criterion with how to check it; the reviewer rules on each.
    pub contract: bool,
    /// With review `auto` and a route on the other harness configured, the
    /// reviewer runs there (Claude work reviewed by Codex and back).
    pub review_other_family: bool,
    /// The reviewer gets the screenshots the attempt saved (Codex as
    /// attachments, Claude as paths to open) and longer verify output.
    pub review_evidence: bool,
    /// The planner splits slow whole-repo checks into `final_verify`, run
    /// once after review passes instead of on every attempt and stop.
    pub defer_heavy_checks: bool,
    /// A Claude implement run's `settings.json` gets a PreToolUse hook
    /// (`orchd hook rtk`) that offers `rtk rewrite`'s shorter form of a
    /// `Bash` command, and `bashOutputMaxChars` caps how much of a
    /// command's own output comes back. Codex runs and review/plan
    /// sessions are untouched either way.
    pub lean_output: bool,
    /// The review brief leaves out the implementer's summary and decisions,
    /// so the reviewer judges the diff without the author's account.
    pub review_blind: bool,
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

    /// Keys a serialized default `Variant` leaves out, but a partial
    /// override object may still name.
    pub const OPTIONAL_KEYS: [&'static str; 6] = [
        "plannerRoute",
        "tierRoutes",
        "maxCostUsd",
        "maxAttemptCostUsd",
        "batchQuestions",
        "groundedChecks",
    ];

    /// Every route override names a route in `routes`.
    pub fn check_routes(&self, routes: &[Route]) -> Result<(), String> {
        let known = |id: &str| routes.iter().any(|r| r.id == id);
        if let Some(id) = self.planner_route.as_deref().filter(|id| !known(id)) {
            return Err(format!(
                "variant plannerRoute \"{id}\" is not a configured route"
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
    Done,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub text: String,
    pub options: Vec<String>,
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
    pub by: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub goal: String,
    pub criteria: Vec<String>,
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
    /// The tier the planner chose, kept even when `variant.planner_tier` is
    /// off so its choice can be compared with Jev's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_tier: Option<Tier>,
    /// Set to a fixed-set reason (see `classify_tier` in `engine.rs`) when
    /// the implement tier was picked by falling back to `Standard` instead
    /// of a classified or planner choice; cleared as soon as Jev or the
    /// planner picks the tier, or a failure moves the task up a tier.
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
    /// What that check returned; a task counts as a success only when it is
    /// done and this is `code == 0` (or there is no check command).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_check: Option<EvalCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<Question>,
    #[serde(default)]
    pub decisions: Vec<String>,
    /// Non-blocking planner questions answered by their recommendation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assumptions: Vec<Assumption>,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    #[serde(default)]
    pub cost_usd: f64,
    /// How many times the owner raised the `variant.max_cost_usd` budget;
    /// the budget in force is that amount times `1 + budget_raises`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub budget_raises: u32,
    /// Hides the task from the default `task.list` without deleting it.
    /// `#[serde(default)]` so a `task.json` written before this field
    /// existed still loads, with `archived: false`.
    #[serde(default)]
    pub archived: bool,
    pub created_at: i64,
    pub updated_at: i64,
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
    /// The held-out check (`variant.grounded_checks`) failed after verify.
    Heldout,
    Review,
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
            FailureKind::Heldout => "heldout",
            FailureKind::Review => "review",
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
    pub resumed: bool,
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
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub verify: Vec<VerifyOutcome>,
    #[serde(default)]
    pub gate_blocks: u32,
    /// Tokens of the first turn's prompt (input + cache creation + cache
    /// read) of a fresh Claude implement attempt: the fixed prefix plus the
    /// brief. `None` for resumed and Codex attempts.
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
    /// separate from `cost_usd` on purpose: `attempt_cost()` subtracts
    /// earlier attempts' `cost_usd` when a session resumes, and a review's
    /// cost must never be part of that subtraction. Always added to
    /// `task.cost_usd` too, at the point the review runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_cost_usd: Option<f64>,
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
    fn settings_default_round_trips_through_json() {
        let s = Settings::default();
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["review"], "auto");
        assert_eq!(v["tiers"]["mechanical"], "codex");
        assert_eq!(v["tiers"]["standard"], "claude-sonnet");
        assert_eq!(v["tiers"]["hard"], "claude-opus");
        assert_eq!(v["maxAttempts"], 4);
        assert_eq!(v["parallel"], 2);
        assert_eq!(v["classifier"]["backend"], "openrouter");
        assert_eq!(v["classifier"]["model"], "typesafe/jev-1.13");
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
            "routes": [], "tiers": {}, "review": "", "classifier": {"backend": "none", "model": "", "providerId": ""},
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
            assumptions: vec![],
            archived: false,
            planned_tier: None,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            eval_check: None,
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
    fn lean_output_is_camel_case_off_by_default_and_optional_on_disk() {
        assert!(!Variant::default().lean_output);
        let on = Variant {
            lean_output: true,
            ..Variant::default()
        };
        let v = serde_json::to_value(&on).unwrap();
        assert_eq!(v["leanOutput"], true);
        let back: Variant = serde_json::from_value(v).unwrap();
        assert_eq!(back, on);
        // A task.json variant written before the flag existed.
        let old: Variant =
            serde_json::from_value(serde_json::json!({"retryMode": "fresh"})).unwrap();
        assert!(!old.lean_output);
        assert_eq!(old.retry_mode, RetryMode::Fresh);
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
            r#"{"retryMode":"resume","stallTimeoutSecs":0,"plannerTier":false,"contract":false,"reviewOtherFamily":false,"reviewEvidence":false,"deferHeavyChecks":false,"leanOutput":false,"reviewBlind":false,"advisor":false,"loopDetect":false}"#
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
            json.ends_with(r#""plannerRoute":"claude-sonnet","tierRoutes":{"mechanical":"codex","hard":"claude-sonnet"}}"#),
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
            assumptions: vec![],
            archived: false,
            planned_tier: None,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            eval_check: None,
            created_at: 1,
            updated_at: 1,
        };
        let v = serde_json::to_value(&task).unwrap();
        assert_eq!(v["baseSha"], "abc");
        assert_eq!(v["createdAt"], 1);
        assert!(v.get("base_sha").is_none());
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
            assumptions: vec![],
            archived: true,
            planned_tier: None,
            tier_fallback: None,
            variant: Default::default(),
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            eval_check: None,
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
            .ends_with(r#""groundedChecks":true}"#));
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
