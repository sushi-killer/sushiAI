use crate::task::review::{DeclineRuling, Verdict};

use super::{finding_paths, parse_decline_rulings, parse_review, parse_review_with_rule};

#[test]
fn parse_review_accepts_a_fence_closed_on_the_json_line() {
    let r = parse_review("```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}```").unwrap();
    assert_eq!(r.verdict, Verdict::Pass);
}

#[test]
fn parse_review_accepts_bare_json_with_object_findings() {
    // Captured Codex reply: no fence, no tag, findings as objects.
    let text = r#"{"verdict":"FAIL","findings":[{"severity":"P1","file":"ui-evidence","issue":"Required UI evidence is missing"}]}"#;
    let r = parse_review(text).unwrap();
    assert_eq!(r.verdict, Verdict::Fail);
    assert!(r.findings[0].contains("Required UI evidence is missing"));
    assert!(parse_review("Looks fine to me.").is_none());
}

#[test]
fn parse_review_accepts_the_xml_tag_shape_codex_replies_with() {
    let text = "<sushi-review>\n{\"verdict\":\"FAIL\",\"findings\":[\"x\"]}\n</sushi-review>";
    let r = parse_review(text).unwrap();
    assert_eq!(r.verdict, Verdict::Fail);
}

#[test]
fn a_repeat_mark_is_not_part_of_the_text_and_is_not_read() {
    let text = "```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"P1: plain string\",{\"finding\":\"P1: a\",\"repeat\":true}]}\n```";
    let r = parse_review(text).unwrap();
    assert_eq!(r.findings, vec!["P1: plain string", "P1: a"]);
    assert!(r.repeated.is_empty());
}

#[test]
fn parse_review_keeps_kind_scenario_and_id_out_of_the_finding_text() {
    let text = "```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[{\"finding\":\"P1: a\",\"id\":\"F-123456\",\"kind\":\"Style\",\"scenario\":{\"run\":\"cargo test a\",\"fails\":\"red\"}},\"P1: plain\"]}\n```";
    let r = parse_review(text).unwrap();
    assert_eq!(
        r.findings,
        vec!["P1: a".to_string(), "P1: plain".to_string()]
    );
    assert_eq!(r.details[0].id.as_deref(), Some("F-123456"));
    assert_eq!(r.details[0].caused_by_diff, None);
    assert_eq!(r.details[0].kind.as_deref(), Some("style"));
    assert_eq!(r.details[0].scenario.as_ref().unwrap().run, "cargo test a");
    assert_eq!(r.details[1], Default::default());
}

#[test]
fn parse_review_keeps_every_criterion_ruling_as_data() {
    let text = "```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[],\"criteria\":[{\"criterion\":\"A works\",\"met\":true,\"evidence\":\"read a.rs\"},{\"criterion\":\"B works\",\"met\":false},{\"criterion\":\"C works\",\"met\":null,\"evidence\":\"  \"}]}\n```";
    let (r, recorded_as_pass) = parse_review_with_rule(text, &[]).unwrap();
    assert_eq!(
        r.criteria,
        vec![
            crate::task::review::CriterionRuling {
                criterion: "A works".into(),
                index: None,
                met: Some(true),
                evidence: Some("read a.rs".into()),
            },
            crate::task::review::CriterionRuling {
                criterion: "B works".into(),
                index: None,
                met: Some(false),
                evidence: None,
            },
            crate::task::review::CriterionRuling {
                criterion: "C works".into(),
                index: None,
                met: None,
                evidence: None,
            },
        ]
    );
    // The verdict and findings are what they were before rulings were kept.
    assert!(!recorded_as_pass);
    assert_eq!(r.verdict, crate::task::review::Verdict::Fail);
    assert_eq!(
        r.findings,
        vec!["Unmet criterion: B works", "Not checked by review: C works"]
    );
    let json = serde_json::to_value(&r).unwrap();
    assert!(json["criteria"][2]["met"].is_null());
    assert!(json["criteria"][1].get("evidence").is_none());
    let plain =
        parse_review("```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```").unwrap();
    assert!(serde_json::to_value(&plain)
        .unwrap()
        .get("criteria")
        .is_none());
}

#[test]
fn a_ruling_names_its_criterion_by_index_and_a_prefix_stays_distinct() {
    let criteria = vec![
        "Beta".to_string(),
        "Beta works -- check: read it".to_string(),
    ];
    let text = "```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[],\"criteria\":[{\"criterion\":1,\"met\":true},{\"criterion\":0,\"met\":false,\"evidence\":\"missing\"}]}\n```";
    let (r, _) = parse_review_with_rule(text, &criteria).unwrap();
    let rulings: Vec<(Option<usize>, Option<bool>)> =
        r.criteria.iter().map(|c| (c.index, c.met)).collect();
    assert_eq!(rulings, [(Some(1), Some(true)), (Some(0), Some(false))]);
    assert_eq!(r.criteria[0].criterion, "Beta works -- check: read it");
    assert_eq!(r.findings, vec!["Unmet criterion: Beta (missing)"]);
    assert_eq!(r.verdict, Verdict::Fail);
}

