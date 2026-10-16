use super::fixtures::{full_task, review_verify};
use sushiai_orch::engine::attempts::pick::PickCandidate;

pub(super) fn renders() -> Vec<(&'static str, String)> {
    let full = full_task();
    let diff = "--- a/src/settings.ts\n+++ b/src/settings.ts\n@@ -1 +1 @@\n-old\n+new";
    let advice = "The lint failure is an unused import; remove it.";

    let mut pick_a = review_verify();
    pick_a.truncate(1);
    let pick_b = review_verify();
    vec![
        (
            "build_pick_brief",
            sushiai_orch::engine::attempts::pick::build_pick_brief(
                &full,
                &PickCandidate {
                    verify: &pick_a,
                    diff,
                },
                &PickCandidate {
                    verify: &pick_b,
                    diff: "+other",
                },
            ),
        ),
        (
            "build_advisor_brief",
            sushiai_orch::engine::attempts::advisor::build_advisor_brief(
                &full,
                diff,
                "`npm run lint` exited 1.",
            ),
        ),
        (
            "advisor_block",
            sushiai_orch::engine::attempts::advisor::advisor_block(advice),
        ),
    ]
}
