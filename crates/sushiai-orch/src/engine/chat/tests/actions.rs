use super::{take_action, ActionState};
use serde_json::json;

#[test]
fn an_action_keeps_prose_and_opaque_arguments_from_the_last_tagged_block() {
    let reply = concat!(
        "Before.\n```sushi-action\n{\"server\":\"old\",\"tool\":\"old\"}\n```\n\n",
        "<sushi-action>{\"server\":\"notes\",\"tool\":\"write\",",
        "\"args\":{\"text\":\"literal ```sushi-review```\"},\"summary\":\"Save note\"}</sushi-action>\nAfter."
    );
    let (text, action) = take_action(reply, &|server, tool| server == "notes" && tool == "write");
    assert!(text.contains("\"server\":\"old\""));
    assert!(text.ends_with("\n\nAfter."));
    let action = action.unwrap();
    assert_eq!(action.args, json!({"text": "literal ```sushi-review```"}));
    assert_eq!(action.state, ActionState::Pending);
    assert_eq!(action.summary, "Save note");
}

#[test]
fn a_malformed_last_action_or_a_refused_tool_leaves_the_reply_untouched() {
    for reply in [
        "```sushi-action\n{\"server\":\"notes\",\"tool\":\"write\"}\n```\n<sushi-action>broken</sushi-action>",
        "Words.\n```sushi-action\n{\"server\":\"notes\",\"tool\":\"read\"}```",
    ] {
        let (text, action) = take_action(reply, &|_, tool| tool == "write");
        assert_eq!(text, reply);
        assert!(action.is_none());
    }
}
