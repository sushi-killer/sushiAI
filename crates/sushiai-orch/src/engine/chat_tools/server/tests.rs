use super::*;

#[test]
fn the_test_profile_fixture_lists_only_example_servers() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../electron/test-fixtures/mcp-home");
    let listed = available_json(&Settings::default(), &fixture);
    let refs: Vec<&str> = listed["servers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["ref"].as_str())
        .collect();
    assert_eq!(
        refs,
        ["claude-json:example-notes", "claude-json:example-tracker"]
    );
}

#[test]
fn a_plugin_server_keeps_claude_codes_name() {
    let cfg = &ChatToolConfig {
        id: "slack".into(),
        label: "Slack".into(),
        enabled: true,
        server: serde_json::json!({"ref": "plugin:slack/slack"}),
        overrides: Default::default(),
    };
    assert_eq!(server_key(cfg), "plugin:slack:slack");
    assert_eq!(
        claude_tool_name(&server_key(cfg), "search"),
        "mcp__plugin_slack_slack__search"
    );
}
