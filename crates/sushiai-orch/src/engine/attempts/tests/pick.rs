use super::parse_pick;

#[test]
fn parse_pick_reads_the_fenced_block_and_rejects_other_answers() {
    let text = "x\n```sushi-pick\n{\"pick\":\"B\",\"why\":\" tidier \"}\n```";
    assert_eq!(parse_pick(text), Some((true, "tidier".to_string())));
    assert_eq!(
        parse_pick("```sushi-pick\n{\"pick\":\"a\"}\n```"),
        Some((false, String::new()))
    );
    assert_eq!(parse_pick("```sushi-pick\n{\"pick\":\"c\"}\n```"), None);
    assert_eq!(parse_pick("no block"), None);
}
