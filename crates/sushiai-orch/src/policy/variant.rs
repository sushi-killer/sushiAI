use super::routing::{Route, Tier};
use super::settings::Settings;
use crate::task::state::Task;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// The experiment flags a task runs with. `Settings::experiments` is the
/// default; `task.create {variant}` overrides it per task, and the task
/// keeps its own copy, so an A/B pair can run side by side and `orchd ab`
/// groups results by it. A flag that wins becomes the plain behaviour and
/// leaves this struct.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Run a supplied hidden check; regular checks always run. On unless
    /// explicitly disabled, including on tasks saved before this switch.
    #[serde(skip_serializing_if = "is_true")]
    pub held_out: bool,
    /// Kill an implement attempt that repeats itself (the same tool call
    /// three times or eight edits of one file with no
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
    /// on; blocking ones go to the overseer and the owner as one question. On
    /// unless explicitly disabled, including on tasks saved before this
    /// switch was on by default.
    #[serde(skip_serializing_if = "is_true")]
    pub batch_questions: bool,
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
    /// Route id of the review stage instead of `settings.review`; `""` turns
    /// review off, `"auto"` picks one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_route: Option<String>,
    /// Route id for the finding judge; `"auto"` selects one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_route: Option<String>,
    /// Route id of the brief check (and the cheap judges) instead of
    /// `settings.briefCheckRoute`; `""` turns them off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brief_check_route: Option<String>,
    /// Attempts the task may run instead of `settings.maxAttempts`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
}

impl Default for Variant {
    fn default() -> Self {
        Self {
            stall_timeout_secs: 0,
            review_evidence: false,
            advisor: false,
            held_out: true,
            loop_detect: false,
            planner_route: None,
            tier_routes: BTreeMap::new(),
            max_cost_usd: 0.0,
            max_attempt_cost_usd: 0.0,
            batch_questions: true,
            best_of: 0,
            best_of_route: None,
            land: false,
            review_route: None,
            judge_route: None,
            brief_check_route: None,
            max_attempts: None,
        }
    }
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

fn is_true(v: &bool) -> bool {
    *v
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
    pub const RETIRED_KEYS: [&'static str; 8] = [
        "retryMode",
        "plannerTier",
        "contract",
        "reviewOtherFamily",
        "deferHeavyChecks",
        "leanOutput",
        "reviewBlind",
        "groundedChecks",
    ];

    /// Keys a serialized default `Variant` leaves out, but a partial
    /// override object may still name.
    pub const OPTIONAL_KEYS: [&'static str; 13] = [
        "heldOut",
        "plannerRoute",
        "tierRoutes",
        "maxCostUsd",
        "maxAttemptCostUsd",
        "batchQuestions",
        "bestOf",
        "bestOfRoute",
        "land",
        "reviewRoute",
        "judgeRoute",
        "briefCheckRoute",
        "maxAttempts",
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
        for (key, id) in [
            ("reviewRoute", &self.review_route),
            ("briefCheckRoute", &self.brief_check_route),
        ] {
            if let Some(id) = id
                .as_deref()
                .filter(|id| !id.is_empty() && *id != "auto" && !known(id))
            {
                return Err(format!("variant {key} \"{id}\" is not a configured route"));
            }
        }
        if let Some(id) = self
            .judge_route
            .as_deref()
            .filter(|id| *id != "auto" && !known(id))
        {
            return Err(format!(
                "variant judgeRoute \"{id}\" is not a configured route"
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

    /// Choose once from the request at entry to a plan stage, before any
    /// clarification answers are appended to it.
    pub fn plan_route_for<'a>(&'a self, settings: &'a Settings, task: &Task) -> (&'a str, String) {
        let planner = self.plan_route_id(settings);
        if settings.small_plan_max_chars == 0 {
            let reason = if self.planner_route.is_some() {
                "variant.plannerRoute"
            } else {
                "small-request check off: smallPlanMaxChars = 0"
            };
            return (planner, format!("Plan route: {planner} ({reason})"));
        }
        if self.planner_route.is_some() {
            return (
                planner,
                format!("Plan route: {planner} (variant.plannerRoute)"),
            );
        }
        if planner.is_empty() {
            return (planner, "Plan route: off (planner is empty)".into());
        }

        let request = task.request.as_deref().unwrap_or_default();
        let chars = request.chars().count();
        let paths = if task.parent.is_some() && !task.paths.is_empty() {
            task.paths.iter().collect::<HashSet<_>>().len()
        } else {
            request
                .split_whitespace()
                .map(|token| {
                    token.trim_matches(['\'', '"', '`', '[', ']', '(', ')', '{', '}', '<', '>'])
                })
                .filter(|token| {
                    !token.contains("://")
                        && (token.contains('/')
                            || token.rsplit_once('.').is_some_and(|(_, ext)| {
                                (1..=8).contains(&ext.len())
                                    && ext.bytes().all(|c| c.is_ascii_alphanumeric())
                            }))
                })
                .collect::<HashSet<_>>()
                .len()
        };
        let list_items = request
            .lines()
            .filter(|line| {
                let line = line.trim_start();
                let rest = line.strip_prefix(['-', '*', '+']).or_else(|| {
                    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
                    (digits > 0)
                        .then(|| line[digits..].strip_prefix(['.', ')']))
                        .flatten()
                });
                rest.is_some_and(|rest| {
                    rest.starts_with(char::is_whitespace) && !rest.trim_start().is_empty()
                })
            })
            .count();
        let mut reasons = Vec::new();
        if chars > settings.small_plan_max_chars {
            reasons.push(format!("{chars} chars > {}", settings.small_plan_max_chars));
        }
        if paths > settings.small_plan_max_paths {
            reasons.push(format!("{paths} paths > {}", settings.small_plan_max_paths));
        }
        if list_items >= 2 {
            reasons.push(format!("{list_items} list items >= 2"));
        }
        if !reasons.is_empty() {
            return (
                planner,
                format!("Plan route: {planner} (not small: {})", reasons.join(", ")),
            );
        }

        let facts = format!(
            "small request: {chars} chars <= {}, {paths} paths <= {}, no list",
            settings.small_plan_max_chars, settings.small_plan_max_paths
        );
        match settings.tiers.get(&Tier::Standard).filter(|id| {
            !id.is_empty() && settings.routes.iter().any(|route| route.id == **id)
        }) {
            Some(id) => (id, format!("Plan route: {id} ({facts})")),
            None => (planner, format!("Plan route: {planner} ({facts}; standard route not configured, fell back to planner)")),
        }
    }

    /// The review stage's route id: this variant's override, else
    /// `settings.review` (`""` = review off, `"auto"` = pick one).
    pub fn review_route_id<'a>(&'a self, settings: &'a Settings) -> &'a str {
        self.review_route.as_deref().unwrap_or(&settings.review)
    }

    /// The brief check's (and the cheap judges') route id: this variant's
    /// override, else `settings.briefCheckRoute` (`""` = off).
    pub fn brief_check_route_id<'a>(&'a self, settings: &'a Settings) -> &'a str {
        self.brief_check_route
            .as_deref()
            .unwrap_or(&settings.brief_check_route)
    }

    /// The attempt budget: this variant's override, else `settings.maxAttempts`.
    pub fn max_attempts(&self, settings: &Settings) -> u32 {
        self.max_attempts.unwrap_or(settings.max_attempts)
    }
}

#[cfg(test)]
#[path = "tests/variant.rs"]
mod tests;
