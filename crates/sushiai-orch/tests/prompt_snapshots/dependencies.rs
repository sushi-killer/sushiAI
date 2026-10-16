use super::fixtures::{dependency, full_task};
use serde_json::json;
use sushiai_orch::task::Task;
use sushiai_orch::task::TaskStatus;

pub(super) fn renders() -> Vec<(&'static str, String)> {
    let full = full_task();
    let mut with_deps = serde_json::to_value(&full).unwrap();
    with_deps["dependsOn"] = json!(["t-dep-a", "t-missing", "t-dep-b"]);
    let with_deps: Task = serde_json::from_value(with_deps).unwrap();
    let mut not_done = serde_json::to_value(dependency("t-dep-b")).unwrap();
    not_done["status"] = json!(TaskStatus::Running);
    let not_done: Task = serde_json::from_value(not_done).unwrap();
    let all = vec![dependency("t-dep-a"), not_done];

    let mut relayed = serde_json::to_value(&full).unwrap();
    relayed["relayOf"] = json!("t-dep-a");
    let relayed: Task = serde_json::from_value(relayed).unwrap();
    vec![
        (
            "landed_dependencies_block",
            sushiai_orch::engine::attempts::dependencies::landed_dependencies_block(
                &with_deps, &all, None,
            ),
        ),
        (
            "relay_block",
            sushiai_orch::engine::attempts::dependencies::relay_block(&relayed, &all),
        ),
    ]
}
