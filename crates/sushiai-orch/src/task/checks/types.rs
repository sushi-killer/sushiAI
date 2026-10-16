use serde::{Deserialize, Serialize};

/// One test file the planner wants committed before the first attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedTest {
    pub path: String,
    pub content: String,
}

/// Who wrote a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckSource {
    /// The planner wrote it for this task.
    Planner,
    /// The repo's own verify, CI or preset command.
    Repo,
    /// The owner wrote it in the request or an answer.
    Owner,
}

/// A command of the plan and who wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcedCheck {
    pub command: String,
    pub source: CheckSource,
}

/// The planner's tests and the files the implementer may not change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannerTests {
    /// Files kept from the plan, written and committed before attempt 1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<PlannedTest>,
    /// The commit an older build made of them. Only read to undo it once
    /// (`protected::migrate_tests_commit`); the files live in `files`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// path -> sha256 of every protected file (the planner's files as stored,
    /// the others as on the base); `None` until computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protected: Option<std::collections::BTreeMap<String, String>>,
    /// Protected files the last attempt changed and orchd restored; the next
    /// brief tells the agent. Replaced after every attempt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discarded: Vec<String>,
    /// Planner test files whose "this test is wrong" claim was already ruled
    /// on; a claim is judged once per file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ruled: Vec<String>,
    /// Where each command of the plan came from, recorded when the plan is
    /// stored. A command not listed is the repo's. Only a `Planner` command
    /// may be dropped without the owner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourcedCheck>,
    /// Older tasks stored the planner's commands as bare strings; the store
    /// converts them once into `sources` (`store::migrate_check_sources`).
    #[serde(default, rename = "commands", skip_serializing)]
    pub legacy_commands: Vec<String>,
}

impl PlannerTests {
    pub fn is_empty(&self) -> bool {
        *self == PlannerTests::default()
    }
}

/// How a check ran on the base checkout, before any work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Baseline {
    /// Already passes: it proves nothing, so it does not gate.
    Pass,
    /// Fails as it should: it gates the attempt.
    Fail,
    /// Could not run (missing tool, spawn failure): left out.
    Env,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// Index into `Task::criteria`.
    pub criterion: usize,
    pub run: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<Baseline>,
}

/// Drops checks whose criterion is out of range or whose command is blank,
/// and clears any supplied baseline: only the base run may set it.
pub fn valid_checks(checks: Vec<Check>, criteria_len: usize) -> Vec<Check> {
    checks
        .into_iter()
        .filter(|c| c.criterion < criteria_len && !c.run.trim().is_empty())
        .map(|c| Check {
            baseline: None,
            ..c
        })
        .collect()
}

/// [`valid_checks`] for the one held-out check.
pub fn valid_check(check: Option<Check>, criteria_len: usize) -> Option<Check> {
    valid_checks(check.into_iter().collect(), criteria_len)
        .into_iter()
        .next()
}