#[test]
fn a_ruling_without_a_known_index_is_unmet_unknown_and_settles_nothing() {
    let criteria = vec!["Alpha".to_string()];
    for named in ["\"Alpha\"", "7", "null"] {
        let text = format!("```sushi-review\n{{\"verdict\":\"PASS\",\"criteria\":[{{\"criterion\":{named},\"met\":false}}]}}\n```");
        let (r, _) = parse_review_with_rule(&text, &criteria).unwrap();
        assert_eq!(r.criteria[0].index, None, "{named}");
        assert_eq!(r.verdict, Verdict::Fail, "{named}");
        assert_eq!(r.findings.len(), 1, "{named}");
    }
    let met = "```sushi-review\n{\"verdict\":\"PASS\",\"criteria\":[{\"criterion\":\"Alpha\",\"met\":true}]}\n```";
    let (r, _) = parse_review_with_rule(met, &criteria).unwrap();
    assert_eq!(r.criteria[0].index, None);
}

#[test]
fn a_finding_object_carries_its_id_and_caused_by_diff() {
    let text = "```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[{\"finding\":\"P1: a\",\"id\":\"F-3\",\"causedByDiff\":false},{\"finding\":\"P1: b\",\"id\":null,\"causedByDiff\":true},\"P1: old format\"]}\n```";
    let r = parse_review(text).unwrap();
    assert_eq!(r.findings, vec!["P1: a", "P1: b", "P1: old format"]);
    assert_eq!(r.details[0].id.as_deref(), Some("F-3"));
    assert_eq!(r.details[0].caused_by_diff, Some(false));
    assert_eq!(r.details[1].id, None);
    assert_eq!(r.details[1].caused_by_diff, Some(true));
    assert_eq!(r.details[2], Default::default());
}

#[test]
fn parse_review_reads_verdict_and_findings() {
    let text = "```sushi-review\n{\"verdict\":\"FAIL\",\"findings\":[\"missing test\"]}\n```";
    let review = parse_review(text).unwrap();
    assert_eq!(review.verdict, Verdict::Fail);
    assert_eq!(review.findings, vec!["missing test".to_string()]);
}

fn review_reply(verdict: &str, findings: &str, criteria: &str) -> String {
    format!("```sushi-review\n{{\"verdict\":\"{verdict}\",\"findings\":{findings}{criteria}}}\n```")
}

// The exact reply of task 50a52492, run 6: the object is closed right after
// `findings`, and `criteria` follows a premature `}`.
const PREMATURE_CLOSE: &str =
    include_str!("../../../../tests/fixtures/review-run6-premature-close.txt");

#[test]
fn a_criteria_block_after_a_premature_close_is_spliced_into_the_verdict() {
    let (r, flag) = parse_review_with_rule(PREMATURE_CLOSE, &[]).unwrap();
    assert_eq!(r.verdict, Verdict::Fail);
    assert!(!flag);
    assert!(r.findings.iter().any(|f| f.starts_with(
        "Unmet criterion: Before and after screenshots show the same busy Code layout"
    )));
    assert!(r.findings[0].starts_with("P1: artifacts/restore-after.png"));
}

#[test]
fn a_verdict_is_read_from_a_json_fence() {
    let r = parse_review(
        "All good.\n```json\n{\"verdict\":\"FAIL\",\"findings\":[\"P1: a - b\"]}\n```\nBye",
    )
    .unwrap();
    assert_eq!(r.verdict, Verdict::Fail);
    assert_eq!(r.findings, vec!["P1: a - b".to_string()]);
}

#[test]
fn a_verdict_is_read_from_an_unlabelled_fence() {
    let r = parse_review("Verdict:\n```\n{\"verdict\":\"PASS\",\"findings\":[]}\n```\n").unwrap();
    assert_eq!(r.verdict, Verdict::Pass);
}

#[test]
fn a_verdict_is_read_from_the_last_bare_object_in_prose() {
    let text = "I checked {\"unrelated\": \"thing\"} first, then decided: {\"verdict\":\"FAIL\",\"findings\":[\"P0: x\"]} and that is it.";
    let r = parse_review(text).unwrap();
    assert_eq!(r.verdict, Verdict::Fail);
    assert_eq!(r.findings, vec!["P0: x".to_string()]);
}

