use serde::{Deserialize, Serialize};

/// One owner-approved standing note about a repo, stored in
/// `<data>/repo-notes.json` and shown to the planner. `source` is `owner`
/// or `proposal:<id>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoNote {
    pub id: String,
    pub text: String,
    pub source: String,
    pub created_at: i64,
}
