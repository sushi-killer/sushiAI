use crate::engine::decision::completed_implement_attempts;
use crate::engine::decision::last_failure_is_conflict;
use crate::engine::lines::no_planner_tier_line;
use crate::engine::lines::variant_route_line;
use crate::engine::state::App;
use crate::policy::routing::Harness;
use crate::policy::routing::Route;
use crate::policy::routing::Tier;
use crate::policy::settings::Settings;
use crate::task::Task;

pub(in crate::engine) struct Selection {
    pub(in crate::engine) route: Route,
    pub(in crate::engine) conflict_only: bool,
    pub(in crate::engine) carry_conflicts: Vec<String>,
}

pub(in crate::engine) fn route(
    app: &App,
    task: &mut Task,
    settings: &Settings,
    parent_conflict_only: bool,
) -> Option<Selection> {
    // A landing that conflicted only needs its markers resolved: that
    // runs on the cheap route, not as a full re-implementation.
    let carry_conflicts = task.queue.carry_conflicts.clone();
    let conflict_only = !carry_conflicts.is_empty()
        || last_failure_is_conflict(task)
        || (parent_conflict_only && completed_implement_attempts(task) == 0);
    let (route_id, overridden) = task.variant().implement_route_id(
        settings,
        if conflict_only {
            Tier::Mechanical
        } else {
            task.tier
        },
    );
    if conflict_only {
        task.record_decision(format!(
                "Orchestrator: the carry conflicted -> conflict-only attempt on the cheap route {route_id}"
            ));
    } else if overridden {
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
        conflict_only,
        carry_conflicts,
    })
}
