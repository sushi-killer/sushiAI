use crate::engine::permissions::answers::permission_question;
use crate::engine::permissions::capabilities::missing_tool_claim;
use crate::engine::permissions::capabilities::tool_to_enable;
use crate::policy::permissions::is_protected;
use crate::policy::permissions::normalized;
use crate::policy::permissions::parse_choice;
use crate::policy::permissions::rule_covers;
use crate::policy::permissions::Choice;
use crate::policy::permissions::Subject;
use crate::policy::permissions::ALLOW_ALWAYS;
use crate::policy::permissions::ALLOW_ONCE;
use crate::policy::permissions::DENY;
use crate::task::QuestionKind;
use crate::task::TaskStatus;
use serde_json::json;

#[test]
fn a_directory_rule_covers_what_is_under_it_and_nothing_else() {
    assert!(rule_covers("path:.claude/**", "path:.claude/a/b.md"));
    assert!(rule_covers("tool:x", "tool:x"));
    assert!(!rule_covers("path:.claude/**", "path:.claudex/a"));
    assert!(!rule_covers("path:.claude/a.md", "path:.claude/b.md"));
}

#[test]
fn paths_are_normalized_before_the_protected_check() {
    assert!(is_protected(&normalized("./a/../.claude/x.md")));
    assert!(is_protected(&normalized(".claude")));
    assert!(!is_protected(&normalized("docs/.claude/x.md")));
    assert!(!is_protected(&normalized(".claudeish/x")));
}

#[test]
fn answers_are_read_as_the_three_options_or_a_denial_with_a_note() {
    assert_eq!(parse_choice("Allow once"), Choice::Once);
    assert_eq!(parse_choice(" always for this repo. "), Choice::Always);
    assert_eq!(parse_choice("Deny"), Choice::Deny(None));
    assert_eq!(
        parse_choice("not that one"),
        Choice::Deny(Some("not that one".to_string()))
    );
}

#[test]
fn the_question_names_the_tool_or_path() {
    let q = permission_question(&[Subject::Path(".claude/x.md".into())], "");
    assert_eq!(q.text, "Allow .claude/x.md for this task?");
    assert_eq!(q.options, vec![ALLOW_ONCE, ALLOW_ALWAYS, DENY]);
    assert_eq!(q.kind, QuestionKind::Permission);
}

#[test]
fn only_a_connected_tool_that_is_off_can_be_offered_for_a_no_tool_claim() {
    let (app, _dir) = crate::engine::test_support::test_app();
    let task = crate::engine::test_support::task_with_status(TaskStatus::Running);
    assert_eq!(tool_to_enable(&app, &task, "move an Asana task"), None);
    app.settings.write().unwrap().chat_tools = vec![serde_json::from_value(json!({
        "id": "asana", "label": "Asana", "enabled": false,
        "server": {"type": "http", "url": "https://example.invalid/mcp"}
    }))
    .unwrap()];
    assert_eq!(
        tool_to_enable(&app, &task, "move an Asana task").as_deref(),
        Some("asana")
    );
    assert_eq!(tool_to_enable(&app, &task, "send an email"), None);
}

#[test]
fn a_no_tool_claim_needs_its_prefix_and_a_capability() {
    assert_eq!(
        missing_tool_claim("No tool: move an Asana task").as_deref(),
        Some("move an Asana task")
    );
    assert_eq!(missing_tool_claim("no tool:  x ").as_deref(), Some("x"));
    assert_eq!(missing_tool_claim("No tool:"), None);
    assert_eq!(missing_tool_claim("Which database?"), None);
    assert_eq!(missing_tool_claim("é"), None);
}
