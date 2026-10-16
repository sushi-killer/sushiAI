use crate::engine::availability::cheapest_available;
use crate::engine::state::App;
use crate::policy::routing::Route;
use crate::policy::variant::Variant;

/// The `briefCheckRoute` id, the route it resolves to (`None` when no route
/// on an available harness exists) and, when the id is not usable and the
/// cheapest available route stands in, the reason to record; `None` overall
/// when the setting is empty, i.e. cheap checks are off. `"auto"` names no
/// route: it is the cheapest configured route whose harness is available.
/// The variant may switch the check off or name another route.
pub(in crate::engine) fn check_route_for(
    app: &App,
    variant: &Variant,
) -> Option<(String, Option<Route>, Option<String>)> {
    let settings = app.settings.read().unwrap();
    let route_id = variant.brief_check_route_id(&settings).to_string();
    if route_id.is_empty() {
        return None;
    }
    let has = |h| app.harness_available(h);
    let named = settings
        .routes
        .iter()
        .find(|r| r.id == route_id && has(r.harness));
    if let Some(route) = named {
        return Some((route_id.clone(), Some(route.clone()), None));
    }
    let cheapest = cheapest_available(&settings, has);
    let note = cheapest.as_ref().filter(|_| route_id != "auto").map(|c| {
        format!(
            "route {route_id} is not configured or unavailable; using the cheapest available route {}",
            c.id
        )
    });
    Some((route_id, cheapest, note))
}
