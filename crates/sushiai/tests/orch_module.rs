//! The stub orchestration module in the real daemon binary.

mod common;

use common::*;
use serde_json::json;
use sushiai_protocol::code;

#[test]
fn orch_echo_round_trips_and_hello_lists_the_capability() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = Client::connect(&sandbox.socket());
    let hello = client.call("hello", json!({"protocol": 1, "client": "test"}));
    assert!(hello["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .any(|c| c == "orch"));
    let reply = client.call("orch.echo", json!({"a": 1}));
    assert_eq!(reply["echo"], json!({"a": 1}));
}

#[test]
fn a_hook_role_connection_cannot_call_orch() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut hook = Client::connect(&sandbox.socket());
    hook.call(
        "hello",
        json!({"protocol": 1, "client": "t", "role": "hook"}),
    );
    let err = hook.try_call("orch.echo", json!({})).expect_err("refused");
    assert_eq!(err.code, code::UNAUTHORIZED);
}
