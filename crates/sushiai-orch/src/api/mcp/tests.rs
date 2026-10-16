use super::call::{handle_tools_call, orch_method_for};
use super::schema::{tool_specs, ORCHESTRATOR_TOOLS, READ_ONLY_TOOLS, TASK_TOOLS};
use super::scope::{message_params, with_task_mcp, Bridge};
use super::session::{dispatch, handle_line, PROTOCOL_VERSION};
use crate::task::messages::ORCHESTRATOR;
use serde_json::{json, Value};

use crate::engine::state::App;
use std::sync::Arc;

fn bridge(task: Option<&str>) -> Bridge {
    Bridge {
        // Never dialed by the branches under test here (`initialize`,
        // `ping`, `tools/list`, and the invalid-params/unknown-tool
        // exits out of `tools/call`); a bogus path just documents that.
        call: Box::new(|_, _| Err("no backend".to_string())),
        task: task.map(str::to_string),
        read_only: false,
    }
}

fn orchestrator() -> Bridge {
    bridge(None)
}

#[test]
fn initialize_echoes_a_requested_protocol_version_and_carries_the_role() {
    let params = json!({"protocolVersion": "2024-11-05"});
    let result = dispatch(&orchestrator(), "initialize", &params).unwrap();
    assert_eq!(result["protocolVersion"], "2024-11-05");
    assert_eq!(result["serverInfo"]["name"], "sushiai-orchestrator");
    assert!(!result["instructions"]
        .as_str()
        .unwrap()
        .contains("task_preflight"));
}

#[test]
fn initialize_defaults_the_protocol_version_when_absent() {
    let result = dispatch(&orchestrator(), "initialize", &json!({})).unwrap();
    assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
}

#[test]
fn ping_returns_an_empty_object() {
    let result = dispatch(&orchestrator(), "ping", &json!({})).unwrap();
    assert_eq!(result, json!({}));
}

#[test]
fn task_amend_schema_offers_full_dependency_replacement_before_implementing() {
    let result = dispatch(&orchestrator(), "tools/list", &json!({})).unwrap();
    let amend = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "task_amend")
        .unwrap();
    let dependencies = &amend["inputSchema"]["properties"]["dependsOn"];
    assert_eq!(dependencies["type"], "array");
    assert_eq!(dependencies["items"]["type"], "string");
    assert!(dependencies["description"]
        .as_str()
        .unwrap()
        .contains("full new list"));
    let description = amend["description"].as_str().unwrap();
    assert!(description.contains("dependsOn"));
    assert!(description.contains("first implement attempt"));
    assert!(description.contains("empty list"));
}

#[test]
fn task_amend_schema_offers_an_addendum_string() {
    let result = dispatch(&orchestrator(), "tools/list", &json!({})).unwrap();
    let amend = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "task_amend")
        .unwrap();
    assert_eq!(
        amend["inputSchema"]["properties"]["addendum"]["type"],
        "string"
    );
    assert!(amend["description"].as_str().unwrap().contains("addendum"));
}

#[test]
fn task_pr_never_forwards_a_branch() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sink = seen.clone();
    let bridge = Bridge {
        call: Box::new(move |method, args| {
            *sink.lock().unwrap() = Some((method.to_string(), args));
            Ok(json!({}))
        }),
        task: None,
        read_only: false,
    };
    let call =
        json!({"name": "task_pr", "arguments": {"id": "t1", "branch": "main", "title": "T"}});
    dispatch(&bridge, "tools/call", &call).unwrap();
    let (method, args) = seen.lock().unwrap().take().unwrap();
    assert_eq!(method, "task.pr");
    assert!(args.get("branch").is_none());
    assert_eq!(args["id"], "t1");
    assert_eq!(args["source"], "lead");
}

#[test]
fn task_pr_is_an_orchestrator_only_tool() {
    assert!(tool_specs()
        .iter()
        .any(|(name, method, ..)| *name == "task_pr" && *method == "task.pr"));
    assert!(ORCHESTRATOR_TOOLS.contains(&"task_pr"));
    assert!(!TASK_TOOLS.contains(&"task_pr"));
    assert!(!READ_ONLY_TOOLS.contains(&"task_pr"));
}

