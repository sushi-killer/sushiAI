use super::settings::Settings;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub id: String,
    pub label: String,
    pub harness: Harness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Claude only: a named model-profile (env/apiKeyHelper) merged into the
    /// run's `settings.json`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// Claude only: named subscription or API-key account used for this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// 1 (cheap, mechanical) to 3 (hard). Review routing never picks a route
    /// weaker than the implementer's; unset uses `route_strength`'s default
    /// for the model. A route with no model needs `strength` to meet a review
    /// floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength: Option<u32>,
}

/// The route's explicit strength, else a default from its model:
/// haiku/luna/mini -> 1, opus -> 3, anything else (or no model) -> 2.
pub fn route_strength(route: &Route) -> u32 {
    if let Some(s) = route.strength {
        return s;
    }
    let model = route.model.as_deref().unwrap_or_default().to_lowercase();
    if ["haiku", "luna", "mini"].iter().any(|k| model.contains(k)) {
        1
    } else if model.contains("opus") {
        3
    } else {
        2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Mechanical,
    Standard,
    Hard,
}

impl Tier {
    /// mechanical -> standard -> hard -> hard (caps out).
    pub fn up(self) -> Tier {
        match self {
            Tier::Mechanical => Tier::Standard,
            Tier::Standard => Tier::Hard,
            Tier::Hard => Tier::Hard,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Mechanical => "mechanical",
            Tier::Standard => "standard",
            Tier::Hard => "hard",
        }
    }
}

impl Settings {
    /// The route that drafts a pull request: `pr_route`, else the
    /// `mechanical` tier's route, else the planner route.
    pub fn pr_draft_route(&self) -> Option<&Route> {
        let id = Some(self.pr_route.as_str())
            .filter(|id| !id.is_empty())
            .or_else(|| self.tiers.get(&Tier::Mechanical).map(String::as_str))
            .unwrap_or(&self.planner);
        self.routes.iter().find(|r| r.id == id)
    }
}

#[cfg(test)]
#[path = "tests/routing.rs"]
mod tests;
