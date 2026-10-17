use super::fixtures::{review_task, review_verify};
use sushiai_orch::task::Decline;
use sushiai_orch::task::DeclineReason;
use sushiai_orch::verify::review::routing::Lenses;

pub(super) fn renders() -> Vec<(&'static str, String)> {
    let diff = "--- a/src/settings.ts\n+++ b/src/settings.ts\n@@ -1 +1 @@\n-old\n+new";
    let note = "Outcome complete: wired the Save button.";
    let agent_decisions = vec!["Agent: kept the existing settings schema".to_string()];
    let screenshots = vec![
        ("artifacts/settings.png".to_string(), Some(4)),
        ("artifacts/dialog.png".to_string(), None),
    ];
    let review = review_task();
    let declines = vec![
        Decline {
            id: "F-1a2b3c".into(),
            reason: DeclineReason::AlreadySatisfied,
            evidence: vec!["src/settings.ts:41 catches the write error".into()],
        },
        Decline {
            id: "F-4d5e6f".into(),
            reason: DeclineReason::Wrong,
            evidence: vec!["src/dialog.tsx:12".into(), "artifacts/dialog.png".into()],
        },
    ];
    let some_lenses = Lenses {
        seams: true,
        code: false,
        tests: true,
        errors: false,
        docs: true,
    };
    vec![(
        "build_review_brief",
        sushiai_orch::verify::review::brief::build_review_brief(
            &review,
            &sushiai_orch::verify::review::brief::ReviewEvidence {
                implementer_note: note,
                agent_decisions: &agent_decisions,
                verify_results: &review_verify(),
                screenshots: &screenshots,
                evidence_from: Some(3),
                declines: &declines,
                grown_protected: "--- a/tests/settings.test.ts\n+++ b/tests/settings.test.ts\n@@ -3,2 +3,5 @@\n test(\"loads\", load);\n+test(\"saves\", save);\n test(\"resets\", reset);",
            },
            diff,
            &some_lenses,
        ),
    )]
}
