use crate::prompts::brief_text;
use crate::task::questions::Risk;
use crate::task::review::{Decline, DeclineReason, Verdict};
use crate::verify::review::reply::parse_review;

use super::{agent_decision_lines, parse_impossible, parse_report, Impossible, Outcome};

#[test]
fn a_reply_that_drops_the_backticks_still_parses() {
    // Captured from a real Codex review.
    let r = parse_review(
        "sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[{\"severity\":\"P1\",\"finding\":\"note is wrong\"}]}",
    )
    .unwrap();
    assert_eq!(r.verdict, Verdict::Fail);
    assert_eq!(r.findings, vec!["note is wrong: P1".to_string()]);
    let report =
        parse_report("sushi-report\n{\"outcome\":\"blocked\",\"question\":\"Which?\"}").unwrap();
    assert_eq!(report.outcome, Outcome::Blocked);
}

#[test]
fn a_blocked_report_carries_its_risk() {
    let report = parse_report(
        "sushi-report\n{\"outcome\":\"blocked\",\"question\":\"Which?\",\"risk\":[\"security\"]}",
    )
    .unwrap();
    assert_eq!(report.risk, vec![Risk::Security]);
    let plain =
        parse_report("sushi-report\n{\"outcome\":\"blocked\",\"question\":\"Which?\"}").unwrap();
    assert!(plain.risk.is_empty());
    assert!(brief_text("report_format").contains("\"risk\":[]"));
    assert!(brief_text("rules").contains("`risk`"));
}

#[test]
fn parse_report_reads_a_null_or_structured_handoff() {
    let report = |handoff: &str| {
        parse_report(&format!(
            "```sushi-report\n{{\"outcome\":\"blocked\",\"handoff\":{handoff},\"question\":\"Which?\"}}\n```"
        ))
        .expect("the report still parses")
    };
    assert_eq!(report("null").handoff, "");
    assert_eq!(report("\"next: x\"").handoff, "next: x");
    let structured = report("{\"next\":\"fix y\"}");
    assert_eq!(structured.outcome, Outcome::Blocked);
    assert!(structured.handoff.contains("fix y"));
}

#[test]
fn parse_report_reads_last_fence_and_ignores_earlier_ones() {
    let text = "blah\n```sushi-report\n{\"outcome\":\"partial\",\"summary\":\"old\"}\n```\nmore text\n```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"final answer\",\"decisions\":[\"chose X\"],\"question\":\"\"}\n```\n";
    let report = parse_report(text).unwrap();
    assert_eq!(report.outcome, Outcome::Complete);
    assert_eq!(report.summary, "final answer");
    assert_eq!(report.decisions, vec!["chose X".to_string()]);
}

#[test]
fn parse_report_returns_none_when_malformed_or_missing() {
    assert!(parse_report("no fence here").is_none());
    assert!(parse_report("```sushi-report\nnot json\n```").is_none());
    assert!(parse_report("```sushi-report\n{\"outcome\":\"unknown\"}\n```").is_none());
}

#[test]
fn agent_decision_lines_strip_an_echoed_prefix() {
    assert_eq!(
        agent_decision_lines(&["Owner: a".into(), "Agent:  b".into(), "c".into()]),
        vec!["Agent: a", "Agent: b", "Agent: c"]
    );
}

#[test]
fn a_sushi_impossible_block_needs_a_fence_and_a_criterion_in_range() {
    let text = "```sushi-impossible\n{\"criterion\": 1, \"evidence\": \"contradicts 0\"}\n```\n```sushi-report\n{\"outcome\":\"blocked\"}\n```";
    assert_eq!(
        parse_impossible(text, 2),
        Some(Impossible {
            criterion: 1,
            evidence: "contradicts 0".into()
        })
    );
    assert_eq!(parse_impossible(text, 1), None, "out of range");
    // No fence: the report's own JSON must not be read as a claim.
    assert_eq!(
        parse_impossible("```sushi-report\n{\"criterion\": 0}\n```", 3),
        None
    );
    assert_eq!(parse_impossible("{\"criterion\": 0}", 3), None);
}

#[test]
fn a_report_reads_well_formed_declines() {
    let text = "```sushi-report\n{\"outcome\":\"complete\",\"declines\":[{\"id\":\" F-1 \",\"reason\":\"already_satisfied\",\"evidence\":[\"a.rs:1\",\"artifacts/x.png\"]},{\"id\":\"F-2\",\"reason\":\"changes_behaviour\",\"evidence\":[\"b.rs:9\"]}]}\n```";
    let report = parse_report(text).unwrap();
    assert_eq!(
        report.declines,
        [
            Decline {
                id: "F-1".into(),
                reason: DeclineReason::AlreadySatisfied,
                evidence: vec!["a.rs:1".into(), "artifacts/x.png".into()],
            },
            Decline {
                id: "F-2".into(),
                reason: DeclineReason::ChangesBehaviour,
                evidence: vec!["b.rs:9".into()],
            },
        ]
    );
}

#[test]
fn a_decline_without_evidence_or_a_known_reason_is_refused() {
    let parse = |decline: &str| {
        parse_report(&format!(
            "```sushi-report\n{{\"outcome\":\"complete\",\"declines\":[{decline}]}}\n```"
        ))
        .unwrap()
        .declines
    };
    assert!(parse(r#"{"id":"F-1","reason":"wrong"}"#).is_empty());
    assert!(parse(r#"{"id":"F-1","reason":"wrong","evidence":"a.rs:1"}"#).is_empty());
    assert!(parse(r#"{"id":"F-1","reason":"wrong","evidence":[" "]}"#).is_empty());
    assert!(parse(r#"{"id":"F-1","reason":"because","evidence":["a.rs:1"]}"#).is_empty());
    assert!(parse(r#"{"reason":"wrong","evidence":["a.rs:1"]}"#).is_empty());
    assert!(parse(r#"{"id":"F-1","rebuttal":"r","evidence":["a.rs:1"]}"#).is_empty());
    assert_eq!(
        parse(r#"{"id":"F-1","reason":"wrong","evidence":["a.rs:1"]}"#).len(),
        1
    );
}

#[test]
fn malformed_declines_are_dropped_and_the_report_still_parses() {
    let parse = |declines: &str| {
        parse_report(&format!(
            "```sushi-report\n{{\"outcome\":\"complete\",\"summary\":\"s\",\"declines\":{declines}}}\n```"
        ))
        .unwrap()
    };
    for not_an_array in ["\"nope\"", "{\"id\":\"F-1\"}", "null", "3"] {
        let report = parse(not_an_array);
        assert!(report.declines.is_empty(), "{not_an_array}");
        assert_eq!(report.summary, "s");
    }
    let report = parse(
        "[1, \"x\", {\"id\":\"\",\"reason\":\"wrong\",\"evidence\":[\"a\"]}, {\"id\":\"F-3\",\"reason\":\"wrong\",\"evidence\":[\"ok\"]}]",
    );
    assert_eq!(report.declines.len(), 1);
    assert_eq!(report.declines[0].id, "F-3");
}
