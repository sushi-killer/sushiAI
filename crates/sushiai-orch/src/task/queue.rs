pub(crate) mod cancellation;
pub(crate) mod operations;
use serde::{Deserialize, Serialize};

/// A task's place in the file queue (`engine/leases.rs`, `engine/graph.rs`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QueueState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub park: Option<ParkRecord>,
    /// When landing first found the base branch's checkout dirty (ms); the
    /// landing gives up and leaves the work on the task branch after an hour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty_since: Option<i64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stopped_explicitly: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub restart_children: bool,
    /// A base carry needs one conflict-only attempt.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub carry_conflicts: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub carry_conflict_attempt: Option<u32>,
    /// The pull request carry conflicted and the task went back for its one
    /// resolution attempt; a second conflict ends the task `failed`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pr_conflict_tried: bool,
    /// What the task waits for right now, if anything: a global slot, a
    /// subtask slot, dependencies, or a file another task has changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<Wait>,
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
    /// A usage or rate limit paused the task until this time (ms); the loop
    /// resumes by itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused_until: Option<i64>,
    /// Consecutive pauses for a usage or rate limit; sets the backoff and
    /// the cap. Only a run that ends without an error resets it.
    #[serde(skip_serializing_if = "is_zero")]
    pub rate_pauses: u32,
    /// Attempts ever interrupted by such a pause: kept out of the hard
    /// attempt ceiling, which counts every other implement attempt.
    #[serde(skip_serializing_if = "is_zero")]
    pub paused_attempts: u32,
    /// When the overseer was last reminded of this task's waiting question
    /// (ms); the next reminder is one interval later.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait_reminded_at: Option<i64>,
}

/// Why a task is not running, as the panel shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Wait {
    /// All global `parallel` slots are taken.
    Slot { running: u32, limit: u32 },
    /// The parent already runs `childParallel` subtasks.
    SubtaskSlot { running: u32, limit: u32 },
    /// Titles of the tasks it depends on that are not done.
    Dependency { titles: Vec<String> },
    /// Another live task has really changed a file this task tried to edit.
    File {
        task_id: String,
        title: String,
        path: String,
    },
}

/// An explicit pause, independent of file-lease waiting or the backlog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParkRecord {
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    pub by: AnswerSource,
}

/// RPC provenance; an omitted source is the owner in the app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnswerSource {
    #[default]
    Owner,
    Lead,
    Chat,
    Mcp,
    Overseer,
    Policy,
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

fn is_zero(n: &u32) -> bool {
    *n == 0
}