#[test]
fn tools_list_has_exactly_the_orchestrator_tools_and_no_settings_set() {
    let result = dispatch(&orchestrator(), "tools/list", &json!({})).unwrap();
    let tools = result["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "task_list",
            "task_get",
            "task_create",
            "task_backlog",
            "task_start",
            "task_report",
            "task_pr",
            "task_lead_touch",
            "task_answer",
            "task_amend",
            "settings_get",
            "task_archive",
            "task_unarchive",
            "task_stop",
            "task_delete",
            "peer_list",
            "peer_send",
            "inbox_read",
            "ask_orchestrator",
            "orchestrator_reply",
            "repo_audit",
            "evolution_run",
            "repo_notes_list",
            "repo_notes_add",
            "repo_notes_remove",
            "costs_summary",
            "hosts_list",
        ]
    );
    assert_eq!(names, ORCHESTRATOR_TOOLS);
    for t in tools {
        assert!(t["inputSchema"]["type"] == "object");
        assert!(!t["inputSchema"]["properties"].is_null());
    }
    for name in TASK_TOOLS {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        let props = tool["inputSchema"]["properties"].as_object().unwrap();
        assert!(!props.is_empty(), "{name}");
    }
}

#[test]
fn a_task_bridge_offers_only_messaging_and_never_a_sender_choice() {
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let task = Bridge {
        task: Some("t1".into()),
        read_only: false,
        call: Box::new(move |method, args| {
            recorded.lock().unwrap().push((method.to_string(), args));
            Ok(json!({}))
        }),
    };
    let result = dispatch(&task, "tools/list", &json!({})).unwrap();
    let tools = result["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, TASK_TOOLS);
    for t in tools {
        assert!(t["inputSchema"]["properties"].get("from").is_none());
    }
    let init = dispatch(&task, "initialize", &json!({})).unwrap();
    assert_eq!(init["serverInfo"]["name"], "sushiai-messages");
    for name in [
        "task_create",
        "task_start",
        "settings_set",
        "task_stop",
        "orchestrator_reply",
    ] {
        let call = json!({"name": name, "arguments": {
            "repo": "/tmp/example-repo", "title": "Example", "goal": "Make a change",
            "id": "t2", "settings": {"parallel": 2}
        }});
        let out = dispatch(&task, "tools/call", &call).unwrap();
        assert_eq!(out["isError"], true, "{name}");
        assert!(
            out["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("unknown tool"),
            "{out}"
        );
    }
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn a_read_only_bridge_offers_only_the_looking_tools_and_refuses_the_rest() {
    let read_only = Bridge {
        read_only: true,
        ..bridge(None)
    };
    let result = dispatch(&read_only, "tools/list", &json!({})).unwrap();
    let names: Vec<&str> = result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, READ_ONLY_TOOLS);
    for name in [
        "task_create",
        "task_start",
        "task_amend",
        "peer_send",
        "repo_notes_add",
    ] {
        let call = json!({"name": name, "arguments": {}});
        let out = dispatch(&read_only, "tools/call", &call).unwrap();
        assert_eq!(out["isError"], true, "{name}");
    }
}

#[test]
fn message_params_send_as_the_bridge_task_whatever_the_agent_claims() {
    let sent = message_params(
        "peer_send",
        json!({"from": "someone-else", "to": "t2", "text": "hi"}),
        Some("t1"),
    );
    assert_eq!(sent["from"], "t1");
    let asked = message_params("ask_orchestrator", json!({"text": "?"}), Some("t1"));
    assert_eq!(asked["from"], "t1");
    assert_eq!(asked["to"], ORCHESTRATOR);
    let inbox = message_params("inbox_read", json!({}), Some("t1"));
    assert_eq!(inbox, json!({"id": "t1"}));
    // Without a task the sender defaults to the orchestrator.
    let inbox = message_params("inbox_read", json!({}), None);
    assert_eq!(inbox["id"], ORCHESTRATOR);
    let answer = json!({"question": "m1", "text": "yes"});
    let reply = message_params("orchestrator_reply", answer, None);
    assert_eq!(reply["from"], ORCHESTRATOR);
    assert_eq!(reply["replyTo"], "m1");
    assert_eq!(reply["text"], "yes");
}

#[test]
fn unknown_method_is_a_json_rpc_method_not_found_error() {
    let err = dispatch(&orchestrator(), "not/a/method", &json!({})).unwrap_err();
    assert_eq!(err.0, -32601);
}

#[test]
fn tools_call_with_an_unknown_tool_name_is_a_tool_error_not_a_protocol_error() {
    let result = dispatch(
        &orchestrator(),
        "tools/call",
        &json!({"name": "task_purge", "arguments": {}}),
    )
    .unwrap();
    assert_eq!(result["isError"], true);
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("unknown tool"));
}

#[test]
fn tools_call_with_malformed_params_is_a_tool_error_not_a_protocol_error() {
    let result = dispatch(&orchestrator(), "tools/call", &json!({"arguments": {}})).unwrap();
    assert_eq!(result["isError"], true);
}

#[test]
fn every_tool_name_maps_to_a_dotted_orch_method() {
    let b = orchestrator();
    for (name, method, ..) in tool_specs() {
        assert_eq!(orch_method_for(&b, name), Some(method));
    }
    assert_eq!(orch_method_for(&b, "task_delete"), Some("task.delete"));
    assert_eq!(orch_method_for(&b, "task_stop"), Some("task.stop"));
    assert_eq!(orch_method_for(&b, "settings_set"), None);
    assert_eq!(orch_method_for(&b, "secrets_set"), None);
}

#[test]
fn a_notification_without_an_id_gets_no_response() {
    let out = handle_line(&orchestrator(), r#"{"method":"ping","params":{}}"#);
    assert!(out.is_none());
}

#[test]
fn a_request_with_an_id_gets_a_jsonrpc_shaped_response() {
    let out = handle_line(&orchestrator(), r#"{"id":1,"method":"ping","params":{}}"#).unwrap();
    assert_eq!(out["jsonrpc"], "2.0");
    assert_eq!(out["id"], 1);
    assert_eq!(out["result"], json!({}));
}

#[test]
fn unparseable_json_answers_with_a_null_id_parse_error() {
    let out = handle_line(&orchestrator(), "not json").unwrap();
    assert_eq!(out["id"], Value::Null);
    assert_eq!(out["error"]["code"], -32700);
}

const TASK_A: &str = "11111111-1111-4111-8111-111111111111";
const TASK_B: &str = "22222222-2222-4222-8222-222222222222";
const UNKNOWN: &str = "99999999-9999-4999-8999-999999999999";

/// A real `App` with two tasks in one repo, in `dir`.
fn live_app(dir: &std::path::Path) -> Arc<App> {
    let app = App::new(dir.into(), dir.into(), "/x/sushiai".to_string()).unwrap();
    for id in [TASK_A, TASK_B] {
        let task = serde_json::from_value(json!({
            "id": id, "title": id, "goal": "g", "criteria": [], "verify": [],
            "repo": "/r", "worktree": "/w", "branch": "b", "baseSha": "s",
            "status": "running", "tier": "standard", "decisions": [],
            "attempts": [], "costUsd": 0.0, "createdAt": 1, "updatedAt": 1,
        }))
        .unwrap();
        app.store.save_task(&task).unwrap();
    }
    app
}

/// One tool call from inside the runtime; the bridge itself blocks.
fn call(bridge: &Bridge, name: &str, arguments: Value) -> Value {
    let params = json!({"name": name, "arguments": arguments});
    tokio::task::block_in_place(|| handle_tools_call(bridge, &params))
}

fn refused(bridge: &Bridge, name: &str, arguments: Value) {
    let out = call(bridge, name, arguments.clone());
    assert_eq!(out["isError"], true, "{name} {arguments} -> {out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_message_is_a_tool_error_and_nothing_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let app = live_app(dir.path());
    let bridge = |task: Option<&str>| Bridge {
        call: {
            let app = app.clone();
            Box::new(move |method, params| {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(app.dispatch(method, params))
                })
            })
        },
        task: task.map(str::to_string),
        read_only: false,
    };
    let orch = bridge(None);
    let a = bridge(Some(TASK_A));
    let stranger = bridge(Some(UNKNOWN));
    let a_to_a = json!({"from": TASK_A, "to": TASK_A, "text": "hi"});
    refused(&orch, "peer_send", a_to_a);
    refused(&a, "peer_send", json!({"to": TASK_A, "text": "hi"}));
    refused(&a, "peer_send", json!({"to": UNKNOWN, "text": "hi"}));
    let from_unknown = json!({"from": UNKNOWN, "to": TASK_A, "text": "hi"});
    refused(&orch, "peer_send", from_unknown);
    refused(&orch, "ask_orchestrator", json!({"text": "?"}));
    refused(&stranger, "ask_orchestrator", json!({"text": "?"}));
    assert!(!dir.path().join("messages.json").exists());

    // The same bridge sends a valid one, still waiting for its reader.
    let out = call(&a, "peer_send", json!({"to": TASK_B, "text": "hi"}));
    assert!(out.get("isError").is_none(), "{out}");
    let inbox = call(&bridge(Some(TASK_B)), "inbox_read", json!({}));
    let text = inbox["content"][0]["text"].as_str().unwrap();
    let inbox: Value = serde_json::from_str(text).unwrap();
    assert_eq!(inbox[0]["from"], TASK_A);
    assert_eq!(inbox[0]["delivered"], false);
}

#[test]
fn task_create_gets_the_project_mcp_only_for_its_own_repo() {
    let file = json!({"repo": "/r", "mcp": {"mcpServers": {"a": {}}}});
    let got = with_task_mcp(
        "task.create",
        json!({"repo": "/r"}),
        Some(file.clone()),
        true,
    );
    assert_eq!(got["mcp"]["mcpServers"]["a"], json!({}));
    let other = with_task_mcp(
        "task.create",
        json!({"repo": "/x"}),
        Some(file.clone()),
        true,
    );
    assert!(other.get("mcp").is_none());
    let own = with_task_mcp(
        "task.create",
        json!({"repo": "/r", "mcp": {"mcpServers": {}}}),
        Some(file.clone()),
        true,
    );
    assert_eq!(own["mcp"], json!({"mcpServers": {}}));
    let start = with_task_mcp("task.start", json!({"repo": "/r"}), Some(file), false);
    assert!(start.get("mcp").is_none());
}

#[test]
fn task_create_from_overseer_replaces_caller_mcp_with_the_project_mcp() {
    let file = json!({"repo": "/r", "mcp": {"mcpServers": {"project": {}}}});
    let args = json!({"repo": "/r", "mcp": {"mcpServers": {"agent": {}}}});
    let got = with_task_mcp("task.create", args, Some(file.clone()), false);
    assert_eq!(got["mcp"], file["mcp"]);
    let other = with_task_mcp(
        "task.create",
        json!({"repo": "/other", "mcp": {"mcpServers": {"agent": {}}}}),
        Some(file),
        false,
    );
    assert!(other.get("mcp").is_none());
}

#[test]
fn task_create_from_overseer_without_a_project_file_removes_caller_mcp() {
    let args = json!({"repo": "/r", "mcp": {"mcpServers": {"agent": {}}}});
    let got = with_task_mcp("task.create", args, None, false);
    assert!(got.get("mcp").is_none());
}

#[test]
fn task_create_from_a_task_bridge_removes_caller_mcp() {
    assert!(!TASK_TOOLS.contains(&"task_create"));
    let args = json!({"repo": "/r", "mcp": {"mcpServers": {"agent": {}}}});
    let got = with_task_mcp("task.create", args.clone(), None, false);
    assert!(got.get("mcp").is_none());
    let file = json!({"repo": "/r", "mcp": {"mcpServers": {"project": {}}}});
    let got = with_task_mcp("task.create", args, Some(file.clone()), false);
    assert_eq!(got["mcp"], file["mcp"]);
}

#[test]
fn task_create_from_owner_chat_keeps_caller_mcp() {
    let args = json!({"repo": "/r", "mcp": {"mcpServers": {"owner": {}}}});
    let file = json!({"repo": "/r", "mcp": {"mcpServers": {"project": {}}}});
    for file in [None, Some(file)] {
        let got = with_task_mcp("task.create", args.clone(), file, true);
        assert_eq!(got, args);
    }
}
