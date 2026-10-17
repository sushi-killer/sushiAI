use super::fixtures::{full_task, task};
use serde_json::json;
use sushiai_orch::verify::review::brief::NarrowReview;
use sushiai_orch::verify::review::routing::Lenses;

pub(super) fn renders() -> Vec<(&'static str, String)> {
    let full = full_task();
    let diff = "--- a/src/settings.ts\n+++ b/src/settings.ts\n@@ -1 +1 @@\n-old\n+new";
    let note = "Outcome complete: wired the Save button.";
    let first_review = task(json!({}));
    let settled_criteria = vec![(0, full.criteria[0].clone())];
    let narrow_settled = NarrowReview {
        open_criteria: &[],
        settled_criteria: &settled_criteria,
        delta: diff,
    };
    vec![
        (
            "build_review_brief_first",
            sushiai_orch::verify::review::brief::build_review_brief(
                &first_review,
                &sushiai_orch::verify::review::brief::ReviewEvidence {
                    implementer_note: note,
                    agent_decisions: &[],
                    verify_results: &[],
                    screenshots: &[],
                    evidence_from: None,
                    declines: &[],
                    grown_protected: "",
                },
                diff,
                &Lenses::ALL,
            ),
        ),
        (
            "build_narrow_review_brief_settled",
            sushiai_orch::verify::review::brief::build_narrow_review_brief(
                &first_review,
                &sushiai_orch::verify::review::brief::ReviewEvidence {
                    implementer_note: note,
                    agent_decisions: &[],
                    verify_results: &[],
                    screenshots: &[],
                    evidence_from: None,
                    declines: &[],
                    grown_protected: "",
                },
                &narrow_settled,
            ),
        ),
    ]
}
