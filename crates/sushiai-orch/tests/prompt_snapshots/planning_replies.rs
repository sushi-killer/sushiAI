use super::fixtures::{batch, PAST_WORK, REQUEST};
use serde_json::json;
use sushiai_orch::policy::settings::ScopedCheck;
use sushiai_orch::policy::variant::Variant;

pub(super) fn renders() -> Vec<(&'static str, String)> {
    let scoped: Vec<ScopedCheck> = vec![
        serde_json::from_value(json!({
            "command": "cargo test -p sushiai-orch",
            "paths": ["crates/sushiai-orch/**", "Cargo.lock"]
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "repo": "/other",
            "command": "npm run e2e",
            "paths": ["e2e/**"]
        }))
        .unwrap(),
        serde_json::from_value(json!({ "command": "npm run unscoped", "paths": [] })).unwrap(),
    ];
    vec![
        (
            "build_plan_correction_brief",
            sushiai_orch::engine::planning::prompt::build_plan_correction_brief(
                " criteria 0 and 1 clash \n",
                &batch(),
                true,
            ),
        ),
        (
            "build_plan_correction_brief_subtask",
            sushiai_orch::engine::planning::prompt::build_plan_correction_brief(
                "criteria 0 and 1 clash",
                &Variant::default(),
                false,
            ),
        ),
        (
            "build_plan_continue_brief",
            sushiai_orch::engine::planning::prompt::build_plan_continue_brief(&batch(), true),
        ),
        (
            "build_plan_continue_brief_subtask",
            sushiai_orch::engine::planning::prompt::build_plan_continue_brief(
                &Variant::default(),
                false,
            ),
        ),
        (
            "build_plan_retry_brief",
            sushiai_orch::engine::planning::prompt::build_plan_retry_brief(
                REQUEST,
                &batch(),
                PAST_WORK,
            ),
        ),
        (
            "build_subtask_plan_retry_brief",
            sushiai_orch::engine::planning::prompt::build_subtask_plan_retry_brief(
                REQUEST,
                &Variant::default(),
                PAST_WORK,
            ),
        ),
        (
            "scoped_checks_block",
            sushiai_orch::engine::planning::prompt::scoped_checks_block(&scoped, "/repo"),
        ),
    ]
}
