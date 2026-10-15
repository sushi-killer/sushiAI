use crate::engine::chat::proposals::ProposalDep;
use crate::engine::chat::types::ChatQuestion;
use crate::policy::routing::Tier;

use super::parse_reply;
use crate::engine::chat::test_support::block;

#[test]
fn the_last_draft_block_wins_and_prose_around_it_stays() {
    let reply = format!(
        "Before.\n\n{}\n\nMiddle.\n\n{}\n\nAfter.",
        block(r#"{"title":"First"}"#),
        block(r#"{"title":"Second","goal":"g","criteria":["a"],"dependsOn":["t1"],"tier":"hard"}"#)
    );
    let parsed = parse_reply(&reply);
    let draft = parsed.draft.unwrap();
    assert_eq!(draft.title, "Second");
    assert_eq!(draft.goal, "g");
    assert_eq!(draft.criteria, vec!["a"]);
    assert_eq!(draft.depends_on, vec!["t1"]);
    assert_eq!(draft.tier, Some(Tier::Hard));
    assert!(parsed.text.starts_with("Before."));
    assert!(parsed.text.contains("Middle."));
    assert!(parsed.text.ends_with("After."));
    assert!(
        parsed.text.contains("sushi-draft"),
        "the earlier block stays"
    );
    assert!(!parsed.text.contains("Second"));
}

#[test]
fn stripping_covers_a_fence_closed_on_the_json_line_and_the_tag_form() {
    let inline = parse_reply("Intro.\n```sushi-draft\n{\"title\":\"T\"}```\nOutro.");
    assert_eq!(inline.text, "Intro.\n\nOutro.");
    let tagged = parse_reply("Intro. <sushi-draft>{\"title\":\"T\"}</sushi-draft> Outro.");
    assert_eq!(tagged.text, "Intro.\n\nOutro.");
    assert_eq!(tagged.draft.unwrap().title, "T");
}

#[test]
fn a_malformed_or_empty_block_is_no_block() {
    for reply in [
        block("{not json"),
        block(r#"{"title":"  ","goal":"g","questions":[{"text":""}]}"#),
        block("[]"),
    ] {
        let parsed = parse_reply(&format!("Hi\n{reply}"));
        assert_eq!(parsed.text, format!("Hi\n{reply}"));
        assert!(parsed.draft.is_none());
        assert!(parsed.questions.is_empty());
    }
}

#[test]
fn an_unknown_tier_is_none_and_text_criteria_read_as_text() {
    let parsed = parse_reply(&block(
        r#"{"title":"T","tier":"huge","criteria":[{"text":"c1"},"c2",5]}"#,
    ));
    let draft = parsed.draft.unwrap();
    assert_eq!(draft.tier, None);
    assert_eq!(draft.criteria, vec!["c1", "c2"]);
}

#[test]
fn a_questions_only_block_has_no_draft_and_empty_questions_are_dropped() {
    let parsed = parse_reply(&format!(
        "Which one?\n{}",
        block(r#"{"questions":[{"text":"A or B?","options":["A","B"]},{"text":" "}]}"#)
    ));
    assert!(parsed.draft.is_none());
    assert_eq!(parsed.text, "Which one?");
    assert_eq!(
        parsed.questions,
        vec![ChatQuestion {
            text: "A or B?".into(),
            options: vec!["A".into(), "B".into()]
        }]
    );
}

#[test]
fn a_block_only_reply_reads_as_the_title_else_the_first_question() {
    let titled = parse_reply(&block(r#"{"title":"Do it","questions":[{"text":"Q?"}]}"#));
    assert_eq!(titled.text, "Do it");
    let asked = parse_reply(&block(r#"{"questions":[{"text":"Q1?"},{"text":"Q2?"}]}"#));
    assert_eq!(asked.text, "Q1?");
}

#[test]
fn a_task_list_reads_index_and_id_dependencies_and_a_bad_list_is_dropped() {
    let ok = parse_reply(&block(
        r#"{"tasks":[{"title":"A"},{"title":"B","dependsOn":[1,"abc-def","1"," 1 "],"tier":"hard"}]}"#,
    ));
    let proposal = ok.proposal.unwrap();
    assert_eq!(ok.text, "2 proposed tasks");
    assert_eq!(
        proposal.tasks[1].depends_on,
        vec![ProposalDep::Index(1), ProposalDep::Id("abc-def".into())]
    );
    assert_eq!(proposal.tasks[1].tier, Some(Tier::Hard));
    for bad in [
        r#"{"tasks":[]}"#,
        r#"{"tasks":"soon"}"#,
        r#"{"tasks":[{"goal":"no title"}]}"#,
        r#"{"tasks":[{"title":"A","dependsOn":[2]}]}"#,
        r#"{"tasks":[{"title":"A","dependsOn":[1]}]}"#,
        r#"{"tasks":[{"title":"A","dependsOn":[2]},{"title":"B","dependsOn":[1]}]}"#,
    ] {
        let parsed = parse_reply(&format!("Words.\n{}", block(bad)));
        assert!(parsed.proposal.is_none(), "{bad}");
        assert!(parsed.text.starts_with("Words."), "{bad}");
    }
    // A bad list beside a question keeps the question and the prose.
    let mixed = parse_reply(&format!(
        "Words.\n{}",
        block(r#"{"tasks":"x","questions":[{"text":"Q?","options":["a","b"]}]}"#)
    ));
    assert!(mixed.proposal.is_none());
    assert_eq!(mixed.text, "Words.");
    assert_eq!(mixed.questions.len(), 1);
}
