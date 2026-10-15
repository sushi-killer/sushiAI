use serde::Deserialize;

/// The arguments of `task.pr`. The owner may name a branch; an agent names
/// none, and the pull request is always the task's own branch.
#[derive(Deserialize)]
pub(crate) struct PrParams {
    pub id: String,
    pub title: Option<String>,
    pub branch: Option<String>,
    /// Replaces the generated body when not empty.
    pub body: Option<String>,
    /// Set by the MCP bridge; the app sends none.
    #[serde(default)]
    pub source: Option<String>,
}
