use super::fixtures::{batch, PAST_WORK, REQUEST};
use sushiai_orch::policy::variant::Variant;

pub(super) fn renders() -> Vec<(&'static str, String)> {
    vec![
        (
            "build_plan_brief",
            sushiai_orch::engine::planning::prompt::build_plan_brief(
                REQUEST,
                &Variant::default(),
                PAST_WORK,
            ),
        ),
        (
            "build_plan_brief_batch",
            sushiai_orch::engine::planning::prompt::build_plan_brief(REQUEST, &batch(), PAST_WORK),
        ),
        (
            "build_plan_brief_no_past_work",
            sushiai_orch::engine::planning::prompt::build_plan_brief(
                REQUEST,
                &Variant::default(),
                "",
            ),
        ),
        (
            "build_subtask_plan_brief",
            sushiai_orch::engine::planning::prompt::build_subtask_plan_brief(
                REQUEST,
                &Variant::default(),
                PAST_WORK,
            ),
        ),
        (
            "build_subtask_plan_brief_batch",
            sushiai_orch::engine::planning::prompt::build_subtask_plan_brief(
                REQUEST,
                &batch(),
                PAST_WORK,
            ),
        ),
    ]
}
