use super::fixtures::{message, where_to_look};

pub(super) fn renders() -> Vec<(&'static str, String)> {
    vec![
        (
            "discarded_block",
            sushiai_orch::engine::attempts::context::discarded_block(&[
                "tests/a.test.ts".to_string(),
                "tests/b.test.ts".to_string(),
            ]),
        ),
        (
            "coordination_block",
            sushiai_orch::engine::attempts::context::coordination_block(
                "t-fixture",
                &[
                    (
                        "t-other".to_string(),
                        message("m1", "question", "Which file?"),
                    ),
                    (
                        "t-third".to_string(),
                        message("m2", "reply", "settings.json"),
                    ),
                    ("t-lead".to_string(), message("m3", "message", "FYI")),
                ],
            ),
        ),
        (
            "conflict_block",
            sushiai_orch::engine::attempts::context::conflict_block(
                "criterion [0] wants it hidden, criterion [1] clicked",
            ),
        ),
        (
            "carry_conflict_block",
            sushiai_orch::engine::attempts::context::carry_conflict_block(
                &["src/a.ts".to_string(), "src/b.ts".to_string()],
                "t-1",
            ),
        ),
        (
            "parent_failure_block",
            sushiai_orch::engine::attempts::context::parent_failure_block("`npm run ci` exited 1."),
        ),
        (
            "review_conflict_block",
            sushiai_orch::verify::review::brief::review_conflict_block(
                "criterion [0] wants it hidden, criterion [1] clicked",
            ),
        ),
        ("where_to_look_block", where_to_look()),
        (
            "build_audit_brief",
            sushiai_orch::engine::audit::brief::build_audit_brief(),
        ),
    ]
}
