//! Black-box integration tests (timeline): spawn the real `orchd` binary and
//! read `task.timeline` and `failures.catalogue` over its NDJSON unix socket.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

fn attempt(n: u32, stage: &str, start: i64, end: i64) -> Value {
    json!({
        "n": n, "stage": stage, "routeId": "claude-sonnet", "harness": "claude",
        "model": "sonnet", "reason": "test", "resumed": false,
        "startedAt": start, "endedAt": end, "status": "passed",
    })
}

fn write_task(data: &Path, id: &str, repo: &str, title: &str, attempts: Vec<Value>) {
    let task = json!({
        "id": id, "title": title, "goal": "g", "criteria": [], "verify": ["true"],
        "repo": repo, "worktree": "/nowhere", "branch": "b", "baseSha": "0",
        "status": "done", "tier": "standard", "attempts": attempts,
        "createdAt": 1_000, "updatedAt": 90_000,
    });
    let dir = data.join("tasks").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("task.json"), task.to_string()).unwrap();
}

fn failed(kind: &str, detail: &str, signature: &str) -> Value {
    json!({"kind": kind, "detail": detail, "signature": signature})
}

#[test]
fn task_timeline_orders_plan_implement_review_and_the_owner_wait_with_costs() {
    let daemon = Daemon::spawn(&[]);
    let id = "11111111-1111-4111-8111-111111111111";

    let mut plan = attempt(1, "plan", 1_000, 5_000);
    plan["costUsd"] = json!(0.02);
    // Failed on verify: 3s of work then a 2s check.
    let mut first = attempt(2, "implement", 5_000, 10_000);
    first["status"] = json!("failed");
    first["costUsd"] = json!(0.5);
    first["verify"] = json!([{"command": "true", "code": 1, "tail": "boom", "ms": 2_000}]);
    first["failure"] = failed("verify", "boom", "verify:boom");
    // The owner answered 20s after the failure; then a passing attempt with
    // a review.
    let mut second = attempt(3, "implement", 30_000, 40_000);
    second["costUsd"] = json!(0.7);
    second["reviewCostUsd"] = json!(0.1);
    second["review"] = json!({"verdict": "PASS", "findings": []});
    second["verify"] = json!([{"command": "true", "code": 0, "tail": "", "ms": 1_000}]);
    write_task(
        daemon.data_dir(),
        id,
        "/repo",
        "Timeline case",
        vec![plan, first, second],
    );

    let timeline = daemon.request("task.timeline", json!({"id": id}));
    let segs = timeline.as_array().unwrap_or_else(|| panic!("{timeline}"));
    let stages: Vec<(&str, u64)> = segs
        .iter()
        .map(|s| (s["stage"].as_str().unwrap(), s["attempt"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        stages,
        [
            ("plan", 1),
            ("implement", 2),
            ("verify", 2),
            ("wait", 3),
            ("implement", 3),
            ("verify", 3),
            ("review", 3),
        ],
        "{timeline}"
    );
    assert_eq!(segs[0]["costUsd"], 0.02);
    assert_eq!(segs[1]["costUsd"], 0.5);
    assert_eq!(segs[1]["endedAt"], 8_000);
    assert_eq!(segs[2]["failureKind"], "verify");
    assert_eq!(segs[2]["outcome"], "fail");
    assert_eq!(segs[3]["startedAt"], 10_000);
    assert_eq!(segs[3]["endedAt"], 30_000);
    assert_eq!(segs[6]["costUsd"], 0.1);
    assert_eq!(segs[6]["outcome"], "pass");
    // Segments never overlap.
    for pair in segs.windows(2) {
        assert!(pair[0]["endedAt"].as_i64() <= pair[1]["startedAt"].as_i64());
    }

    let missing = raw_request_with_params(
        &daemon.socket,
        "task.timeline",
        json!({"id": "22222222-2222-4222-8222-222222222222"}),
        Some(&daemon.token),
    );
    assert!(missing.to_string().contains("not found"), "{missing}");
    daemon.shutdown_and_wait();
}

#[test]
fn failures_catalogue_groups_two_tasks_with_one_signature_into_one_row() {
    let daemon = Daemon::spawn(&[]);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let sig = "verify:error TS: cannot find name";
    let mut a = attempt(1, "implement", now - 3_000, now - 2_000);
    a["status"] = json!("failed");
    a["failure"] = failed("verify", "error TS1: cannot find name x", sig);
    let mut b = attempt(1, "implement", now - 1_500, now - 1_000);
    b["status"] = json!("failed");
    b["failure"] = failed("verify", "error TS2: cannot find name y", sig);
    let mut other = attempt(1, "implement", now - 500, now);
    other["status"] = json!("failed");
    other["failure"] = failed("stall", "no output", "stall:no output");
    let mut old = attempt(1, "implement", 1_000, 2_000);
    old["status"] = json!("failed");
    old["failure"] = failed("stall", "long ago", "stall:long ago");
    write_task(
        daemon.data_dir(),
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "/repo",
        "First",
        vec![a],
    );
    write_task(
        daemon.data_dir(),
        "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        "/repo",
        "Second",
        vec![b],
    );
    write_task(
        daemon.data_dir(),
        "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
        "/other",
        "Third",
        vec![other, old],
    );

    let all = daemon.request("failures.catalogue", json!({}));
    let rows = all.as_array().unwrap_or_else(|| panic!("{all}"));
    assert_eq!(rows.len(), 3, "{all}");
    assert_eq!(rows[0]["signature"], sig);
    assert_eq!(rows[0]["kind"], "verify");
    assert_eq!(rows[0]["count"], 2);
    let titles: Vec<&str> = rows[0]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["First", "Second"]);
    assert_eq!(rows[0]["lastSeen"], now - 1_000);
    assert_eq!(rows[0]["exampleDetail"], "error TS2: cannot find name y");
    assert!(rows[0]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"] == rows[0]["exampleTaskId"]));
    assert_eq!(rows[1]["count"], 1);

    let recent = daemon.request("failures.catalogue", json!({"sinceDays": 1}));
    assert_eq!(recent.as_array().unwrap().len(), 2, "{recent}");
    let scoped = daemon.request("failures.catalogue", json!({"repo": "/other"}));
    assert_eq!(scoped.as_array().unwrap().len(), 2, "{scoped}");
    daemon.shutdown_and_wait();
}

#[test]
fn failures_catalogue_ignores_archived_and_eval_tasks() {
    let daemon = Daemon::spawn(&[]);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let mut fail = attempt(1, "implement", now - 2_000, now - 1_000);
    fail["status"] = json!("failed");
    fail["failure"] = failed("verify", "boom", "verify:boom");
    let ids = [
        ("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", "Live", json!({})),
        (
            "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
            "Archived",
            json!({"archived": true}),
        ),
        (
            "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
            "Eval",
            json!({"evalSet": "set-1"}),
        ),
    ];
    for (id, title, extra) in &ids {
        write_task(daemon.data_dir(), id, "/repo", title, vec![fail.clone()]);
        let path = daemon.data_dir().join("tasks").join(id).join("task.json");
        let mut task: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for (k, v) in extra.as_object().unwrap() {
            task[k] = v.clone();
        }
        std::fs::write(&path, task.to_string()).unwrap();
    }
    let rows = daemon.request("failures.catalogue", json!({}));
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["count"], 1);
    assert_eq!(rows[0]["tasks"][0]["title"], "Live");
    daemon.shutdown_and_wait();
}
