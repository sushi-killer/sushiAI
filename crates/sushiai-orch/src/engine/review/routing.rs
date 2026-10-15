use crate::policy::pricing::route_cost;
use crate::policy::routing::route_strength;
use crate::policy::routing::Route;
use crate::policy::routing::Tier;
use crate::policy::settings::Settings;

/// The weakest strength a reviewer or judge may have: never below the
/// implementer's, and at least 2 for mechanical/standard work, 3 for hard.
pub(in crate::engine) fn review_floor(implementer: &Route, tier: Tier) -> u32 {
    let tier_floor = if tier == Tier::Hard { 3 } else { 2 };
    route_strength(implementer).max(tier_floor)
}

/// The route's strength when it is known: an explicit `strength`, else the
/// model's default; `None` for a route with no model and no `strength`.
fn known_strength(route: &Route) -> Option<u32> {
    route
        .strength
        .or_else(|| route.model.as_ref().map(|_| route_strength(route)))
}

/// Whether `route` is one of `excluded`, or runs the same harness and model as
/// one of them.
pub(in crate::engine) fn is_excluded(route: &Route, excluded: &[&Route]) -> bool {
    excluded
        .iter()
        .any(|e| e.id == route.id || (e.harness == route.harness && e.model == route.model))
}

/// The best route whose strength is known and at `floor` or above, skipping
/// `exclude` (by id, or by harness and model). Order: a priced route first,
/// then a harness other than `other_than`'s, then the lower price, then
/// settings order. With none at the floor, and `fallback` set, the route with
/// the highest known strength (same order on a tie); a route of unknown
/// strength ranks below every known one. The bool is true when the route
/// reaches the floor.
pub(in crate::engine) fn cheapest_route_at<'a>(
    settings: &'a Settings,
    floor: u32,
    exclude: &[&Route],
    other_than: &Route,
    fallback: bool,
) -> Option<(&'a Route, bool)> {
    let pool: Vec<(usize, &Route)> = settings
        .routes
        .iter()
        .enumerate()
        .filter(|(_, r)| !is_excluded(r, exclude))
        .collect();
    let key = |(i, r): &(usize, &Route)| {
        let cost = route_cost(&settings.prices, r);
        (
            cost.is_none(),
            r.harness == other_than.harness,
            cost.unwrap_or(0.0),
            *i,
        )
    };
    let cmp = |a: &(usize, &Route), b: &(usize, &Route)| {
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0)
            .then(ka.1.cmp(&kb.1))
            .then(ka.2.total_cmp(&kb.2))
            .then(ka.3.cmp(&kb.3))
    };
    if let Some(found) = pool
        .iter()
        .filter(|(_, r)| known_strength(r).is_some_and(|s| s >= floor))
        .min_by(|a, b| cmp(a, b))
    {
        return Some((found.1, true));
    }
    if !fallback {
        return None;
    }
    let top = pool.iter().map(|(_, r)| known_strength(r)).max()?;
    pool.iter()
        .filter(|(_, r)| known_strength(r) == top)
        .min_by(|a, b| cmp(a, b))
        .map(|(_, r)| (*r, false))
}

/// The review route and a one-line reason for the global review setting.
/// See `select_review_route_with`. Only the unit tests call it.
#[cfg(test)]
pub fn select_review_route<'a>(
    settings: &'a Settings,
    implementer: &Route,
    tier: Tier,
) -> Option<(&'a Route, String)> {
    select_review_route_with(settings, &settings.review, implementer, tier)
}

/// The review route and a one-line reason. `review_id` is the effective review
/// id (`Variant::review_route_id`). An explicit id is used as is. `"auto"`
/// never picks the implementer's route nor any route with its harness and
/// model: it takes the best route of known strength at `review_floor` or
/// above (priced, on another harness, then cheapest), else the strongest
/// remaining one. Only when no other route exists does it fall back to the
/// implementer's own route, reason `self-review: no other route`.
/// `""` -> no review (handled by the caller).
pub fn select_review_route_with<'a>(
    settings: &'a Settings,
    review_id: &str,
    implementer: &Route,
    tier: Tier,
) -> Option<(&'a Route, String)> {
    if review_id != "auto" {
        let route = settings.routes.iter().find(|r| r.id == review_id)?;
        return Some((route, "explicit setting".to_string()));
    }
    let floor = review_floor(implementer, tier);
    let Some((route, reached)) =
        cheapest_route_at(settings, floor, &[implementer], implementer, true)
    else {
        let own = settings
            .routes
            .iter()
            .find(|r| r.id == implementer.id)
            .or_else(|| {
                settings
                    .routes
                    .iter()
                    .find(|r| r.harness == implementer.harness && r.model == implementer.model)
            })?;
        return Some((own, "self-review: no other route".to_string()));
    };
    let reason = if reached {
        format!(
            "{} tier, strength {}",
            tier_name(tier),
            route_strength(route)
        )
    } else if known_strength(route).is_none() {
        format!(
            "no route at strength {floor}; {} has no model and no strength: set strength on it",
            route.id
        )
    } else {
        format!("no route at strength {floor}; strongest available")
    };
    Some((route, reason))
}

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Mechanical => "mechanical",
        Tier::Standard => "standard",
        Tier::Hard => "hard",
    }
}
