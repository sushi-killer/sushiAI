use crate::policy::routing::Harness;
use crate::task::review::{Decline, ReviewResult, ReviewScope};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyOutcome {
    pub command: String,
    pub code: Option<i32>,
    pub tail: String,
    pub ms: u64,
    #[serde(default)]
    pub load: f64,
    #[serde(default)]
    pub cores: u32,
    /// A high-load timeout occurred, even if its single retry exited non-zero.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub load_timeout: bool,
    /// The failing tests the full output named; empty for a passing
    /// run or an output no known runner format matches. Kept in memory only:
    /// the stored outcome has the counts and `tests_key`, and the lists are in
    /// the `test-lists` file of that key.
    #[serde(skip)]
    pub failing: Vec<String>,
    /// The passing tests the full output named; in memory only, like `failing`.
    #[serde(skip)]
    pub passing: Vec<String>,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub failing_count: usize,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub passing_count: usize,
    /// Names the file with both lists; empty when the run named no test.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tests_key: String,
    /// The output shows a crash, an abort or a build that did not finish.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub crashed: bool,
    /// A cargo run that stopped at its first failing test binary.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped_early: bool,
    /// A digit-blind fingerprint of the output without its test lines.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output_key: String,
    /// Returned to the task owner as well as published while the check waits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    /// The worktree setup failed, so the command never ran. Set by orchd,
    /// never read from the command's output.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub setup_failed: bool,
    /// Checkout HEAD when this command actually ran; the checked tree may include uncommitted work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// Git tree id of the worktree the command ran in. Two failing runs of one
    /// command compare as flaky only when their trees are equal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<String>,
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
    /// The implement run was still going at `settings.implement_budget_mins`.
    TimeBudget,
    Verify,
    /// Landing carried the work onto a moved base and files conflict: the
    /// next attempt only resolves them.
    Conflict,
    /// The held-out check failed after verify.
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
            FailureKind::TimeBudget => "time_budget",
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
    /// Review findings the agent declined in its report, with evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declines: Vec<Decline>,
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
    /// When the run first streamed an edit (`ab::is_edit_event`), in ms.
    /// Recorded live: `events.jsonl` has no timestamps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_edit_at: Option<i64>,
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
    /// Git tree id of the worktree the last review run of this attempt that
    /// gave a verdict saw; the next attempt's review is narrowed to what
    /// changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_tree: Option<String>,
    /// The task's base commit when `review_tree` was taken, so a later review
    /// can leave out files only the base moved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_base: Option<String>,
    /// Whether that review saw the full diff or only the narrow delta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_scope: Option<ReviewScope>,
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
    /// Whose failure the attribution turn found when this attempt's final
    /// check was red on the base too; `None` when no turn ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<Blame>,
    /// The worktree against the task's base when the attempt ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_stat: Option<DiffStat>,
}

/// Whose failure a red check is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Blame {
    Introduced,
    Inherited,
    Environment,
    Incomplete,
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

fn is_zero_usize(n: &usize) -> bool {
    *n == 0
}
