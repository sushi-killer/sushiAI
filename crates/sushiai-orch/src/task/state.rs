use crate::policy::routing::Tier;
use crate::policy::variant::Variant;
use crate::task::attempt::{Attempt, DiffStat, EvalCheck};
use crate::task::checks::types::{Check, PlannerTests};
use crate::task::delivery::{Deliverable, FreshBase, LeadTouch, PrPlan, Unlanded};
use crate::task::lifecycle::LoopState;
use crate::task::planning::{BriefCheck, ChecklistItem, PlanGate, TaskMetrics};
use crate::task::questions::{AnsweredQuestion, Assumption, Question};
use crate::task::queue::QueueState;
use crate::task::review::FindingRecord;
use serde::{Deserialize, Serialize};

/// Where a task can start: the orchestrator chat, the panel, an MCP client,
/// the CLI, the planner splitting a parent, an eval run, or another task's
/// handoff.
pub const TASK_SOURCES: [&str; 7] = ["chat", "ui", "mcp", "cli", "planner", "eval", "handoff"];

impl TaskData {
    pub fn variant(&self) -> Variant {
        self.variant.clone().unwrap_or_default()
    }

    /// The dollar budget in force, with the owner's raises; `None` = none.
    pub fn cost_budget(&self) -> Option<f64> {
        let step = self.variant().max_cost_usd;
        (step > 0.0).then(|| step * (1 + self.budget_raises) as f64)
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

/// A live task exposes an immutable state view. Only task operations mutate it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Task {
    pub(in crate::task) data: TaskData,
}

impl std::ops::Deref for Task {
    type Target = TaskData;
    fn deref(&self) -> &TaskData {
        &self.data
    }
}

impl Task {
    pub(crate) fn touch(&mut self, now: i64) {
        self.data.updated_at = now;
    }
    pub(crate) fn record_decision(&mut self, decision: String) {
        self.data.decisions.push(decision);
    }
    pub(crate) fn record_decisions(&mut self, decisions: impl IntoIterator<Item = String>) {
        self.data.decisions.extend(decisions);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskData {
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
    /// (from the planner).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
    /// One extra check the implementer never sees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_out: Option<Check>,
    /// The planner's test files and what orchd did with them.
    #[serde(default, skip_serializing_if = "PlannerTests::is_empty")]
    pub planner_tests: PlannerTests,
    /// The owner's raw one-sentence ask, set only by the `{repo, request}`
    /// form of `task.create`; the drafting stage fills `title`/`goal`/
    /// `criteria`/`verify` from it and leaves this as the original text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    pub repo: String,
    /// Local project whose in-memory environment is available to this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub worktree: String,
    /// The worktree directory was removed because the task no longer needs
    /// it (done, archived, or gc); it is recreated from the branch when the
    /// task runs again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub worktree_removed: bool,
    /// Files the goal, criteria or verify name that were gitignored or
    /// untracked in the worktree, kept under `tasks/<id>/deliverables/`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliverables: Vec<Deliverable>,
    /// The commit a `variant.land` task put on its base branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed_sha: Option<String>,
    /// When the task landed (ms since epoch), set beside `landed_sha`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed_at: Option<i64>,
    /// Work on a done top-level task's branch that is absent from its base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlanded: Option<Unlanded>,
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
    /// Set when the task's base was named (`base`, or the project's default
    /// branch): it is fetched again when the task starts. `None` for a task
    /// that took the checkout's HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_base: Option<FreshBase>,
    /// Ids of tasks that must be `done` before this one starts implementing
    /// (it may still draft its plan meanwhile).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    /// Free-text supplements the owner added with `task.amend`, in order;
    /// the executor brief renders them right after the goal.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addenda: Vec<String>,
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
    /// The parent planner drafted this task completely (title, goal, criteria
    /// and verify), so it skips its own planner.
    #[serde(default, skip_serializing_if = "is_false")]
    pub planned_by_parent: bool,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending_amendment: Option<crate::task::amendment::PendingAmendment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub delivery_checks: Vec<crate::task::VerifyOutcome>,
    /// Where the brief consistency check stands for this task.
    #[serde(default, skip_serializing_if = "BriefCheck::is_default")]
    pub brief_check: BriefCheck,
    /// Where the plan gate stands: the one replan, the one review and the
    /// findings the implementer is told about.
    #[serde(default, skip_serializing_if = "PlanGate::is_default")]
    pub plan_gate: PlanGate,
    /// How the task ended, set once when it reaches `done` or `failed` and
    /// cleared when it starts again (`engine/metrics.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<TaskMetrics>,
    /// The ordered parts of a merged task: orchd runs each item's check
    /// after every attempt, before the full verify.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checklist: Vec<ChecklistItem>,
    /// Where the task stands in orchd's file queue: why it waits. Flat in
    /// the JSON.
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
    /// Every review finding of the task by stable id, with whether the
    /// latest review still reports it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<FindingRecord>,
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
    /// The pull request `task.pr` opened for this task's branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    /// The remote branch that pull request is from; absent: the task's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_branch: Option<String>,
    /// The task is done and its pull request is still to be drafted and
    /// opened; set with `done`, cleared when that flow ends. A restart
    /// resumes it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pr_pending: bool,
    /// The branch, title and body chosen for the pending pull request, saved
    /// before the first push so a restart reuses them instead of drafting
    /// again. Cleared when the flow ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_plan: Option<PrPlan>,
    /// The overseer was already woken when this task finished: a restart,
    /// an unarchive or a rebroadcast never wakes it again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overseer_woken: bool,
    /// An attempt was committed with no review verdict: the work is never
    /// landed by itself, it reaches the owner as a pull request marked so.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unreviewed: bool,
    /// How many times the task went into `failed`; the owner notice is keyed
    /// on it, so a restart that fails again is told once more.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub failed_count: u32,
    /// What the task loop remembers about itself. Decision lines are for the
    /// owner to read; nothing parses them back into state.
    #[serde(flatten)]
    pub loop_state: LoopState,
    pub created_at: i64,
    pub updated_at: i64,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

#[cfg(test)]
impl Task {
    pub(crate) fn fixture_data(&mut self) -> &mut TaskData {
        &mut self.data
    }
}
