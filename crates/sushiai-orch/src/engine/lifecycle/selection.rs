use crate::engine::decision::completed_implement_attempts;
use crate::engine::lines::no_planner_tier_line;
use crate::engine::lines::variant_route_line;
use crate::engine::state::App;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::task::state::Task;

pub(in crate::engine) struct Selection {
    pub(in crate::engine) route: Route,
    pub(in crate::engine) carry_conflicts: Vec<String>,
}

pub(in crate::engine) fn route(
    app: &App,
    task: &mut Task,
    settings: &Settings,
) -> Option<Selection> {
    let carry_conflicts = task.queue.carry_conflicts.clone();
    let (route_id, overridden) = task.variant().implement_route_id(settings, task.tier);
    if overridden {
        let line = variant_route_line(&format!("tier {}", task.tier.as_str()), &route_id);
        if !task.decisions.contains(&line) {
            task.record_decision(line);
        }
    }
    if completed_implement_attempts(task) == 0 && task.planned_tier.is_some() {
        task.record_decision(format!(
            "Planner: tier {} -> route {route_id}",
            task.tier.as_str()
        ));
    }
    if completed_implement_attempts(task) == 0 && task.planned_tier.is_none() {
        task.record_decision(no_planner_tier_line(&route_id));
    }
    let route = settings
        .routes
        .iter()
        .find(|r| r.id == route_id)
        .cloned()
        .unwrap_or(Route {
            id: "codex".to_string(),
            label: "Codex".to_string(),
            harness: Harness::Codex,
            model: None,
            effort: None,
            profile_id: None,
            account_id: None,
            strength: None,
        });
    let Some((route, swap)) = app.usable_route(settings, route) else {
        // No route runs on an installed harness: ask again.
        return None;
    };
    if let Some(line) = swap {
        if !task.decisions.contains(&line) {
            task.record_decision(line);
        }
    }

    Some(Selection {
        route,
        carry_conflicts,
    })
}

#[cfg(test)]
mod tests {
    use super::route;
    use crate::engine::test_support::{task_with_status, test_app};
    use crate::policy::routing::Tier;
    use crate::task::state::TaskStatus;

    #[test]
    fn a_carry_conflict_keeps_the_tasks_own_tier_route() {
        let (app, _dir) = test_app();
        let settings = app.settings.read().unwrap().clone();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().tier = Tier::Hard;
        task.record_carry_conflicts(vec!["src/file.rs".into()]);
        let selected = route(&app, &mut task, &settings).expect("configured tier route");
        assert_eq!(&selected.route.id, settings.tiers.get(&Tier::Hard).unwrap());
        assert_eq!(selected.carry_conflicts, ["src/file.rs"]);
    }
    #[test]
    fn a_check_repair_keeps_the_tasks_own_tier_route() {
        let (app, _dir) = test_app();
        let settings = app.settings.read().unwrap().clone();
        let mut task = task_with_status(TaskStatus::Queued);
        task.fixture_data().tier = Tier::Hard;
        task.schedule_pr_check_repair(
            "cause".into(),
            crate::engine::decision::failure(
                crate::task::FailureKind::Verify,
                "named output".into(),
            ),
        );
        let selected = route(&app, &mut task, &settings).unwrap();
        assert_eq!(&selected.route.id, settings.tiers.get(&Tier::Hard).unwrap());
        assert!(selected.carry_conflicts.is_empty());
    }
}
