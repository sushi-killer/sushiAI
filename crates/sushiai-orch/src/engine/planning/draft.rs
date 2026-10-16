use crate::engine::planning::serde::{
    lenient_check, lenient_checks, lenient_criteria, lenient_tests, lenient_tier,
};
use crate::policy::routing::Tier;
use crate::prompts::reply::{last_fenced_block, lenient_text};
use crate::task::checks::types::Check;
use crate::task::questions::Risk;

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PlanQuestion {
    pub text: String,
    #[serde(default)]
    pub options: Vec<String>,
    /// `variant.batch_questions`: the option to take when nobody answers.
    #[serde(default)]
    pub recommended: Option<String>,
    #[serde(default)]
    pub evidence: String,
    /// Missing means blocking: only a planner that says otherwise skips the
    /// owner.
    #[serde(default = "default_blocking")]
    pub blocking: bool,
    /// What the question commits the owner to (`Question.risk`).
    #[serde(default)]
    pub risk: Vec<Risk>,
    /// The index in `options` taken when nobody answers (`Question.default_option`).
    #[serde(default, rename = "defaultOption")]
    pub default_option: Option<usize>,
}

fn default_blocking() -> bool {
    true
}

impl PlanQuestion {
    /// The recommendation, when it can stand in for an answer: non-empty
    /// and, if the question lists options, one of them.
    pub fn usable_recommendation(&self) -> Option<&str> {
        let r = self.recommended.as_deref()?.trim();
        (!r.is_empty() && (self.options.is_empty() || self.options.iter().any(|o| o == r)))
            .then_some(r)
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PlanDraft {
    pub title: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default, deserialize_with = "lenient_criteria")]
    pub criteria: Vec<String>,
    /// Criteria the planner wrote as `{text, visual: true}`; filled by
    /// [`parse_plan`].
    #[serde(skip)]
    pub visual_criteria: Vec<String>,
    #[serde(default)]
    pub verify: Vec<String>,
    #[serde(default)]
    pub questions: Vec<PlanQuestion>,
    #[serde(default, rename = "finalVerify")]
    pub final_verify: Vec<String>,
    /// Unknown or missing -> `None`: the task runs on the standard route.
    #[serde(default, deserialize_with = "lenient_tier")]
    pub tier: Option<Tier>,
    /// The planner's split of a request too large for one session; empty
    /// for an ordinary plan.
    #[serde(default)]
    pub subtasks: Vec<PlanSubtask>,
    /// Unusable entries are dropped while parsing, out-of-range ones by `valid_checks` when the plan is stored.
    #[serde(default, deserialize_with = "lenient_checks")]
    pub checks: Vec<Check>,
    #[serde(default, rename = "heldOut", deserialize_with = "lenient_check")]
    pub held_out: Option<Check>,
    /// Failing test files the planner wants committed before attempt 1;
    /// unusable entries are dropped while parsing, unsafe or oversized ones
    /// when the plan is stored.
    #[serde(default, deserialize_with = "lenient_tests")]
    pub tests: Vec<crate::task::checks::types::PlannedTest>,
    /// Repo-relative files or directories the whole task edits; a task whose
    /// paths overlap another live task's waits for it.
    #[serde(default)]
    pub paths: Vec<String>,
    /// The repo command that captures the screenshot evidence, when a screen
    /// changes.
    #[serde(default)]
    pub screenshot: Option<String>,
    /// Which criteria prove each item of the request; unusable entries are
    /// dropped while parsing.
    #[serde(default, deserialize_with = "lenient_coverage")]
    pub coverage: Vec<Coverage>,
}

/// One request item and the criteria (0-based) that prove it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Coverage {
    pub requirement: String,
    #[serde(default)]
    pub criteria: Vec<usize>,
}

