use super::{audit_rubric, build_audit_brief, parse_audit, AuditParseError};

#[test]
fn audit_brief_carries_the_rubric_verbatim_and_asks_for_the_sushi_audit_fence() {
    let brief = build_audit_brief();
    assert!(brief.contains(audit_rubric()));
    for area in [
        "1. Instructions:",
        "2. Build and test commands:",
        "3. Test health:",
        "4. Legibility:",
        "5. Context hygiene:",
        "6. Safety:",
        "7. Verification surfaces for a reviewer:",
    ] {
        assert!(brief.contains(area), "{area}");
    }
    assert!(brief.contains("```sushi-audit"));
    assert!(brief.contains("unmeasured"));
    assert!(brief.contains("read-only"));
}

#[test]
fn parse_audit_reads_items_and_caps_top_fixes() {
    let text = "Done.\n\n```sushi-audit\n{\"summary\":\"ok\",\"items\":[{\"area\":\"1. Instructions\",\"grade\":\"Weak\",\"evidence\":[\"AGENTS.md:1\"],\"recommendation\":\"shorten it\",\"effort\":\"SMALL\"}],\"topFixes\":[\"a\",\"b\",\"c\",\"d\",\"e\",\"f\"]}\n```\n";
    let report = parse_audit(text).unwrap();
    assert_eq!(report.summary, "ok");
    assert_eq!(report.items.len(), 1);
    assert_eq!(report.items[0].grade, crate::task::audit::AuditGrade::Weak);
    assert_eq!(
        report.items[0].effort,
        crate::task::audit::AuditEffort::Small
    );
    assert_eq!(report.items[0].evidence, vec!["AGENTS.md:1".to_string()]);
    assert_eq!(report.top_fixes, vec!["a", "b", "c", "d", "e"]);
}

fn audit_reply(report: serde_json::Value) -> String {
    format!("Done.\n\n```sushi-audit\n{report}\n```\n")
}

fn valid_audit() -> serde_json::Value {
    serde_json::json!({
        "summary": "ok",
        "items": [{
            "area": "1. Instructions",
            "grade": "weak",
            "evidence": ["AGENTS.md:1"],
            "recommendation": "shorten it",
            "effort": "small",
        }],
        "topFixes": ["shorten it"],
    })
}

#[test]
fn parse_audit_returns_no_block_without_a_fence() {
    assert_eq!(
        parse_audit("I looked around; it is fine."),
        Err(AuditParseError::NoBlock)
    );
    assert!(parse_audit(&audit_reply(valid_audit())).is_ok());
}

#[test]
fn parse_audit_rejects_a_report_that_breaks_the_contract() {
    type BreakIt = Box<dyn Fn(&mut serde_json::Value)>;
    let cases: Vec<(&str, BreakIt)> = vec![
        ("not json", Box::new(|v| *v = serde_json::json!("{"))),
        (
            "no items",
            Box::new(|v| drop(v.as_object_mut().unwrap().remove("items"))),
        ),
        (
            "empty items",
            Box::new(|v| v["items"] = serde_json::json!([])),
        ),
        (
            "no summary",
            Box::new(|v| drop(v.as_object_mut().unwrap().remove("summary"))),
        ),
        (
            "blank summary",
            Box::new(|v| v["summary"] = serde_json::json!("  ")),
        ),
        (
            "no topFixes",
            Box::new(|v| drop(v.as_object_mut().unwrap().remove("topFixes"))),
        ),
        (
            "object topFix",
            Box::new(|v| v["topFixes"] = serde_json::json!([{"fix": "a"}])),
        ),
        (
            "blank topFix",
            Box::new(|v| v["topFixes"] = serde_json::json!([""])),
        ),
        (
            "unknown grade",
            Box::new(|v| v["items"][0]["grade"] = serde_json::json!("great")),
        ),
        (
            "unknown effort",
            Box::new(|v| v["items"][0]["effort"] = serde_json::json!("tiny")),
        ),
        (
            "no area",
            Box::new(|v| drop(v["items"][0].as_object_mut().unwrap().remove("area"))),
        ),
        (
            "blank area",
            Box::new(|v| v["items"][0]["area"] = serde_json::json!("")),
        ),
        (
            "no evidence",
            Box::new(|v| drop(v["items"][0].as_object_mut().unwrap().remove("evidence"))),
        ),
        (
            "empty evidence",
            Box::new(|v| v["items"][0]["evidence"] = serde_json::json!([])),
        ),
        (
            "blank evidence",
            Box::new(|v| v["items"][0]["evidence"] = serde_json::json!([" "])),
        ),
        (
            "object evidence",
            Box::new(|v| v["items"][0]["evidence"] = serde_json::json!([{"file": "a"}])),
        ),
        (
            "no recommendation",
            Box::new(|v| {
                drop(
                    v["items"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("recommendation"),
                )
            }),
        ),
        (
            "blank recommendation",
            Box::new(|v| v["items"][0]["recommendation"] = serde_json::json!("")),
        ),
    ];
    for (name, break_it) in cases {
        let mut report = valid_audit();
        break_it(&mut report);
        let text = match report.as_str() {
            Some(raw) => format!("```sushi-audit\n{raw}\n```"),
            None => audit_reply(report),
        };
        assert!(
            matches!(parse_audit(&text), Err(AuditParseError::Invalid(_))),
            "{name} was accepted"
        );
    }
}
