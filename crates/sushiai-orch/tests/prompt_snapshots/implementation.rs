use super::fixtures::{full_task, task};
use serde_json::json;

pub(super) fn renders() -> Vec<(&'static str, String)> {
    let bare = task(json!({ "criteria": [], "verify": [] }));
    let full = full_task();
    let status = " M src/settings.ts\n?? tests/save.test.ts\n";
    let stat = " src/settings.ts | 4 ++--\n 1 file changed, 2 insertions(+), 2 deletions(-)\n";
    let advice = "The lint failure is an unused import; remove it.";
    let mut timed_out = full.attempts[2].clone();
    timed_out.failure.as_mut().unwrap().kind = sushiai_orch::task::FailureKind::TimeBudget;
    timed_out.failure.as_mut().unwrap().detail = "Attempt reached its time budget".into();
    timed_out.verify.clear();
    vec![
        (
            "build_brief_bare",
            sushiai_orch::engine::attempts::brief::build_brief(&bare, "", ""),
        ),
        (
            "build_brief_full",
            sushiai_orch::engine::attempts::brief::build_brief(&full, status, stat),
        ),
        (
            "build_resume_brief",
            sushiai_orch::engine::attempts::resume::build_resume_brief(
                &full,
                &full.attempts[2],
                "## Messages\n\nA peer says hello.",
                Some(advice),
            ),
        ),
        (
            "build_resume_brief_time_budget",
            sushiai_orch::engine::attempts::resume::build_resume_brief(&full, &timed_out, "", None),
        ),
    ]
}
