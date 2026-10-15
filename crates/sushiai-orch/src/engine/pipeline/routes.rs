use crate::engine::brief_check::routing::check_route_for;
use crate::engine::finding_judge::resolve_judge_route;
use crate::engine::review::routing::select_review_route_with;
use crate::engine::state::App;
use crate::policy::routing::{Route, Tier};
use crate::policy::settings::Settings;
use crate::policy::variant::Variant;

pub(super) struct StageRoute {
    pub(super) id: Option<String>,
    pub(super) label: Option<String>,
    pub(super) model: Option<String>,
    pub(super) automatic_label: Option<String>,
    pub(super) automatic_model: Option<String>,
}

pub(super) fn describe(
    app: &App,
    settings: &Settings,
    variant: &Variant,
    id: &str,
    tier: Tier,
) -> StageRoute {
    let route = match id {
        "plan" | "advisor" => variant.plan_route_id(settings).to_string(),
        "brief-check" => variant.brief_check_route_id(settings).to_string(),
        "implement" => variant.implement_route_id(settings, tier).0,
        "review" => variant.review_route_id(settings).to_string(),
        "judge" => variant.judge_route.as_deref().unwrap_or("auto").to_string(),
        _ => String::new(),
    };
    let resolved = settings
        .routes
        .iter()
        .find(|candidate| candidate.id == route);
    let mut out = StageRoute {
        id: (!route.is_empty()).then(|| route.clone()),
        label: resolved
            .filter(|_| route != "auto")
            .map(|route| route.label.clone()),
        model: resolved.and_then(|route| route.model.clone()),
        automatic_label: None,
        automatic_model: None,
    };
    match id {
        "brief-check" if !route.is_empty() => brief(app, variant, &mut out),
        "review" if !route.is_empty() => review(settings, variant, tier, &mut out),
        "judge" if !variant.review_route_id(settings).is_empty() => {
            judge(settings, variant, tier, &mut out)
        }
        _ => {}
    }
    out
}

fn brief(app: &App, variant: &Variant, out: &mut StageRoute) {
    let resolved = check_route_for(app, variant).and_then(|(_, route, _)| route);
    out.model = resolved
        .as_ref()
        .map(|route| route.model.clone().unwrap_or_else(|| route.label.clone()));
    out.label = resolved.map(|route| route.label);
    let mut automatic = variant.clone();
    automatic.brief_check_route = Some("auto".to_string());
    let automatic = check_route_for(app, &automatic).and_then(|(_, route, _)| route);
    out.automatic_label = automatic.as_ref().map(|route| route.label.clone());
    out.automatic_model = automatic.and_then(|route| route.model);
}

fn implementer<'a>(settings: &'a Settings, variant: &Variant, tier: Tier) -> Option<&'a Route> {
    let id = variant.implement_route_id(settings, tier).0;
    settings.routes.iter().find(|route| route.id == id)
}

fn review(settings: &Settings, variant: &Variant, tier: Tier, out: &mut StageRoute) {
    let implementer = implementer(settings, variant, tier);
    let selected = implementer.and_then(|implementer| {
        select_review_route_with(
            settings,
            variant.review_route_id(settings),
            implementer,
            tier,
        )
    });
    if out.id.as_deref() == Some("auto") {
        out.model = selected.as_ref().and_then(|(route, _)| route.model.clone());
    }
    out.label = selected.map(|(route, _)| route.label.clone());
    let automatic = implementer
        .and_then(|implementer| select_review_route_with(settings, "auto", implementer, tier));
    out.automatic_label = automatic.as_ref().map(|(route, _)| route.label.clone());
    out.automatic_model = automatic.and_then(|(route, _)| route.model.clone());
}

fn judge(settings: &Settings, variant: &Variant, tier: Tier, out: &mut StageRoute) {
    let implementer = implementer(settings, variant, tier);
    let reviewer = implementer.and_then(|implementer| {
        select_review_route_with(
            settings,
            variant.review_route_id(settings),
            implementer,
            tier,
        )
    });
    let (selected, automatic) = match (reviewer, implementer) {
        (Some((reviewer, _)), Some(implementer)) => (
            resolve_judge_route(
                settings,
                variant.judge_route.as_deref(),
                reviewer,
                implementer,
                tier,
            ),
            resolve_judge_route(settings, None, reviewer, implementer, tier),
        ),
        _ => (None, None),
    };
    out.automatic_label = automatic.map(|route| route.label.clone());
    out.automatic_model = automatic.and_then(|route| route.model.clone());
    out.label = selected.map(|route| route.label.clone());
    out.model = selected.and_then(|route| route.model.clone());
}
