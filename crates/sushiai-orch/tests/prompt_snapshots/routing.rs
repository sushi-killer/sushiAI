use super::fixtures::{full_task, task};
use serde_json::json;
use sushiai_orch::verify::review::routing::Lenses;

pub(super) fn renders() -> Vec<(&'static str, String)> {
    let bare = task(json!({ "criteria": [], "verify": [] }));
    let full = full_task();
    let some_lenses = Lenses {
        seams: true,
        code: false,
        tests: true,
        errors: false,
        docs: true,
    };
    let no_lenses = Lenses {
        seams: false,
        code: false,
        tests: false,
        errors: false,
        docs: false,
    };
    let changed = vec!["src/settings.ts".to_string(), "src/dialog.tsx".to_string()];
    vec![
        (
            "review_retry_note",
            sushiai_orch::verify::review::brief::review_retry_note(),
        ),
        (
            "review_lens_block_all",
            sushiai_orch::verify::review::routing::review_lens_block(&Lenses::ALL),
        ),
        (
            "review_lens_block_some",
            sushiai_orch::verify::review::routing::review_lens_block(&some_lenses),
        ),
        (
            "review_lens_block_none",
            sushiai_orch::verify::review::routing::review_lens_block(&no_lenses),
        ),
        (
            "build_route_brief",
            sushiai_orch::verify::review::routing::build_route_brief(&full, &changed),
        ),
        (
            "build_route_brief_bare",
            sushiai_orch::verify::review::routing::build_route_brief(&bare, &[]),
        ),
    ]
}
