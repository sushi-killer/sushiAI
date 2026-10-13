//! The orchestrator module's relay: a client that wants another host's orchestrator asks the
//! daemon, the daemon passes the request to the listening desktop app as an `orch.relay`
//! notification, and the app's `orch.relay.reply` is the answer.

mod common;

use common::*;
use serde_json::json;

fn enabled_daemon() -> Sandbox {
    let mut sandbox = Sandbox::new();
    let dir = sandbox.home().join("modules");
    std::fs::create_dir_all(&dir).expect("modules dir");
    std::fs::write(dir.join("orch.enabled"), b"").expect("flag");
    sandbox.start_daemon();
    sandbox
}

fn connected(sandbox: &Sandbox) -> Client {
    let mut client = Client::connect(&sandbox.socket());
    client.call("hello", json!({"protocol": 1, "client": "test"}));
    client
}

#[test]
fn a_relayed_call_reaches_the_app_and_its_reply_comes_back() {
    let sandbox = enabled_daemon();
    let mut app = connected(&sandbox);
    let mut agent = connected(&sandbox);
    let params = json!({"host": "ssh:lab", "method": "task.list", "params": {"repo": "/r"}});
    let sent = agent.send("orch.relay.call", params);
    let note = app.wait_note("orch.relay");
    assert_eq!(note["host"], "ssh:lab");
    assert_eq!(note["method"], "task.list");
    assert_eq!(note["params"], json!({"repo": "/r"}));
    let reply = json!({"id": note["id"], "result": [{"id": "t1"}]});
    app.call("orch.relay.reply", reply);
    let got = agent.response("orch.relay.call", sent).expect("answered");
    assert_eq!(got, json!([{"id": "t1"}]));
}

#[test]
fn a_reply_with_an_error_fails_the_caller_with_that_text() {
    let sandbox = enabled_daemon();
    let mut app = connected(&sandbox);
    let mut agent = connected(&sandbox);
    let params = json!({"host": "ssh:lab", "method": "task.get", "params": {"id": "t1"}});
    let sent = agent.send("orch.relay.call", params);
    let note = app.wait_note("orch.relay");
    let reply = json!({"id": note["id"], "error": "Lab is not connected"});
    app.call("orch.relay.reply", reply);
    let err = agent
        .response("orch.relay.call", sent)
        .expect_err("refused");
    assert!(
        err.message.contains("Lab is not connected"),
        "{}",
        err.message
    );
}

#[test]
fn a_reply_to_an_unknown_relay_is_refused() {
    let sandbox = enabled_daemon();
    let mut app = connected(&sandbox);
    let reply = json!({"id": "no-such-relay", "result": 1});
    app.try_call("orch.relay.reply", reply)
        .expect_err("unknown relay id");
}
