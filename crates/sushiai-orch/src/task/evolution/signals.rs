use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Loop,
    Discovery,
    ThrowawayScript,
    VerifySignature,
    ReviewFinding,
    OwnerQuestion,
    ProcessRead,
    Graph,
    RepeatedReview,
    Preexisting,
}

/// Where the evidence for a signal is: a file relative to the data dir and
/// a 1-based inclusive line range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcerptRef {
    pub file: String,
    pub from_line: usize,
    pub to_line: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signal {
    pub kind: SignalKind,
    pub task_id: String,
    pub repo: String,
    /// 0 for a signal about the whole task.
    #[serde(default)]
    pub attempt: u32,
    pub detail: String,
    pub wasted_calls: u32,
    pub wasted_usd: f64,
    pub excerpt_ref: ExcerptRef,
}

/// One recorded occurrence, including its optional-on-disk timestamp.
pub(crate) struct StoredSignal {
    pub(crate) signal: Signal,
    pub(crate) ts: i64,
}
