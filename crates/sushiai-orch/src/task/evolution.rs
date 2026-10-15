use super::attempt::Fingerprint;
use serde::{Deserialize, Serialize};

/// Which side of the line a proposed change lands on: orchd's own
/// harness/settings, or the audited repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    Harness,
    Repo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProposalForm {
    Script,
    Test,
    Lint,
    Doc,
    Command,
    Skill,
    Prompt,
    Gate,
    Routing,
    Default,
}

/// What a proposer run hands back in its ```sushi-proposal block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProposalReply {
    pub track: Track,
    pub form: ProposalForm,
    pub change: String,
    pub evidence: String,
    pub metric: String,
    pub test: String,
    /// The experiment flags of the harness-track change, as an `orchd eval`
    /// arm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Proposed,
    Rejected,
    Approved,
    Adopted,
    RevertSuggested,
}

/// Finished tasks of a repo on one side of an adoption, and the signals of
/// the proposal's cluster those tasks left.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalCounts {
    pub tasks: u32,
    pub signals: u32,
}

/// One evolution proposal, stored at `<data>/evolution/proposals/<id>.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    pub cluster_key: String,
    pub kind: String,
    pub repo: String,
    pub track: Track,
    pub form: ProposalForm,
    pub change: String,
    pub evidence: String,
    pub metric: String,
    pub test: String,
    pub status: ProposalStatus,
    pub created_at: i64,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopted_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ProposalCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ProposalCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub mod signals;
