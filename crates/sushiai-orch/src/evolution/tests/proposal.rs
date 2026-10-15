use super::{
    build_proposal_brief, parse_proposal, ProposalBriefInput, ProposalExcerpt, ProposalParseError,
};

fn proposal_reply(body: serde_json::Value) -> String {
    format!("Done.\n\n```sushi-proposal\n{body}\n```\n")
}

fn valid_proposal() -> serde_json::Value {
    serde_json::json!({
        "track": "Repo", "form": "SCRIPT",
        "change": "add scripts/check.sh", "evidence": "occurrence 1",
        "metric": "wasted calls", "test": "run it",
    })
}

#[test]
fn parse_proposal_reads_the_block_and_folds_case() {
    let reply = parse_proposal(&proposal_reply(valid_proposal())).unwrap();
    assert_eq!(reply.track, crate::task::evolution::Track::Repo);
    assert_eq!(reply.form, crate::task::evolution::ProposalForm::Script);
    assert!(reply.arm.is_none());
    assert_eq!(
        parse_proposal("I found nothing to propose."),
        Err(ProposalParseError::NoBlock)
    );
}

#[test]
fn parse_proposal_rejects_a_block_that_breaks_the_contract() {
    type BreakIt = Box<dyn Fn(&mut serde_json::Value)>;
    let cases: Vec<(&str, BreakIt)> = vec![
        (
            "no change",
            Box::new(|v| drop(v.as_object_mut().unwrap().remove("change"))),
        ),
        (
            "blank test",
            Box::new(|v| v["test"] = serde_json::json!(" ")),
        ),
        (
            "unknown track",
            Box::new(|v| v["track"] = serde_json::json!("both")),
        ),
        (
            "unknown form",
            Box::new(|v| v["form"] = serde_json::json!("magic")),
        ),
        (
            "arm string",
            Box::new(|v| v["arm"] = serde_json::json!("x")),
        ),
    ];
    for (name, break_it) in cases {
        let mut v = valid_proposal();
        break_it(&mut v);
        assert!(
            matches!(
                parse_proposal(&proposal_reply(v)),
                Err(ProposalParseError::Invalid(_))
            ),
            "{name} was accepted"
        );
    }
    let mut v = valid_proposal();
    v["arm"] = serde_json::json!({"flag": true});
    assert!(parse_proposal(&proposal_reply(v)).unwrap().arm.is_some());
}

#[test]
fn proposal_brief_quotes_raw_lines_and_forbids_weakening() {
    let excerpts: Vec<ProposalExcerpt> = (0..7)
        .map(|i| ProposalExcerpt {
            task_id: format!("task-{i}"),
            file: "runs/events.jsonl".into(),
            from_line: 10,
            lines: (0..50)
                .map(|n| format!("{{\"raw\":{n},\"t\":\"```\"}}"))
                .collect(),
        })
        .collect();
    let brief = build_proposal_brief(&ProposalBriefInput {
        kind: "loop",
        detail: "same read",
        repo: "/repo",
        occurrences: 7,
        tasks: 7,
        wasted_calls: 30,
        wasted_usd: 1.5,
        excerpts: &excerpts,
    });
    assert!(brief.contains("may never weaken or remove a check, gate or protected path"));
    assert!(brief.contains("```sushi-proposal"));
    assert_eq!(brief.matches("### Occurrence").count(), 5);
    assert_eq!(brief.matches("{\"raw\":0,").count(), 5);
    assert_eq!(brief.matches("{\"raw\":39,").count(), 5);
    assert_eq!(brief.matches("{\"raw\":40,").count(), 0);
}
