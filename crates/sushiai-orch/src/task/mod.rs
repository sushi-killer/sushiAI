//! Pure task state and operations; synchronization and persistence belong to the engine.

pub(crate) mod amendment;
pub mod attempt;
pub(crate) mod attempts;
pub mod checks;
pub(crate) mod creation;
pub mod delivery;
pub(crate) mod findings;
pub(crate) mod graph;
pub mod lifecycle;
pub mod planning;
pub mod questions;
pub mod queue;
pub mod review;
pub mod state;
pub(crate) mod transitions;

pub use attempt::{
    Attempt, AttemptStatus, Blame, Candidate, CheckTally, CriterionResult, CriterionStatus,
    DiffStat, EvalCheck, Failure, FailureKind, Fingerprint, Stage, Usage, VerifyOutcome,
};
pub use checks::{
    check_part_command, image_mime, is_command_check, is_visual_criterion, valid_check,
    valid_checks, Baseline, Check, CheckSource, PlannedTest, PlannerTests, SourcedCheck,
    IMAGE_TYPES,
};
pub use delivery::{BranchFrom, Deliverable, FreshBase, LeadTouch, PrPlan, Unlanded};
pub use lifecycle::{
    is_option, BaseAnswer, LoopState, UnreviewedPart, PRE_EXISTING_RETRY, STATE_VERSION,
};
pub use planning::{
    BriefCheck, ChecklistItem, FailureCause, MissingTool, PlanFinding, PlanGate, Severity,
    TaskMetrics, COST_PHASES,
};
pub use questions::{
    AnsweredBy, AnsweredQuestion, AskedBy, Assumption, Question, QuestionKind, Risk, HOLD_TTL_MS,
};
pub use queue::{AnswerSource, Backlog, BacklogBucket, ParkRecord, QueueState, Wait};
pub use review::{
    CriterionRuling, Decline, DeclineReason, DeclineRuling, FindingDetail, FindingRecord,
    FindingScenario, FindingStatus, ReviewResult, ReviewScope, Verdict,
};
pub use state::{Task, TaskData, TaskStatus, TASK_SOURCES};

#[cfg(test)]
mod tests;

pub mod messages;

pub mod audit;

pub mod evolution;

pub mod notes;

pub(crate) mod migrations;

#[cfg(test)]
pub(crate) mod test_support;

pub(crate) mod costs;

pub(crate) mod ladder;

pub(crate) mod loop_operations;

pub(crate) mod suspension;