#[test]
fn a_missing_verdict_is_derived_from_the_criteria() {
    let pass =
        parse_review(r#"{"criteria":[{"criterion":"a","met":true},{"criterion":"b","met":null}]}"#)
            .unwrap();
    assert_eq!(pass.verdict, Verdict::Pass);
    let fail = parse_review(
            r#"```json
{"findings":[],"criteria":[{"criterion":"a","met":true},{"criterion":"b","met":"no","evidence":"why"}]}
```"#,
        )
        .unwrap();
    assert_eq!(fail.verdict, Verdict::Fail);
    assert!(fail
        .findings
        .contains(&"Unmet criterion: b (why)".to_string()));
}

#[test]
fn a_reply_with_no_verdict_shape_is_rejected() {
    for text in [
        "Looks fine to me.",
        r#"{"summary":"fine","findings":[]}"#,
        r#"{"criteria":[{"criterion":"a","met":null}]}"#,
        r#"{"criteria":[]}"#,
        r#"{"verdict":"MAYBE","findings":[]}"#,
        "```sushi-review\nnot json\n```",
    ] {
        assert!(parse_review(text).is_none(), "{text}");
    }
}

#[test]
fn review_rulings_are_read_by_id_and_a_malformed_list_rules_on_nothing() {
    let reply = |rulings: &str| {
        format!(
            "```sushi-review\n{{\"verdict\":\"FAIL\",\"findings\":[],\"declineRulings\":{rulings}}}\n```"
        )
    };
    assert_eq!(
        parse_decline_rulings(&reply(
            r#"[{"id":"F-1","upheld":true},{"id":"F-2","upheld":false}]"#
        )),
        [
            DeclineRuling {
                id: "F-1".into(),
                upheld: true
            },
            DeclineRuling {
                id: "F-2".into(),
                upheld: false
            },
        ]
    );
    assert!(parse_decline_rulings(&reply(r#"[{"id":"F-1"}]"#)).is_empty());
    assert!(parse_decline_rulings(&reply("null")).is_empty());
    assert!(parse_decline_rulings("no review here").is_empty());
}

#[test]
fn finding_paths_reads_path_line_tokens() {
    let findings = vec![
        "P1: src/a.rs:12 - missing check".to_string(),
        "P2: `./src/b.rs:3-9` - odd".to_string(),
        "P1: \"web/c.ts:7\", and src/a.rs:40:".to_string(),
        "P2: no path here, v1.2 and 10:30 are not paths".to_string(),
        "Unmet criterion: Beta is done (missing)".to_string(),
    ];
    assert_eq!(
        finding_paths(&findings),
        vec!["src/a.rs", "src/b.rs", "web/c.ts"]
    );
}

#[test]
fn finding_paths_needs_an_exact_path() {
    assert!(finding_paths(&["P1: a.rs - no line".to_string()]).is_empty());
    assert!(finding_paths(&["P1: :12 - nothing".to_string()]).is_empty());
}

#[test]
fn a_fail_with_only_p2_p3_findings_is_recorded_as_pass() {
    let text = review_reply(
        "FAIL",
        r#"["P2: a.rs:3 - naming","[P3] typo",{"severity":"P3","issue":"nit"}]"#,
        "",
    );
    let (r, flag) = parse_review_with_rule(&text, &[]).unwrap();
    assert_eq!(r.verdict, Verdict::Pass);
    assert!(flag);
    assert_eq!(r.findings.len(), 3);
    assert_eq!(parse_review(&text).unwrap().verdict, Verdict::Pass);
}

#[test]
fn a_fail_stays_fail_unless_every_own_finding_is_p2_or_p3() {
    for findings in [
        r#"["P1: a.rs:1 - bug","P3: nit"]"#,
        r#"["P2: fine","unlabelled"]"#,
        r#"["P2x: not a severity"]"#,
        "[]",
    ] {
        let (r, flag) = parse_review_with_rule(&review_reply("FAIL", findings, ""), &[]).unwrap();
        assert_eq!(r.verdict, Verdict::Fail, "{findings}");
        assert!(!flag);
    }
    let unmet = r#","criteria":[{"criterion":"c","met":false}]"#;
    let (r, flag) =
        parse_review_with_rule(&review_reply("FAIL", r#"["P2: nit"]"#, unmet), &[]).unwrap();
    assert_eq!(r.verdict, Verdict::Fail);
    assert!(!flag);
    // Findings parse_review adds itself are not the reviewer's own.
    let unchecked = r#","criteria":[{"criterion":"c","met":null}]"#;
    let (r, flag) = parse_review_with_rule(&review_reply("FAIL", "[]", unchecked), &[]).unwrap();
    assert_eq!(r.verdict, Verdict::Fail);
    assert!(!flag);
}
