use crate::prompts::brief_text;
use crate::verify::review::brief::build_review_brief;

use super::{parse_lens_routing, Lenses};
use crate::engine::attempts::test_fixtures::sample_task;

#[test]
fn a_route_reply_chooses_the_areas_the_full_review_is_briefed_with() {
    let reply = "```sushi-route\n{\"lenses\":{\"seams\":false,\"code\":true,\"tests\":true,\"errors\":false,\"docs\":false},\"reason\":\"logic only\"}\n```";
    let routing = parse_lens_routing(reply).unwrap();
    assert_eq!(routing.lenses.chosen(), ["code", "tests"]);
    assert_eq!(routing.reason, "logic only");
    let text = build_review_brief(
        &sample_task(),
        &crate::verify::review::brief::ReviewEvidence {
            implementer_note: "note",
            agent_decisions: &[],
            verify_results: &[],
            screenshots: &[],
            evidence_from: None,
            declines: &[],
            grown_protected: "",
        },
        "d",
        &routing.lenses,
    );
    assert!(text.contains("## Review areas"));
    assert!(text.contains(brief_text("lens_code")) && text.contains(brief_text("lens_tests")));
    assert!(
        !text.contains(brief_text("lens_seams"))
            && !text.contains(brief_text("lens_errors"))
            && !text.contains(brief_text("lens_docs"))
    );
}

#[test]
fn a_route_reply_missing_an_area_is_refused_and_every_area_is_reviewed() {
    let partial = "```sushi-route\n{\"lenses\":{\"seams\":true,\"code\":true,\"tests\":true,\"errors\":true},\"reason\":\"x\"}\n```";
    assert!(parse_lens_routing(partial).is_none());
    assert!(parse_lens_routing("no route here").is_none());
    let text = build_review_brief(
        &sample_task(),
        &crate::verify::review::brief::ReviewEvidence {
            implementer_note: "note",
            agent_decisions: &[],
            verify_results: &[],
            screenshots: &[],
            evidence_from: None,
            declines: &[],
            grown_protected: "",
        },
        "d",
        &Lenses::ALL,
    );
    assert!(text.contains(brief_text("lens_seams")) && text.contains(brief_text("lens_docs")));
}