fn lenient_coverage<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Coverage>, D::Error> {
    let v: Option<Vec<serde_json::Value>> = serde::Deserialize::deserialize(d).unwrap_or(None);
    Ok(v.unwrap_or_default()
        .into_iter()
        .filter_map(|c| {
            let requirement = c.get("requirement")?.as_str()?.trim().to_string();
            let criteria = c
                .get("criteria")
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|n| n.as_u64())
                        .map(|n| n as usize)
                        .collect()
                })
                .unwrap_or_default();
            (!requirement.is_empty()).then_some(Coverage {
                requirement,
                criteria,
            })
        })
        .collect())
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PlanSubtask {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub request: String,
    /// What the part achieves; with `criteria` and `verify` it makes a
    /// complete draft the child runs from without its own planner.
    #[serde(default)]
    pub goal: String,
    #[serde(default, deserialize_with = "lenient_criteria")]
    pub criteria: Vec<String>,
    /// Criteria written as `{text, visual: true}`; filled by [`parse_plan`].
    #[serde(skip)]
    pub visual_criteria: Vec<String>,
    #[serde(default)]
    pub verify: Vec<String>,
    #[serde(default, rename = "finalVerify")]
    pub final_verify: Vec<String>,
    /// Unknown or missing -> `None`.
    #[serde(default, deserialize_with = "lenient_tier")]
    pub tier: Option<Tier>,
    /// Keys of the parts this one builds on.
    #[serde(default, rename = "dependsOn")]
    pub depends_on: Vec<String>,
    /// Repo-relative files or directories this part edits.
    #[serde(default)]
    pub paths: Vec<String>,
    /// The targeted command that proves this part alone.
    #[serde(default, deserialize_with = "lenient_text")]
    pub check: String,
    /// Indices count this part's own `criteria`.
    #[serde(default, deserialize_with = "lenient_checks")]
    pub checks: Vec<Check>,
    #[serde(default, rename = "heldOut", deserialize_with = "lenient_check")]
    pub held_out: Option<Check>,
    /// The repo command that captures this part's screenshot evidence.
    #[serde(default)]
    pub screenshot: Option<String>,
    #[serde(default, deserialize_with = "lenient_tests")]
    pub tests: Vec<crate::task::checks::types::PlannedTest>,
}

impl PlanSubtask {
    /// A parent supplied every field needed to implement without replanning.
    pub fn is_complete(&self) -> bool {
        !self.title.trim().is_empty()
            && !self.goal.trim().is_empty()
            && self.criteria.iter().any(|c| !c.trim().is_empty())
            && self.verify.iter().any(|c| !c.trim().is_empty())
    }

    /// The targeted proof of one part, including a complete parent draft.
    pub fn check_command(&self) -> &str {
        if !self.check.trim().is_empty() {
            self.check.trim()
        } else {
            self.verify
                .iter()
                .find(|c| !c.trim().is_empty())
                .map(|c| c.trim())
                .unwrap_or("")
        }
    }
}

/// Same last-fence-wins, malformed-is-`None` rule as [`parse_report`]/
/// [`parse_review`].
pub fn parse_plan(text: &str) -> Option<PlanDraft> {
    let body = last_fenced_block(text, "sushi-plan")?;
    let mut draft: PlanDraft = serde_json::from_str(&body).ok()?;
    let value: Option<serde_json::Value> = serde_json::from_str(&body).ok();
    let visual_of = |v: &serde_json::Value| -> Vec<String> {
        v.get("criteria")
            .and_then(|c| c.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter(|c| c.get("visual").and_then(|v| v.as_bool()) == Some(true))
                    .filter_map(|c| c.get("text")?.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    if let Some(v) = &value {
        draft.visual_criteria = visual_of(v);
        if let Some(parts) = v.get("subtasks").and_then(|s| s.as_array()) {
            for (part, raw) in draft.subtasks.iter_mut().zip(parts) {
                part.visual_criteria = visual_of(raw);
            }
        }
    }
    Some(draft)
}

#[cfg(test)]
#[path = "tests/draft.rs"]
mod tests;
