//! The orchestration module in the real daemon binary.

mod common;

use std::path::Path;
use std::process::{Child, Command, Stdio};

use common::*;
use serde_json::{json, Value};
use sushiai_protocol::code;

/// Enables the module the way the desktop does: the flag file the daemon reads at start.
fn enable(sandbox: &Sandbox) {
    let dir = sandbox.home().join("modules");
    std::fs::create_dir_all(&dir).expect("modules dir");
    std::fs::write(dir.join("orch.enabled"), b"").expect("flag");
}

fn enabled_daemon() -> Sandbox {
    let mut sandbox = Sandbox::new();
    enable(&sandbox);
    sandbox.start_daemon();
    sandbox
}

fn hello(client: &mut Client) -> Value {
    client.call("hello", json!({"protocol": 1, "client": "test"}))
}

/// A one-commit git repo with synthetic content.
fn repo(dir: &Path) -> String {
    let path = dir.join("repo");
    std::fs::create_dir_all(&path).expect("repo dir");
    std::fs::write(path.join("a.txt"), "one\n").expect("file");
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    ] {
        let status = Command::new("git")
            .args(&args)
            .current_dir(&path)
            .status()
            .expect("git");
        assert!(status.success(), "git {args:?}");
    }
    path.to_string_lossy().into_owned()
}

fn new_task(repo: &str) -> Value {
    json!({"repo": repo, "title": "Add a line", "goal": "Add a line to a.txt",
           "criteria": ["a.txt has two lines"], "verify": ["true"]})
}

#[test]
fn an_enabled_module_lists_its_capability_and_runs_a_task_through_the_daemon() {
    let sandbox = enabled_daemon();
    let mut client = Client::connect(&sandbox.socket());
    assert!(hello(&mut client)["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .any(|c| c == "orch"));
    let repo = repo(sandbox.home());
    let task = client.call("orch.task.create", new_task(&repo));
    let id = task["id"].as_str().expect("task id").to_string();
    let listed = client.call("orch.task.list", json!({}));
    let ids: Vec<&str> = listed
        .as_array()
        .expect("task list")
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert!(ids.contains(&id.as_str()), "{listed}");
    // The state lives under the daemon home.
    assert!(sandbox
        .home()
        .join("orchestrator/tasks")
        .join(&id)
        .join("task.json")
        .exists());
}

#[test]
fn a_client_gets_orch_event_notifications() {
    let sandbox = enabled_daemon();
    let mut client = Client::connect(&sandbox.socket());
    hello(&mut client);
    let repo = repo(sandbox.home());
    let task = client.call("orch.task.create", new_task(&repo));
    let note = client.wait_note("orch.event");
    assert_eq!(note["event"], "task", "{note}");
    assert_eq!(note["task"]["id"], task["id"]);
}

#[test]
fn an_unknown_method_and_a_refused_call_carry_their_own_codes() {
    let sandbox = enabled_daemon();
    let mut client = Client::connect(&sandbox.socket());
    hello(&mut client);
    let err = client
        .try_call("orch.nope", json!({}))
        .expect_err("unknown method");
    assert_eq!(err.code, code::METHOD_NOT_FOUND);
    let err = client
        .try_call("orch.task.get", json!({"id": "not-a-task"}))
        .expect_err("refused");
    assert_eq!(err.code, code::INVALID_REQUEST);
    assert!(err.message.contains("invalid task id"), "{}", err.message);
    // The module still answers after a refusal.
    client.call("orch.settings.get", json!({}));
}

#[test]
fn a_module_that_is_not_enabled_is_not_hosted() {
    let mut sandbox = Sandbox::new();
    sandbox.start_daemon();
    let mut client = Client::connect(&sandbox.socket());
    assert!(!hello(&mut client)["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .any(|c| c == "orch"));
    let err = client
        .try_call("orch.task.list", json!({}))
        .expect_err("not hosted");
    assert_eq!(err.code, code::METHOD_NOT_FOUND);
    assert!(!sandbox.home().join("orchestrator").exists());
}

#[test]
fn a_hook_role_connection_cannot_call_orch() {
    let sandbox = enabled_daemon();
    let mut hook = Client::connect(&sandbox.socket());
    hook.call(
        "hello",
        json!({"protocol": 1, "client": "t", "role": "hook"}),
    );
    let err = hook
        .try_call("orch.task.list", json!({}))
        .expect_err("refused");
    assert_eq!(err.code, code::UNAUTHORIZED);
}

/// A process whose executable is called `orchd`: a copy of `sleep`.
fn fake_orchd(dir: &Path) -> Child {
    let exe = dir.join("orchd");
    std::fs::copy("/bin/sleep", &exe).expect("copy sleep");
    Command::new(&exe)
        .arg("120")
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn fake orchd")
}

#[test]
fn a_legacy_orchd_is_stopped_when_the_module_starts() {
    let mut sandbox = Sandbox::new();
    enable(&sandbox);
    let data = sandbox.home().join("orchestrator");
    std::fs::create_dir_all(&data).expect("data dir");
    let mut legacy = fake_orchd(sandbox.home());
    std::fs::write(data.join("orchd.pid"), legacy.id().to_string()).expect("pid file");
    std::fs::write(data.join("control.token"), "old\n").expect("token");
    sandbox.start_daemon();
    let mut client = Client::connect(&sandbox.socket());
    hello(&mut client);
    // A call waits for the module, so the cleanup is done when it answers.
    client.call("orch.settings.get", json!({}));
    let status = legacy.wait().expect("legacy exited");
    assert!(!status.success(), "the legacy process was signalled");
    assert!(!data.join("orchd.pid").exists());
    assert!(!data.join("control.token").exists());
}

#[test]
fn the_module_creates_no_socket_pid_or_token_of_its_own() {
    let sandbox = enabled_daemon();
    let mut client = Client::connect(&sandbox.socket());
    hello(&mut client);
    client.call("orch.settings.get", json!({}));
    let data = sandbox.home().join("orchestrator");
    for name in ["orchd.sock", "orchd.pid", "control.token"] {
        assert!(!data.join(name).exists(), "{name}");
    }
}
