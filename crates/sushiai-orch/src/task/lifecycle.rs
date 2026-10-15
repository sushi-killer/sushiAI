use serde::{Deserialize, Serialize};

/// The `LoopState::state_version` a task written today carries; a task with a
/// lower one is migrated once, when the store loads it (`store.rs`).
pub const STATE_VERSION: u32 = 3;

/// The base-check answer that waits for the base branch to move.
pub const PRE_EXISTING_RETRY: &str = "retry after the base is fixed";

/// Whether `answer` picks `option`: whitespace, trailing punctuation and
/// case do not matter, so `Drop this check.` is `drop this check`.
pub fn is_option(answer: &str, option: &str) -> bool {
    answer
        .trim()
        .trim_end_matches(['.', ',', '!', '?', ';', ':'])
        .trim_end()
        .eq_ignore_ascii_case(option)
}

fn legacy_state_version() -> u32 {
    0
}

impl Default for LoopState {
    fn default() -> Self {
        LoopState {
            state_version: STATE_VERSION,
            unreviewed_attempt: None,
            agent_restarted: false,
            approach_changed_after: None,
            stuck_retries: 0,
            rereviews: 0,
            non_owner_continues: 0,
            dependency_restarted: false,
            unreviewed_parts: Vec::new(),
            impossible_answers: Vec::new(),
            kept_checks: Vec::new(),
            base_answers: Vec::new(),
            waited_out_bases: Vec::new(),
            answered_permissions: Vec::new(),
            final_check_failures: Vec::new(),
        }
    }
}

/// An answer to the question about a check that fails on the base, kept per
/// command: who gave it and whether it waits for the base to move.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaseAnswer {
    pub command: String,
    /// The base the question was asked on; empty when an old record named none.
    pub sha: String,
    /// `Owner`, `Lead`, `Orchestrator`, `policy` or `judge`; empty when an old
    /// record does not say.
    pub by: String,
    /// The answer was `retry after the base is fixed`.
    pub retry: bool,
}

/// A landed subtask that no review read, as its parent's pull request names it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnreviewedPart {
    pub id: String,
    pub title: String,
}

/// Typed task-loop state, flat in the task JSON. A task file without
/// `stateVersion` predates it; a new task starts at [`STATE_VERSION`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoopState {
    #[serde(default = "legacy_state_version", skip_serializing_if = "is_zero_u32")]
    pub state_version: u32,
    /// The implement attempt committed unreviewed while its review question
    /// was retired and the relaunched loop still has to finish it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unreviewed_attempt: Option<u32>,
    /// An agent already restarted this task after it failed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub agent_restarted: bool,
    /// The attempt after which the failure ladder changed the approach.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approach_changed_after: Option<u32>,
    /// Pauses a stuck task has taken since it was last started.
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub stuck_retries: u32,
    /// Times the landing in progress sent the work back to verify and the
    /// review; reset when the landing completes.
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub rereviews: u32,
    /// Extensions of the attempt budget that a chat, the overseer or a lead
    /// gave, not the owner.
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub non_owner_continues: u32,
    /// A dependency of this task already got its one automatic restart.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dependency_restarted: bool,
    /// Landed subtasks that no review read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unreviewed_parts: Vec<UnreviewedPart>,
    /// The criterion of each impossible question the answer policy answered.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub impossible_answers: Vec<usize>,
    /// Checks that fail on the base and stay in the task anyway; a relaunch
    /// does not ask about them again.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept_checks: Vec<String>,
    /// Answers to base-check questions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub base_answers: Vec<BaseAnswer>,
    /// Bases whose bounded wait for a move already ran out.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waited_out_bases: Vec<String>,
    /// Names of the permission subjects the owner already answered.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub answered_permissions: Vec<String>,
    /// Commands a parent's final check failed on after every subtask landed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub final_check_failures: Vec<String>,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}
