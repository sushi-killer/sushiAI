use crate::engine::chat::api::{handle_get, handle_list};
use crate::engine::chat::reply::apply_reply;
use crate::engine::chat::sessions::{push, ChatSession};
use crate::engine::chat::store::load;
use crate::engine::chat::types::{ChatMessage, ChatMode};
use crate::policy::routing::Harness;
use serde_json::json;

use super::keep_tools;
use crate::engine::chat::test_support::{converse, stored_task, test_app, tool, tools_of};

#[tokio::test]
async fn summaries_carry_timestamps_and_counts_but_no_bodies() {
    let (app, _dir, repo) = test_app();
    let empty = handle_list(&app, json!({"repo": repo})).await.unwrap();
    assert!(empty["sessions"][0]["updatedAt"].as_i64().unwrap() > 0);
    converse(&app, &repo, "Hello there", "h");
    let list = handle_list(&app, json!({"repo": repo})).await.unwrap();
    let thread = handle_get(&app, json!({"repo": repo})).await.unwrap();
    let summary = &list["sessions"][0];
    assert_eq!(summary["title"], "Hello there");
    assert_eq!(summary["busy"], false);
    assert_eq!(summary["messageCount"], 2);
    assert_eq!(summary["updatedAt"], thread["messages"][1]["ts"]);
    assert!(summary.get("messages").is_none());
}

const CLAUDE_TASK_GET: &str = r##"{"type":"assistant","message":{"model":"claude-opus-5-5","id":"msg_011CfNVcnvh4Z3JFfM2rvWjU","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_018JzSDbZMnWxhb9CbN7KP8P","name":"mcp__sushiai-orchestrator__task_get","input":{"id":"eb6845cb-12d8-44a5-8e37-ad577410ae22"},"caller":{"type":"direct"}}],"container":null,"stop_reason":null,"stop_sequence":null,"stop_details":null,"usage":{"input_tokens":2,"cache_creation_input_tokens":1093,"cache_read_input_tokens":15516,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1093},"output_tokens":8,"service_tier":"standard","inference_geo":"not_available"},"input_transformations":[],"diagnostics":null,"context_management":null},"parent_tool_use_id":null,"session_id":"7c0af684-01e9-4264-8072-13b0dfee9284","uuid":"ecd1d7e9-6b54-438f-b970-222c42648ba8","timestamp":"2026-09-24T15:23:12.434Z","request_id":"req_011CfNVcnRg3KH9QGyJ5yp1U","wire_tool_inputs":{"toolu_018JzSDbZMnWxhb9CbN7KP8P":{"id":"eb6845cb-12d8-44a5-8e37-ad577410ae22"}},"tool_use_meta":[{"id":"toolu_018JzSDbZMnWxhb9CbN7KP8P","display_name":"Task Get","server_display_name":"sushiai-orchestrator"}]}"##;
const CLAUDE_SUBAGENT_CALL: &str = r##"{"type":"assistant","message":{"model":"claude-sonnet-5","id":"msg_011CfNbyzMp8S5EiSJ3qs9ji","type":"message","role":"assistant","content":[{"type":"tool_use","id":"toolu_01R45q2hpFap9LAz5gHbVb41","name":"Bash","input":{"command":"grep -n \"orchestrator\" src/WorkspacePanels.tsx src/PanelIcon.tsx 2>/dev/null; echo \"---\"; grep -rn \"orchestrator\" src/App.tsx 2>/dev/null; echo \"---Sidebar---\"; ls src | grep -i sidebar"},"caller":{"type":"direct"}}],"container":null,"stop_reason":null,"stop_sequence":null,"stop_details":null,"usage":{"input_tokens":2,"cache_creation_input_tokens":9011,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":9011,"ephemeral_1h_input_tokens":0},"output_tokens":4,"service_tier":"standard","inference_geo":"not_available"},"input_transformations":[],"diagnostics":null,"context_management":null},"parent_tool_use_id":"toolu_01Lh54Qd1nHYeUUnnqgVJV6t","session_id":"230c3739-45f1-44a7-b3c5-08d1d59ed818","uuid":"d7d7435e-872b-4a20-8a7d-73cdd1f5e806","timestamp":"2026-09-24T16:46:41.108Z","request_id":"req_011CfNbyywkXgqVNioPJzxr6","subagent_type":"Explore","task_description":"Find how to open orchestrator panel in UI"}"##;
// Real `cua_repl` pair from a task run's events.jsonl, kept as is.
const CODEX_CUA_STARTED: &str = r##"{"type":"item.started","item":{"id":"item_10","type":"mcp_tool_call","server":"cua_repl","tool":"js","arguments":{"code":"const app = await cua.getApp('sushiAI'); await app.getAXStateAndScreenshot()","title":"Inspect the sushiAI panel"},"result":null,"error":null,"status":"in_progress"}}"##;
const CODEX_CUA_COMPLETED: &str = r##"{"type":"item.completed","item":{"id":"item_10","type":"mcp_tool_call","server":"cua_repl","tool":"js","arguments":{"code":"const app = await cua.getApp('sushiAI'); await app.getAXStateAndScreenshot()","title":"Inspect the sushiAI panel"},"result":{"content":[{"type":"text","text":"Computer Use was not approved to use sushiAI"}],"_meta":{"codex/nodeReplExecutionDurationMs":9,"codex/toolSurface":{"app":{"appId":"local.sushiai.workspace","kind":"appId"},"kind":"computerUse"}},"structured_content":null},"error":null,"status":"failed"}}"##;
// Copies of the pair above where only server/tool/arguments were swapped
// for an orchd call: no real Codex call to sushiai-orchestrator exists.
const CODEX_ORCHD_STARTED: &str = r##"{"type":"item.started","item":{"id":"item_10","type":"mcp_tool_call","server":"sushiai-orchestrator","tool":"task_get","arguments":{"id":"eb6845cb-12d8-44a5-8e37-ad577410ae22"},"result":null,"error":null,"status":"in_progress"}}"##;
const CODEX_ORCHD_COMPLETED: &str = r##"{"type":"item.completed","item":{"id":"item_10","type":"mcp_tool_call","server":"sushiai-orchestrator","tool":"task_get","arguments":{"id":"eb6845cb-12d8-44a5-8e37-ad577410ae22"},"result":{"content":[{"type":"text","text":"Computer Use was not approved to use sushiAI"}],"_meta":{"codex/nodeReplExecutionDurationMs":9,"codex/toolSurface":{"app":{"appId":"local.sushiai.workspace","kind":"appId"},"kind":"computerUse"}},"structured_content":null},"error":null,"status":"failed"}}"##;
const TASK_ID: &str = "eb6845cb-12d8-44a5-8e37-ad577410ae22";

