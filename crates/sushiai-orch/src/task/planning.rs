use serde::{Deserialize, Serialize};

/// A criterion the task's tools cannot meet: `tool_id` names the connected
/// tool that would, when one is configured but off for this task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingTool {
    pub criterion: usize,
    pub capability: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_id: Option<String>,
}

/// State of a task's brief consistency check (a cheap read-only model run
/// asking whether the goal and criteria contradict each other).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefCheck {
    /// The check has run (or been skipped for good); it never runs twice.
    #[serde(default)]
    pub done: bool,
    /// The conflict the implement and review briefs must name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict: Option<String>,
    /// A criterion that needs a capability the task will not have; the owner
    /// is asked about it once, before the first attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_tool: Option<MissingTool>,
    /// The owner answered the `missing_tool` question.
    #[serde(default)]
    pub missing_tool_asked: bool,
    /// The command the planner named that captures the screenshot evidence;
    /// orchd runs it itself once verify has passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
    /// The one targeted command the planner named for this subtask's part. A
    /// subtask at its parent runs it in place of the full CI entry, which the
    /// parent runs on the combined tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// Every check command was run on the base once (see `base_check`).
    #[serde(default)]
    pub feasibility_done: bool,
    /// The refresh of a parent-drafted child after its dependencies landed
    /// has run (or been skipped); it never runs twice.
    #[serde(default)]
    pub refreshed: bool,
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

/// How serious a plan finding is; only `Critical` triggers the replan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Critical => "CRITICAL",
            Severity::High => "HIGH",
            Severity::Medium => "MEDIUM",
            Severity::Low => "LOW",
        }
    }
}

/// One problem the plan gate found in a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanFinding {
    pub id: String,
    pub severity: Severity,
    pub kind: String,
    pub text: String,
}

/// State of a task's plan gate (`engine/plan_gate.rs`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanGate {
    /// The planner already ran again because of a critical finding.
    #[serde(default)]
    pub replanned: bool,
    /// The plan review has run once; it never runs again for this task.
    #[serde(default)]
    pub reviewed: bool,
    /// Findings left after the gate; the implementer's brief lists them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<PlanFinding>,
}

impl PlanGate {
    pub fn is_default(&self) -> bool {
        *self == PlanGate::default()
    }
}

/// The phases `TaskMetrics.cost_by_phase` always lists, in display order.
pub const COST_PHASES: [&str; 8] = [
    "plan",
    "implement",
    "review",
    "triage",
    "advisor",
    "finding_judge",
    "answer_judge",
    "pick",
];

/// Where a failed task went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCause {
    Env,
    Plan,
    Impl,
    ReviewLoop,
}

impl FailureCause {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureCause::Env => "env",
            FailureCause::Plan => "plan",
            FailureCause::Impl => "impl",
            FailureCause::ReviewLoop => "review_loop",
        }
    }
}

/// First-pass facts of a finished task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskMetrics {
    pub first_pass: bool,
    pub owner_touches: u32,
    /// USD per phase of `COST_PHASES`; 0 when a phase had no spend.
    pub cost_by_phase: std::collections::BTreeMap<String, f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_cause: Option<FailureCause>,
    pub ended_at: i64,
}

/// One part of a merged task: its command runs after every attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistItem {
    pub text: String,
    #[serde(default)]
    pub check: String,
    #[serde(default)]
    pub passes: bool,
}

impl BriefCheck {
    pub fn is_default(&self) -> bool {
        *self == BriefCheck::default()
    }
}

fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}

#[path = "planning/operations.rs"]
pub(crate) mod operations;
