use super::paths::same_repo_path;
use crate::policy::presets::Preset;
use crate::policy::settings::{GateAt, SandboxMode, Settings};

impl Settings {
    pub fn sandbox_for(&self, repo: &str) -> SandboxMode {
        self.sandbox_repos
            .iter()
            .find(|(path, _)| same_repo_path(path, repo))
            .map(|(_, mode)| *mode)
            .unwrap_or(self.sandbox)
    }

    /// Whether the checks a subtask shares with its parent run at the parent:
    /// the per-repo entry when there is one, else true.
    pub fn gates_at_parent(&self, repo: &str) -> bool {
        self.gate_at
            .iter()
            .find(|(k, _)| same_repo_path(k, repo))
            .map(|(_, at)| *at == GateAt::Parent)
            .unwrap_or(true)
    }

    /// Whether a task may land on `repo`'s default branch: the per-repo entry
    /// when there is one, else the global switch.
    pub fn may_land_on_default(&self, repo: &str) -> bool {
        self.land_on_default_repos
            .iter()
            .find(|(k, _)| same_repo_path(k, repo))
            .map(|(_, allowed)| *allowed)
            .unwrap_or(self.land_on_default)
    }

    /// Whether a done task whose work did not land gets a pull request: the
    /// per-repo entry when there is one, else on unless switched off.
    pub fn may_open_pr_on_done(&self, repo: &str) -> bool {
        self.pr_on_done_repos
            .iter()
            .find(|(k, _)| same_repo_path(k, repo))
            .map(|(_, on)| *on)
            .unwrap_or(!self.pr_on_done_off)
    }

    /// The preset id `repoPresets` names for `repo`.
    pub fn repo_preset_id(&self, repo: &str) -> Option<&str> {
        self.repo_presets
            .iter()
            .find(|(k, _)| same_repo_path(k, repo))
            .map(|(_, id)| id.as_str())
    }

    /// The preset a task created in `repo` starts from, when one is set.
    pub fn repo_preset(&self, repo: &str) -> Option<Preset> {
        self.preset(self.repo_preset_id(repo)?)
    }
}

#[cfg(test)]
#[path = "tests/repo_settings.rs"]
mod tests;
