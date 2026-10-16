use super::routes;
use super::state::task_state;
use super::Spent;
use crate::engine::state::App;
use crate::policy::pipelines::catalog;
use crate::policy::routing::Tier;
use crate::policy::settings::Settings;
use crate::policy::variant::Variant;
use crate::task::Task;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StageOut {
    id: &'static str,
    label: &'static str,
    kind: &'static str,
    enabled: bool,
    optional: bool,
    owner_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    route_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_route_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
}

pub(super) fn describe(
    app: &App,
    settings: &Settings,
    variant: &Variant,
    task: Option<&Task>,
    spent: &HashMap<&'static str, Spent>,
) -> Vec<StageOut> {
    let tier = task.map_or(Tier::Standard, |task| task.tier);
    let mut stages = Vec::new();
    for stage in catalog::compiled().stages() {
        let id = stage.id();
        let label = stage.label();
        let kind = stage.kind();
        let (enabled, optional) = match id {
            "plan" => (!variant.plan_route_id(settings).is_empty(), false),
            "brief-check" => (!variant.brief_check_route_id(settings).is_empty(), true),
            "held-out" => (variant.held_out, true),
            "review" | "judge" => (!variant.review_route_id(settings).is_empty(), false),
            "land" => (variant.land, true),
            "advisor" => (variant.advisor, true),
            _ => (true, false),
        };
        let route = routes::describe(app, settings, variant, id, tier);
        let mut out = StageOut {
            id,
            label,
            kind,
            enabled,
            optional,
            owner_only: matches!(id, "checks" | "base-checks"),
            route_id: route.id,
            route_label: route.label,
            auto_route_label: route.automatic_label,
            auto_model: route.automatic_model,
            model: route.model,
            max_attempts: (id == "implement").then(|| variant.max_attempts(settings)),
            state: task.map(|task| task_state(id, enabled, task, spent)),
            duration_ms: None,
            cost_usd: None,
        };
        if let Some(spent) = spent.get(id).filter(|spent| task.is_some() && spent.seen) {
            out.duration_ms = Some(spent.ms);
            out.cost_usd = Some(spent.usd);
        }
        stages.push(out);
    }
    stages
}
