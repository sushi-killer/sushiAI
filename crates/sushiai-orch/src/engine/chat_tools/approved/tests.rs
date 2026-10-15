use super::*;

#[test]
fn the_send_turn_may_use_only_the_one_tool() {
    let argv = send_argv(Path::new("/tmp/c.json"), "mcp__x__y");
    let at = argv.iter().position(|a| a == "--allowedTools").unwrap();
    assert_eq!(argv[at + 1], "mcp__x__y");
    assert_eq!(
        argv[argv.iter().position(|a| a == "--tools").unwrap() + 1],
        ""
    );
}