#[test]
fn a_real_claude_task_get_line_becomes_the_tasks_title_and_never_its_id() {
    let (app, _dir, repo) = test_app();
    app.store
        .save_task(&stored_task(TASK_ID, "Keep the tool calls"))
        .unwrap();
    let tools = tools_of(&app, &repo, Harness::Claude, &[CLAUDE_TASK_GET]);
    assert_eq!(tools, vec![tool("task_get", "Keep the tool calls")]);

    let mut session = load(&app, &repo).current().clone();
    push(&mut session, "user", "status?", ChatMode::Chat);
    apply_reply(
        &mut session,
        "It is running.",
        tools,
        ChatMode::Chat,
        &|_, _| false,
    );
    let message = serde_json::to_value(session.messages.last().unwrap()).unwrap();
    assert_eq!(
        message["tools"],
        json!([{"name": "task_get", "summary": "Keep the tool calls"}])
    );
    assert!(!message.to_string().contains(TASK_ID));
}

#[test]
fn task_stop_and_task_delete_summaries_are_the_task_title() {
    let (app, _dir, repo) = test_app();
    app.store
        .save_task(&stored_task(TASK_ID, "Stop or delete me"))
        .unwrap();
    for name in ["task_stop", "task_delete"] {
        let line = json!({"type": "assistant", "parent_tool_use_id": null,
            "message": {"content": [{"type": "tool_use",
                "name": format!("mcp__sushiai-orchestrator__{name}"),
                "input": {"id": TASK_ID}}]}})
        .to_string();
        assert_eq!(
            tools_of(&app, &repo, Harness::Claude, &[&line]),
            vec![tool(name, "Stop or delete me")]
        );
    }
}

#[test]
fn an_unknown_task_id_gets_an_empty_summary_and_task_create_its_title() {
    let (app, _dir, repo) = test_app();
    let tools = tools_of(&app, &repo, Harness::Claude, &[CLAUDE_TASK_GET]);
    assert_eq!(tools, vec![tool("task_get", "")]);
    let create = json!({"type": "assistant", "parent_tool_use_id": null, "message": {"content": [
        {"type": "tool_use", "name": "mcp__sushiai-orchestrator__task_create",
         "input": {"title": "Add a button", "goal": "g"}}
    ]}})
    .to_string();
    assert_eq!(
        tools_of(&app, &repo, Harness::Claude, &[&create]),
        vec![tool("task_create", "Add a button")]
    );
}

