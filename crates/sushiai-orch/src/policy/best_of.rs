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
    if variant.best_of < 2
        || task.tier != Tier::Hard
        || !first_attempt
        || task.pr_check_repair_due()
    {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::test_support::sample_task;
    use crate::task::{AttemptStatus, Failure, FailureKind, TaskStatus};

    #[test]
    fn a_first_hard_repair_step_uses_one_agent_even_with_best_of_enabled() {
        let settings = Settings::default();
        let mut task = sample_task("repair", TaskStatus::Queued, AttemptStatus::Passed);
        task.fixture_data().attempts.clear();
        task.fixture_data().tier = Tier::Hard;
        task.fixture_data().variant = Some(super::super::variant::Variant {
            best_of: 2,
            ..Default::default()
        });
        let route = settings
            .routes
            .iter()
            .find(|route| route.id == settings.tiers[&Tier::Hard])
            .unwrap();
        assert!(second_route(&task, &settings, route, true)
            .unwrap()
            .is_some());
        task.schedule_pr_check_repair(
            "cause".into(),
            Failure {
                kind: FailureKind::Verify,
                detail: "observed failure".into(),
                signature: "same".into(),
            },
        );
        assert!(second_route(&task, &settings, route, true)
            .unwrap()
            .is_none());
    }
}
