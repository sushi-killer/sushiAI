//! Black-box integration tests (planning backlog): buckets and order,
//! `task.backlog` rules, that only the owner or the autopilot starts a
//! backlog task, and the autopilot's admission. Real daemon, real git.

mod common;

use common::*;
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

/// Writes `<name>.txt` for the task whose goal names `FILE_<name>`. A task
/// holds until `$GATES/hold_<name>` is gone; `$GATES/started_<name>` says it
/// began.
const SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
name=$(printf '%s' "$input" | grep -o 'FILE_[a-z0-9]*' | head -1 | sed 's/FILE_//')
touch "$GATES/started_$name"
while [ -e "$GATES/hold_$name" ]; do sleep 0.1; done
echo "$name" > "$name.txt"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

struct Env {
    daemon: Daemon,
    gates: tempfile::TempDir,
    _scripts: tempfile::TempDir,
    repo: tempfile::TempDir,
}

fn env_vars(script: &Path, gates: &Path) -> Vec<(String, String)> {
    vec![
        (
            "ORCHD_CLAUDE_BIN".to_string(),
            script.to_str().unwrap().to_string(),
        ),
        ("GATES".to_string(), gates.to_str().unwrap().to_string()),
    ]
}

fn setup(parallel: u32) -> Env {
    let gates = tempfile::tempdir().unwrap();
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", SCRIPT);
    let vars = env_vars(&script, gates.path());
    let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let daemon = Daemon::spawn(&refs);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["parallel"] = json!(parallel);
    daemon.request("settings.set", json!({"settings": settings}));
    Env {
        daemon,
        gates,
        _scripts: scripts,
        repo: init_git_repo(),
    }
}

fn set_autopilot(daemon: &Daemon, on: bool) {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["autopilot"] = json!(on);
    daemon.request("settings.set", json!({"settings": settings}));
}

fn create(env: &Env, name: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut params = json!({
        "repo": env.repo.path().to_str().unwrap(),
        "title": format!("Add {name}"),
        "goal": format!("FILE_{name}: write the file"),
        "verify": [format!("test -f {name}.txt")],
    });
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    env.daemon.request("task.create", params)
}

fn id_of(task: &serde_json::Value) -> String {
    task["id"].as_str().unwrap().to_string()
}

fn hold(gates: &Path, name: &str) {
    std::fs::write(gates.join(format!("hold_{name}")), "").unwrap();
}

fn release(gates: &Path, name: &str) {
    let _ = std::fs::remove_file(gates.join(format!("hold_{name}")));
}

fn started(gates: &Path, name: &str) -> bool {
    gates.join(format!("started_{name}")).exists()
}

