use serde::{Deserialize, Serialize};

/// Thresholds for the evolution loop (`engine/evolution`): what counts as
/// enough evidence and how many proposals one round may make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct EvolutionSettings {
    /// Earlier tasks a repo needs before per-repo baselines mean anything.
    pub min_tasks: u32,
    /// Tool calls a cluster of signals must have wasted to be worth a proposal.
    pub min_wasted_calls: u32,
    pub min_wasted_usd: f64,
    pub max_proposals: u32,
    /// Route id that writes proposals; "" means the hard tier's route.
    pub proposer_route: String,
    /// Tasks after which an accepted change is judged and possibly reverted.
    pub revert_after_tasks: u32,
    /// Process-file calls in one attempt before `process_read` fires.
    pub process_min_calls: u32,
    /// Per repo path: substrings that mark a tool call as reading process
    /// files (lessons, changelogs, convention scripts) for `process_read`.
    /// Empty by default: those files describe one repository, so they live
    /// with that repository's entry, never in a global list.
    pub process_files: std::collections::BTreeMap<String, Vec<String>>,
}

impl Default for EvolutionSettings {
    fn default() -> Self {
        EvolutionSettings {
            min_tasks: 3,
            min_wasted_calls: 20,
            min_wasted_usd: 1.0,
            max_proposals: 3,
            proposer_route: String::new(),
            revert_after_tasks: 10,
            process_min_calls: 3,
            process_files: Default::default(),
        }
    }
}

pub mod proposal;
