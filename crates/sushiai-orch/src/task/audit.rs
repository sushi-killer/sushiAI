use super::attempt::{Fingerprint, Usage};
use crate::policy::routing::Harness;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditStatus {
    Running,
    Done,
    /// The run failed or its reply had no parsable report: `error` says
    /// which, and no `report.json` exists.
    Failed,
    /// Cancelled (daemon shutdown) or interrupted by a daemon restart.
    Stopped,
}

/// One read-only `repo.audit` run, kept as `<data>/audits/<id>/audit.json`.
/// Its report lives next to it in `report.json`, only when the reply held
/// a parsable one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Audit {
    pub id: String,
    pub repo: String,
    pub route_id: String,
    pub harness: Harness,
    pub model: String,
    pub status: AuditStatus,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
    #[serde(default)]
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditGrade {
    Good,
    Weak,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditEffort {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditItem {
    pub area: String,
    pub grade: AuditGrade,
    pub evidence: Vec<String>,
    pub recommendation: String,
    pub effort: AuditEffort,
}

/// The agent's ```sushi-audit reply, as `report.json`. Every field is
/// required: `brief::parse_audit` rejects a report missing any of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditReport {
    pub summary: String,
    pub items: Vec<AuditItem>,
    pub top_fixes: Vec<String>,
}