fn wait_for(what: &str, cond: impl Fn() -> bool) {
    let begin = Instant::now();
    while !cond() {
        assert!(begin.elapsed() < Duration::from_secs(30), "never: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn until_done(daemon: &Daemon, id: &str) -> serde_json::Value {
    poll_until(daemon, id, Duration::from_secs(60), |s| {
        matches!(s, "done" | "failed" | "stopped")
    })
}

fn get(daemon: &Daemon, id: &str) -> serde_json::Value {
    daemon.request("task.get", json!({"id": id}))
}

fn decisions(task: &serde_json::Value) -> String {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn implement_attempts(task: &serde_json::Value) -> usize {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .count()
}

#[test]
fn backlog_buckets_append_move_and_clear() {
    let env = setup(2);
    let a = create(
        &env,
        "a",
        json!({"start": false, "backlog": {"bucket": "next"}}),
    );
    assert_eq!(a["backlog"]["bucket"], "next", "{a}");
    let order_a = a["backlog"]["order"].as_i64().expect("integer order");
    assert_eq!(a["status"], "queued", "{a}");
    let b = create(
        &env,
        "b",
        json!({"start": false, "backlog": {"bucket": "next"}}),
    );
    let order_b = b["backlog"]["order"].as_i64().unwrap();
    assert!(order_b > order_a, "{a} {b}");
    // Nothing started a backlog task, whatever `start` defaults to.
    let c = create(&env, "c", json!({"backlog": {"bucket": "later"}}));
    std::thread::sleep(Duration::from_millis(500));
    assert!(!started(env.gates.path(), "c"));
    assert_eq!(get(&env.daemon, &id_of(&c))["status"], "queued");

    let moved = env.daemon.request(
        "task.backlog",
        json!({"id": id_of(&a), "bucket": "later", "order": 5}),
    );
    assert_eq!(moved["backlog"], json!({"bucket": "later", "order": 5}));
    // Appended after the highest order among the *other* tasks in the bucket
    // (c holds 0), not counting itself.
    let again = env
        .daemon
        .request("task.backlog", json!({"id": id_of(&a), "bucket": "later"}));
    assert_eq!(again["backlog"]["order"], 1, "{again}");
    let cleared = env.daemon.request(
        "task.backlog",
        json!({"id": id_of(&a), "bucket": null, "order": 9}),
    );
    assert!(cleared.get("backlog").is_none(), "{cleared}");
    assert!(get(&env.daemon, &id_of(&a)).get("backlog").is_none());
}

#[test]
fn task_backlog_and_create_reject_what_cannot_be_parked() {
    let env = setup(2);
    let repo = env.repo.path().to_str().unwrap();
    let plain = create(&env, "p", json!({"start": false}));
    let pid = id_of(&plain);
    let err = |params: serde_json::Value| env.daemon.request_error("task.backlog", params);
    assert!(err(json!({"id": pid, "bucket": "someday"})).contains("bucket"));
    assert!(err(json!({"id": pid})).contains("bucket"));
    assert!(
        err(json!({"id": "00000000-0000-0000-0000-000000000000", "bucket": "next"}))
            .contains("not found")
    );

    // A subtask.
    let child = create(&env, "k", json!({"parent": pid}));
    hold(env.gates.path(), "k");
    assert!(err(json!({"id": id_of(&child), "bucket": "next"})).contains("subtask"));
    release(env.gates.path(), "k");

    // Both `parent` and `backlog`: rejected, no task created.
    let before = env.daemon.request("task.list", json!({"repo": repo}));
    let both = env.daemon.request_error(
        "task.create",
        json!({"repo": repo, "title": "x", "goal": "g", "parent": pid,
            "backlog": {"bucket": "next"}}),
    );
    assert!(both.contains("subtask"), "{both}");
    let after = env.daemon.request("task.list", json!({"repo": repo}));
    assert_eq!(
        before.as_array().unwrap().len(),
        after.as_array().unwrap().len()
    );

    // Live loop, then done.
    hold(env.gates.path(), "l");
    let live = create(&env, "l", json!({}));
    let lid = id_of(&live);
    wait_for("l started", || started(env.gates.path(), "l"));
    assert!(err(json!({"id": lid, "bucket": "next"})).contains("started"));
    release(env.gates.path(), "l");
    let done = until_done(&env.daemon, &lid);
    assert_eq!(done["status"], "done", "{done}");
    assert!(err(json!({"id": lid, "bucket": "next"})).contains("started"));

    // Archived.
    let old = create(&env, "z", json!({"start": false}));
    let zid = id_of(&old);
    env.daemon.request("task.archive", json!({"id": zid}));
    assert!(err(json!({"id": zid, "bucket": "next"})).contains("archived"));
}

#[test]
fn a_backlog_task_is_never_started_by_the_graph_or_recovery() {
    let gates = tempfile::tempdir().unwrap();
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", SCRIPT);
    let vars = env_vars(&script, gates.path());
    let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut daemon = Daemon::spawn(&refs);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();
    let mk = |daemon: &Daemon, name: &str, extra: serde_json::Value| {
        let mut params = json!({"repo": repo_path, "title": format!("Add {name}"),
            "goal": format!("FILE_{name}: write"), "verify": [format!("test -f {name}.txt")]});
        for (k, v) in extra.as_object().unwrap() {
            params[k] = v.clone();
        }
        daemon.request("task.create", params)
    };
    let dep = mk(&daemon, "dep", json!({}));
    let dep_id = id_of(&dep);
    let waiting = mk(
        &daemon,
        "w",
        json!({"dependsOn": [dep_id], "backlog": {"bucket": "next"}}),
    );
    let wid = id_of(&waiting);
    assert_eq!(get(&daemon, &wid)["status"], "queued");
    let done = until_done(&daemon, &dep_id);
    assert_eq!(done["status"], "done", "{done}");
    std::thread::sleep(Duration::from_millis(1000));
    let w = get(&daemon, &wid);
    assert_eq!(w["status"], "queued", "{w}");
    assert!(!started(gates.path(), "w"));

    // Restart onto the same data dir.
    let _ = daemon.request("shutdown", json!({}));
    let begin = Instant::now();
    while !matches!(daemon.child.try_wait(), Ok(Some(_))) {
        assert!(begin.elapsed() < Duration::from_secs(10), "no exit");
        std::thread::sleep(Duration::from_millis(50));
    }
    daemon.socket = daemon.data_dir().join("orchd2.sock");
    daemon.child = spawn_orchd_logged(daemon.data_dir(), &daemon.socket, &refs);
    wait_for_socket(&daemon.socket);
    daemon.token = read_control_token(daemon.data_dir());
    std::thread::sleep(Duration::from_millis(1500));
    let w = get(&daemon, &wid);
    assert_eq!(w["status"], "queued", "{w}");
    assert_eq!(implement_attempts(&w), 0, "{w}");
    assert_eq!(w["backlog"]["bucket"], "next", "{w}");
    assert!(!started(gates.path(), "w"));
    daemon.shutdown_and_wait();
}

#[test]
fn task_start_leaves_the_backlog_and_runs_to_done() {
    let env = setup(2);
    let t = create(
        &env,
        "s",
        json!({"start": false, "backlog": {"bucket": "later"}}),
    );
    let id = id_of(&t);
    env.daemon.request("task.start", json!({"id": id}));
    let done = until_done(&env.daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert!(done.get("backlog").is_none(), "{done}");
    assert!(
        decisions(&done).contains("Owner: started from the backlog"),
        "{done}"
    );
}

#[test]
fn autopilot_is_off_by_default() {
    let env = setup(2);
    let settings = env.daemon.request("settings.get", json!({}));
    assert_eq!(settings["autopilot"], false);
    let defaults = env.daemon.request("settings.defaults", json!({}));
    assert_eq!(defaults["autopilot"], false);
    let t = create(
        &env,
        "o",
        json!({"start": false, "backlog": {"bucket": "next"}}),
    );
    std::thread::sleep(Duration::from_millis(800));
    assert!(!started(env.gates.path(), "o"));
    assert_eq!(get(&env.daemon, &id_of(&t))["status"], "queued");
}

#[test]
fn autopilot_starts_ready_next_tasks_in_order_within_parallel() {
    let env = setup(1);
    let g = env.gates.path();
    for n in ["a", "b", "c", "d"] {
        hold(g, n);
    }
    let dep = create(&env, "dep", json!({"start": false}));
    let dep_id = id_of(&dep);
    hold(g, "dep");
    // Created out of order: b has order 1, a has order 0.
    let b = create(
        &env,
        "b",
        json!({"start": false, "backlog": {"bucket": "next", "order": 1}}),
    );
    let a = create(
        &env,
        "a",
        json!({"start": false, "backlog": {"bucket": "next", "order": 0}}),
    );
    // Lowest order, but waits for a dependency that is not done.
    let c = create(
        &env,
        "c",
        json!({"start": false, "dependsOn": [dep_id],
            "backlog": {"bucket": "next", "order": -1}}),
    );
    let later = create(
        &env,
        "d",
        json!({"start": false, "backlog": {"bucket": "later", "order": -5}}),
    );
    let plain = create(&env, "n", json!({"start": false}));
    set_autopilot(&env.daemon, true);

    wait_for("a started", || started(g, "a"));
    std::thread::sleep(Duration::from_millis(500));
    // parallel 1: one loop at a time, and c waits for its dependency.
    assert!(!started(g, "b") && !started(g, "c"), "one at a time");
    let a_now = get(&env.daemon, &id_of(&a));
    assert!(a_now.get("backlog").is_none(), "{a_now}");
    assert!(decisions(&a_now).contains("Autopilot: started from the next backlog"));

    release(g, "a");
    wait_for("b started", || started(g, "b"));
    assert!(!started(g, "c"));
    release(g, "b");
    until_done(&env.daemon, &id_of(&b));
    std::thread::sleep(Duration::from_millis(500));
    assert!(!started(g, "c"), "its dependency is not done");

    // The dependency finishes done; then c starts by itself.
    env.daemon.request("task.start", json!({"id": dep_id}));
    wait_for("dep started", || started(g, "dep"));
    release(g, "dep");
    assert_eq!(until_done(&env.daemon, &dep_id)["status"], "done");
    wait_for("c started", || started(g, "c"));
    release(g, "c");
    let c_done = until_done(&env.daemon, &id_of(&c));
    assert_eq!(c_done["status"], "done", "{c_done}");
    assert!(c_done.get("backlog").is_none(), "{c_done}");
    assert!(decisions(&c_done).contains("Autopilot: started from the next backlog"));

    std::thread::sleep(Duration::from_millis(800));
    assert!(!started(g, "d"), "later bucket is never started");
    assert!(!started(g, "n"), "no backlog is never started");
    let later_now = get(&env.daemon, &id_of(&later));
    assert_eq!(later_now["status"], "queued", "{later_now}");
    assert_eq!(later_now["backlog"]["bucket"], "later");
    assert_eq!(get(&env.daemon, &id_of(&plain))["status"], "queued");
}
