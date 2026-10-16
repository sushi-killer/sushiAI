use sushiai_orch::prompts::evolution::proposal::ProposalBriefInput;
use sushiai_orch::prompts::evolution::proposal::ProposalExcerpt;

pub(super) fn renders() -> Vec<(&'static str, String)> {
    let excerpts = [
        ProposalExcerpt {
            task_id: "t-a".into(),
            file: "events.jsonl".into(),
            from_line: 12,
            lines: vec![
                r#"{"type":"tool","name":"Bash","input":"npm test"}"#.into(),
                "a line with ``` inside".into(),
            ],
        },
        ProposalExcerpt {
            task_id: "t-b".into(),
            file: "events.jsonl".into(),
            from_line: 3,
            lines: vec![r#"{"type":"tool","name":"Bash","input":"npm test"}"#.into()],
        },
    ];
    let proposal = ProposalBriefInput {
        kind: "repeated_full_suite",
        detail: "npm test ran the whole suite",
        repo: "/repo",
        occurrences: 4,
        tasks: 2,
        wasted_calls: 7,
        wasted_usd: 1.5,
        excerpts: &excerpts,
    };
    let proposal_empty = ProposalBriefInput {
        excerpts: &[],
        ..proposal
    };
    vec![
        (
            "build_proposal_brief",
            sushiai_orch::prompts::evolution::proposal::build_proposal_brief(&proposal),
        ),
        (
            "build_proposal_brief_no_excerpts",
            sushiai_orch::prompts::evolution::proposal::build_proposal_brief(&proposal_empty),
        ),
    ]
}
