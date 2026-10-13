//! The bridge's `host` argument: a call that names a host goes through the daemon's relay, one
//! that does not is exactly what it always was.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{json, Value};
use sushiai_orch::mcp::{serve_stdio, Caller};

type Calls = Rc<RefCell<Vec<(String, Value)>>>;

/// Feeds `requests` to a bridge whose backend answers `{"ok": true}`; returns what the backend
/// was asked and every reply line.
fn run(task: Option<&str>, requests: &[Value]) -> (Calls, Vec<Value>) {
    let calls: Calls = Rc::default();
    let sink = calls.clone();
    let call: Caller = Box::new(move |method, params| {
        sink.borrow_mut().push((method.to_string(), params));
        Ok(json!({"ok": true}))
    });
    let input: String = requests.iter().map(|r| format!("{r}\n")).collect();
    let input = input.as_bytes();
    let task = task.map(str::to_string);
    let mut out = Vec::new();
    serve_stdio(task, false, call, input, &mut out);
    let text = String::from_utf8(out).unwrap();
    let mut replies = Vec::new();
    for line in text.lines() {
        replies.push(serde_json::from_str(line).unwrap());
    }
    (calls, replies)
}

fn tool_call(name: &str, arguments: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
           "params": {"name": name, "arguments": arguments}})
}

fn tools_list() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
}

#[test]
fn a_call_with_a_host_is_relayed_and_without_one_goes_straight_to_the_daemon() {
    let with_host = tool_call("task_get", json!({"id": "t1", "host": "ssh:lab"}));
    let without = tool_call("task_get", json!({"id": "t1"}));
    let (calls, replies) = run(None, &[with_host, without]);
    let calls = calls.borrow();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "relay.call");
    let expected = json!({"host": "ssh:lab", "method": "task.get", "params": {"id": "t1"}});
    assert_eq!(calls[0].1, expected);
    assert_eq!(calls[1].0, "task.get");
    assert_eq!(calls[1].1, json!({"id": "t1"}));
    let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("ok"), "{text}");
}

#[test]
fn a_created_task_on_another_host_is_relayed_with_its_source_and_without_the_host() {
    let args = json!({"repo": "/home/dev/app", "request": "Add export", "host": "ssh:lab"});
    let (calls, _) = run(None, &[tool_call("task_create", args)]);
    let calls = calls.borrow();
    assert_eq!(calls[0].0, "relay.call");
    assert_eq!(calls[0].1["host"], "ssh:lab");
    assert_eq!(calls[0].1["method"], "task.create");
    let params = &calls[0].1["params"];
    assert_eq!(params["repo"], "/home/dev/app");
    assert_eq!(params["source"], "chat");
    assert!(params.get("host").is_none());
}

#[test]
fn every_task_tool_and_the_inbox_take_a_host_and_hosts_list_exists() {
    let (_, replies) = run(None, &[tools_list()]);
    let tools = replies[0]["result"]["tools"].as_array().unwrap();
    for name in [
        "task_list",
        "task_get",
        "task_create",
        "task_answer",
        "task_archive",
        "task_unarchive",
        "inbox_read",
    ] {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        let host = &tool["inputSchema"]["properties"]["host"];
        assert_eq!(host["type"], "string", "{name}");
    }
    assert!(tools.iter().any(|t| t["name"] == "hosts_list"));
}

#[test]
fn hosts_list_is_relayed_as_the_hosts_list_method() {
    let (calls, _) = run(None, &[tool_call("hosts_list", json!({}))]);
    let calls = calls.borrow();
    assert_eq!(calls[0].0, "relay.call");
    assert_eq!(calls[0].1["method"], "hosts.list");
}

#[test]
fn a_task_bridge_never_offers_or_accepts_a_host() {
    let (_, replies) = run(Some("t1"), &[tools_list()]);
    let tools = replies[0]["result"]["tools"].as_array().unwrap();
    assert!(!tools.is_empty());
    for tool in tools {
        assert!(tool["inputSchema"]["properties"].get("host").is_none());
    }
    let call = tool_call("inbox_read", json!({"host": "ssh:lab"}));
    let (calls, replies) = run(Some("t1"), &[call]);
    assert_eq!(replies[0]["result"]["isError"], true);
    assert!(calls.borrow().is_empty());
}

#[test]
fn a_malformed_host_is_a_tool_error_and_sends_nothing() {
    for host in [json!(7), json!(""), json!({"id": "lab"})] {
        let call = tool_call("task_list", json!({"host": host}));
        let (calls, replies) = run(None, &[call]);
        assert_eq!(replies[0]["result"]["isError"], true, "{host}");
        assert!(calls.borrow().is_empty(), "{host}");
    }
}
