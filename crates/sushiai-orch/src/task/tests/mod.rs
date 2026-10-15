use crate::policy::routing::{Harness, Route};
use crate::task::Task;
use serde_json::json;

mod attempts;
mod checks;
mod transitions;

fn task() -> Task {
    serde_json::from_value(json!({
        "id": "task-fixture",
        "title": "A synthetic task",
        "goal": "Exercise task transitions",
        "criteria": [],
        "verify": [],
        "repo": "/synthetic/repo",
        "worktree": "/synthetic/worktree",
        "branch": "task/synthetic",
        "baseSha": "synthetic-base",
        "status": "drafting",
        "tier": "standard",
        "createdAt": 1,
        "updatedAt": 1
    }))
    .unwrap()
}

fn route() -> Route {
    Route {
        id: "fixture-route".into(),
        label: "Fixture route".into(),
        harness: Harness::Claude,
        model: Some("fixture-model".into()),
        effort: None,
        profile_id: None,
        account_id: None,
        strength: None,
    }
}

mod model_serde;
