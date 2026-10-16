use super::call::call_tool;
use super::schema::tool_specs;
use super::scope::{bridge_source, with_source, Bridge};
use serde_json::json;

#[test]
fn the_bridge_source_defaults_to_lead_and_only_recognizes_chat_and_overseer() {
    for (declared, expected) in [
        (None, "lead"),
        (Some(""), "lead"),
        (Some("unknown"), "lead"),
        (Some("lead"), "lead"),
        (Some("chat"), "chat"),
        (Some("overseer"), "overseer"),
    ] {
        let source = bridge_source(declared);
        assert_eq!(source, expected);
        for forged in ["owner", "policy", "lead", "chat", "overseer"] {
            let args = with_source(
                "task.answer",
                json!({"id": "t", "source": forged}),
                false,
                source,
            );
            assert_eq!(args["source"], expected);
            let task = with_source(
                "task.answer",
                json!({"id": "t", "source": forged}),
                true,
                source,
            );
            assert_eq!(task["source"], "mcp");
        }
    }
}

#[test]
fn task_stop_and_task_delete_always_carry_the_bridges_source() {
    for method in ["task.stop", "task.delete"] {
        let chat = with_source(method, json!({"id": "t", "source": "owner"}), false, "chat");
        assert_eq!(chat["source"], "chat", "{method}");
        let task = with_source(method, json!({"id": "t"}), true, "chat");
        assert_eq!(task["source"], "mcp", "{method}");
    }
}

#[test]
fn an_overseer_task_delete_is_refused_before_the_backend_but_task_stop_goes_through() {
    use std::sync::{Arc, Mutex};
    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = calls.clone();
    let bridge = Bridge {
        call: Box::new(move |method, _| {
            sink.lock().unwrap().push(method.to_string());
            Ok(json!({}))
        }),
        task: None,
        read_only: false,
    };
    let deleted = call_tool(
        &bridge,
        &json!({"name": "task_delete", "arguments": {"id": "t"}}),
        Some("overseer"),
    );
    let stopped = call_tool(
        &bridge,
        &json!({"name": "task_stop", "arguments": {"id": "t"}}),
        Some("overseer"),
    );
    assert_eq!(deleted["isError"], true);
    assert!(stopped.get("isError").is_none());
    assert_eq!(*calls.lock().unwrap(), ["task.stop"]);
}

#[test]
fn a_task_answer_always_carries_the_bridges_source_whatever_the_caller_sent() {
    let chat = with_source(
        "task.answer",
        json!({"id": "t", "source": ""}),
        false,
        "chat",
    );
    assert_eq!(chat["source"], "chat");
    let task = with_source("task.answer", json!({"id": "t"}), true, "chat");
    assert_eq!(task["source"], "mcp");
    let overseer = with_source("task.answer", json!({"id": "t"}), false, "overseer");
    assert_eq!(overseer["source"], "overseer");
    let other = with_source("task.get", json!({"id": "t"}), true, "chat");
    assert!(other.get("source").is_none());
    let create = with_source("task.create", json!({}), false, "overseer");
    assert_eq!(create["source"], "chat");
    assert!(crate::task::TASK_SOURCES.contains(&"chat"));
}

#[test]
fn decision_memory_repo_notes_carry_the_bridges_source_and_precedents_are_no_tools() {
    for method in ["repo.notes.add", "repo.notes.remove"] {
        let forged = with_source(method, json!({"source": "owner"}), false, "overseer");
        assert_eq!(forged["source"], "overseer");
    }
    let specs = tool_specs();
    assert!(specs.iter().all(|t| !t.1.starts_with("precedents.")));
    let answer = specs.iter().find(|t| t.0 == "task_answer").unwrap();
    assert!(answer.3["properties"].get("like").is_some());
}

#[test]
fn a_task_amend_always_carries_the_bridges_source_whatever_the_caller_sent() {
    let overseer = with_source(
        "task.amend",
        json!({"id": "t", "source": ""}),
        false,
        "overseer",
    );
    assert_eq!(overseer["source"], "overseer");
    let task = with_source("task.amend", json!({"id": "t"}), true, "overseer");
    assert_eq!(task["source"], "mcp");
}
