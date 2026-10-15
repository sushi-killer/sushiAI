use crate::policy::routing::Route;
use crate::policy::routing::Tier;
use crate::policy::settings::Settings;
use crate::task::Task;

/// The second candidate's route when best-of applies to this attempt (the
/// first implement attempt of a hard task with `bestOf` 2), `Err` with a
/// decision line when it applies but there is no route to run it on.
/// `first_attempt` is true when no implement attempt exists yet.
pub(crate) fn second_route(
    task: &Task,
    settings: &Settings,
    route: &Route,
    first_attempt: bool,
) -> Result<Option<Route>, String> {
    let variant = task.variant();
    if variant.best_of < 2 || task.tier != Tier::Hard || !first_attempt {
        return Ok(None);
    }
    let other = match variant.best_of_route.as_deref() {
        Some(id) => settings.routes.iter().find(|r| r.id == id),
        None => settings.routes.iter().find(|r| r.harness != route.harness),
    };
    match other {
        Some(r) => Ok(Some(r.clone())),
        None => Err("Best-of: no route on another harness; ran a single candidate".to_string()),
    }
}
