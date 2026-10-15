use super::routing::Tier;
use super::settings::Settings;
use super::variant::Variant;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A named bundle of stage switches a repo or a task starts from. Every
/// value is optional: a preset names only what it changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: String,
    pub label: String,
    /// Defined in code; never written to settings.
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub values: PresetValues,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PresetValues {
    /// `false` turns the brief check off; `true` leaves the settings' route.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brief_check: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advisor: Option<bool>,
    /// Switch only the hidden check; regular checks are always enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held_out: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brief_check_route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier_routes: Option<BTreeMap<Tier, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
}

impl PresetValues {
    /// Lays these values over `variant`; the one place a preset is applied.
    pub fn apply(&self, variant: &mut Variant) {
        if let Some(on) = self.brief_check {
            variant.brief_check_route = if on { None } else { Some(String::new()) };
        }
        if let Some(on) = self.advisor {
            variant.advisor = on;
        }
        if let Some(on) = self.held_out {
            variant.held_out = on;
        }
        if let Some(id) = &self.planner {
            variant.planner_route = Some(id.clone());
        }
        if let Some(id) = &self.brief_check_route {
            variant.brief_check_route = Some(id.clone());
        }
        if let Some(id) = &self.review {
            variant.review_route = Some(id.clone());
        }
        if let Some(id) = &self.judge {
            variant.judge_route = Some(id.clone());
        }
        if let Some(routes) = &self.tier_routes {
            variant.tier_routes = routes.clone();
        }
        if let Some(n) = self.max_attempts {
            variant.max_attempts = Some(n);
        }
    }
}

impl Preset {
    /// Fast, Careful and Docs: switches and `maxAttempts` only, never a
    /// route id.
    pub fn builtins() -> Vec<Preset> {
        let make = |id: &str, label: &str, check: bool, advisor: bool, held: bool, n: u32| Preset {
            id: id.to_string(),
            label: label.to_string(),
            builtin: true,
            values: PresetValues {
                brief_check: Some(check),
                advisor: Some(advisor),
                held_out: Some(held),
                max_attempts: Some(n),
                ..PresetValues::default()
            },
        };
        vec![
            make("fast", "Fast", false, false, false, 2),
            make("careful", "Careful", true, true, true, 4),
            make("docs", "Docs", true, false, false, 2),
        ]
    }
}

impl Settings {
    /// Built-in presets, then the custom ones.
    pub fn all_presets(&self) -> Vec<Preset> {
        let mut all = Preset::builtins();
        all.extend(self.presets.iter().cloned());
        all
    }

    pub fn preset(&self, id: &str) -> Option<Preset> {
        self.all_presets().into_iter().find(|p| p.id == id)
    }

    /// Custom preset ids are unique and not built-in, preset routes are
    /// configured, and `repoPresets` names known presets.
    pub fn check_presets(&self) -> Result<(), String> {
        let builtin = Preset::builtins();
        let mut seen = std::collections::HashSet::new();
        let known = |id: &str| self.routes.iter().any(|r| r.id == id);
        for p in &self.presets {
            if p.id.trim().is_empty() {
                return Err("a preset id must not be empty".to_string());
            }
            if builtin.iter().any(|b| b.id == p.id) {
                return Err(format!("preset id \"{}\" is a built-in preset", p.id));
            }
            if !seen.insert(p.id.as_str()) {
                return Err(format!("preset id \"{}\" is duplicated", p.id));
            }
            let v = &p.values;
            if let Some(id) = v.planner.as_deref().filter(|id| !known(id)) {
                return Err(format!(
                    "preset {} planner \"{id}\" is not a configured route",
                    p.id
                ));
            }
            for (key, id) in [
                ("review", &v.review),
                ("briefCheckRoute", &v.brief_check_route),
            ] {
                if let Some(id) = id
                    .as_deref()
                    .filter(|id| !id.is_empty() && *id != "auto" && !known(id))
                {
                    return Err(format!(
                        "preset {} {key} \"{id}\" is not a configured route",
                        p.id
                    ));
                }
            }
            if let Some(id) = v.judge.as_deref().filter(|id| *id != "auto" && !known(id)) {
                return Err(format!(
                    "preset {} judge \"{id}\" is not a configured route",
                    p.id
                ));
            }
            for (tier, id) in v.tier_routes.iter().flatten() {
                if !known(id) {
                    return Err(format!(
                        "preset {} tierRoutes.{} \"{id}\" is not a configured route",
                        p.id,
                        tier.as_str()
                    ));
                }
            }
            if v.max_attempts == Some(0) {
                return Err(format!("preset {} maxAttempts must be at least 1", p.id));
            }
        }
        for (repo, id) in &self.repo_presets {
            if self.preset(id).is_none() {
                return Err(format!("repoPresets {repo}: unknown preset \"{id}\""));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/presets.rs"]
mod tests;
