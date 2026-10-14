//! The orchestrator MCP bridge offers task_stop and task_delete to the owner's
//! non-task server only, and forwards them as orchd task.stop / task.delete.
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

fn run(task: Option<&str>, read_only: bool, requests: Vec<Value>) -> (Vec<Value>, Calls) {
    let calls: Calls = Arc::new(Mutex::new(Vec::new()));
    let sink = calls.clone();
    let call: sushiai_orch::mcp::Caller = Box::new(move |method, params| {
        sink.lock().unwrap().push((method.to_string(), params));
        Ok(json!({}))
    });
    let input: String = requests
        .iter()
        .map(|r| format!("{r}\n"))
        .collect::<Vec<_>>()
        .join("");
    let mut out: Vec<u8> = Vec::new();
    sushiai_orch::mcp::serve_stdio(
        task.map(str::to_string),
        read_only,
        call,
        input.as_bytes(),
        &mut out,
    );
    let responses = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (responses, calls)
}

fn tool_names(response: &Value) -> Vec<String> {
    response["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn list() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})
}

fn tool_call(name: &str, id: u64) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {"name": name, "arguments": {"id": "t1"}}})
}

#[test]
fn the_orchestrator_server_lists_task_stop_and_task_delete() {
    let (responses, _) = run(None, false, vec![list()]);
    let names = tool_names(&responses[0]);
    assert!(names.contains(&"task_stop".to_string()), "{names:?}");
    assert!(names.contains(&"task_delete".to_string()), "{names:?}");
    assert!(sushiai_orch::mcp::ORCHESTRATOR_TOOLS.contains(&"task_stop"));
    assert!(sushiai_orch::mcp::ORCHESTRATOR_TOOLS.contains(&"task_delete"));
    assert_eq!(names, sushiai_orch::mcp::ORCHESTRATOR_TOOLS);
}

#[test]
fn task_scoped_and_read_only_servers_get_neither_tool() {
    for (task, read_only) in [(Some("t1"), false), (None, true)] {
        let (responses, calls) = run(
            task,
            read_only,
            vec![
                list(),
                tool_call("task_stop", 2),
                tool_call("task_delete", 3),
            ],
        );
        let names = tool_names(&responses[0]);
        assert!(!names.contains(&"task_stop".to_string()), "{names:?}");
        assert!(!names.contains(&"task_delete".to_string()), "{names:?}");
        for response in &responses[1..] {
            assert_eq!(response["result"]["isError"], true, "{response}");
            let text = response["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.contains("unknown tool"), "{text}");
        }
        assert!(calls.lock().unwrap().is_empty());
    }
}

#[test]
fn task_stop_and_task_delete_forward_as_the_daemon_methods() {
    let (responses, calls) = run(
        None,
        false,
        vec![tool_call("task_stop", 1), tool_call("task_delete", 2)],
    );
    for response in &responses {
        assert!(response["result"].get("isError").is_none(), "{response}");
    }
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "task.stop");
    assert_eq!(calls[0].1["id"], "t1");
    assert_eq!(calls[1].0, "task.delete");
    assert_eq!(calls[1].1["id"], "t1");
}
