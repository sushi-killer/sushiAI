//! The `routine.json` shape.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Routine {
    pub id: String,
    pub name: String,
    #[serde(rename = "where")]
    pub place: Where,
    pub when: When,
    pub agent: Agent,
    #[serde(default)]
    pub how_it_runs: HowItRuns,
    #[serde(default)]
    pub limits: Limits,
    /// The tools and connectors the routine's runs may use, each granted by the owner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub access: Vec<Access>,
    #[serde(default)]
    pub enabled: bool,
}

impl Routine {
    /// The tools the owner granted: the only ones a run may use besides its own commands.
    pub fn granted_tools(&self) -> Vec<String> {
        self.access
            .iter()
            .filter(|a| a.granted)
            .map(|a| a.tool.clone())
            .collect()
    }

    /// The access entries still waiting for the owner.
    pub fn ungranted(&self) -> Vec<&Access> {
        self.access.iter().filter(|a| !a.granted).collect()
    }
}

/// One tool the routine proposes to use. `granted` is the owner's, never the builder's: a
/// save resets it for a new or changed entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Access {
    /// An exact tool name, `Bash(<command prefix>:*)` or `mcp__<server>__<tool>`.
    pub tool: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// `read` or `write`.
    pub kind: String,
    #[serde(default)]
    pub granted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Where {
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct When {
    pub cron: String,
    pub tz: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    /// `claude` or `codex`.
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HowItRuns {
    /// `ask-every-time`, `ask-first` or `on-its-own`.
    pub mode: String,
    #[serde(default = "default_promote_after")]
    pub promote_after: u32,
    /// Output only: how far the most accepted action kind is on its way to `promote_after`.
    /// The counts live in the routine's state, not in `routine.json`.
    #[serde(default, skip_serializing)]
    pub accepted: u32,
}

fn default_promote_after() -> u32 {
    5
}

impl Default for HowItRuns {
    fn default() -> Self {
        HowItRuns {
            mode: "ask-first".into(),
            promote_after: 5,
            accepted: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub timeout_min: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { timeout_min: 30 }
    }
}

/// A routine id is a short slug: it names a folder and a schedule entry.
pub fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= 64
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The id of the daemon's schedule entry for a routine.
pub fn schedule_id(routine: &str) -> String {
    format!("routine.{routine}")
}

/// The routine id behind a schedule entry id, if it is one of ours.
pub fn routine_of_schedule(schedule: &str) -> Option<&str> {
    schedule.strip_prefix("routine.")
}