#[test]
fn built_in_tools_keep_their_names_and_show_repo_relative_paths_or_patterns() {
    let (app, _dir, repo) = test_app();
    let line = json!({"type": "assistant", "parent_tool_use_id": null, "message": {"content": [
        {"type": "tool_use", "name": "Read", "input": {"file_path": format!("{repo}/src/a.rs")}},
        {"type": "tool_use", "name": "Grep", "input": {"pattern": "fn main"}},
        {"type": "tool_use", "name": "Glob", "input": {"pattern": "**/*.rs"}},
        {"type": "tool_use", "name": "mcp__sushiai-orchestrator__task_list", "input": {}}
    ]}})
    .to_string();
    assert_eq!(
        tools_of(&app, &repo, Harness::Claude, &[&line]),
        vec![
            tool("Read", "src/a.rs"),
            tool("Grep", "fn main"),
            tool("Glob", "**/*.rs"),
            tool("task_list", ""),
        ]
    );
}

#[test]
fn a_subagent_call_is_not_recorded() {
    let (app, _dir, repo) = test_app();
    assert!(CLAUDE_SUBAGENT_CALL.contains("\"parent_tool_use_id\":\"toolu"));
    assert!(tools_of(&app, &repo, Harness::Claude, &[CLAUDE_SUBAGENT_CALL]).is_empty());
}

#[test]
fn a_codex_mcp_call_is_recorded_once_per_item_id() {
    let (app, _dir, repo) = test_app();
    app.store
        .save_task(&stored_task(TASK_ID, "Keep the tool calls"))
        .unwrap();
    assert_eq!(
        tools_of(
            &app,
            &repo,
            Harness::Codex,
            &[CODEX_CUA_STARTED, CODEX_CUA_COMPLETED]
        ),
        vec![tool("js", "")]
    );
    assert_eq!(
        tools_of(
            &app,
            &repo,
            Harness::Codex,
            &[CODEX_ORCHD_STARTED, CODEX_ORCHD_COMPLETED]
        ),
        vec![tool("task_get", "Keep the tool calls")]
    );
}

#[test]
fn a_turn_keeps_the_first_eight_calls_and_collapses_repeats() {
    let mut list = Vec::new();
    keep_tools(
        &mut list,
        vec![tool("a", ""), tool("a", ""), tool("b", ""), tool("a", "")],
    );
    assert_eq!(list, vec![tool("a", ""), tool("b", ""), tool("a", "")]);
    keep_tools(
        &mut list,
        (0..12).map(|i| tool("t", &i.to_string())).collect(),
    );
    assert_eq!(list.len(), 8);
    assert_eq!(list[3], tool("t", "0"));
    assert_eq!(list[7], tool("t", "4"));
}

#[test]
fn a_long_summary_is_cut_to_120_chars_with_an_ellipsis() {
    let (app, _dir, repo) = test_app();
    let line = json!({"type": "assistant", "parent_tool_use_id": null, "message": {"content": [
        {"type": "tool_use", "name": "mcp__sushiai-orchestrator__task_create",
         "input": {"title": "x".repeat(300)}}
    ]}})
    .to_string();
    let tools = tools_of(&app, &repo, Harness::Claude, &[&line]);
    assert_eq!(tools[0].summary.chars().count(), 120);
    assert!(tools[0].summary.ends_with('…'));
}

#[test]
fn messages_without_tools_serialize_and_load_as_before() {
    let old = r#"{"id":"m1","role":"assistant","text":"hi","ts":5}"#;
    let message: ChatMessage = serde_json::from_str(old).unwrap();
    assert!(message.tools.is_empty());
    assert_eq!(
        serde_json::to_value(&message).unwrap(),
        serde_json::from_str::<serde_json::Value>(old).unwrap()
    );

    let mut session = ChatSession::default();
    apply_reply(
        &mut session,
        "plain",
        Vec::new(),
        ChatMode::Chat,
        &|_, _| false,
    );
    let value = serde_json::to_value(session.messages.last().unwrap()).unwrap();
    assert!(value.get("tools").is_none());
}
