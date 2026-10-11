//! Black-box tests for `pipeline.describe`, the presets and `task.preset`.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

fn spawn() -> Daemon {
    Daemon::spawn(&[])
}

fn set_settings(daemon: &Daemon, edit: impl FnOnce(&mut Value)) {
    let mut settings = daemon.request("settings.get", json!({}));
    edit(&mut settings);
    daemon.request("settings.set", json!({"settings": settings}));
}

fn set_error(daemon: &Daemon, edit: impl FnOnce(&mut Value)) -> String {
    let mut settings = daemon.request("settings.get", json!({}));
    edit(&mut settings);
    daemon.request_error("settings.set", json!({"settings": settings}))
}

fn create(daemon: &Daemon, repo: &Path, extra: Value) -> Value {
    let mut params = json!({
        "repo": repo.to_str().unwrap(),
        "title": "t",
        "goal": "g",
        "start": false,
    });
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    daemon.request("task.create", params)
}

fn stage<'a>(described: &'a Value, id: &str) -> &'a Value {
    described["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("no stage {id} in {described}"))
}

fn ids(described: &Value) -> Vec<String> {
    described["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect()
}

/// A created task's record rewritten with `edit`.
fn rewrite(daemon: &Daemon, id: &str, edit: impl FnOnce(&mut Value)) {
    let path = daemon.data_dir().join("tasks").join(id).join("task.json");
    let mut t: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut t);
    std::fs::write(&path, t.to_string()).unwrap();
}

fn attempt(n: u32, stage: &str, status: &str, extra: Value) -> Value {
    let mut a = json!({
        "n": n, "stage": stage, "routeId": "claude-sonnet", "harness": "claude",
        "model": "sonnet", "reason": "test", "startedAt": 1_000, "endedAt": 5_000,
        "status": status, "costUsd": 0.5,
    });
    for (k, v) in extra.as_object().unwrap() {
        a[k] = v.clone();
    }
    a
}

#[test]
fn describe_lists_stages_with_routes_from_settings() {
    let daemon = spawn();
    set_settings(&daemon, |s| {
        s["planner"] = json!("claude-opus");
        s["review"] = json!("codex");
        s["tiers"]["standard"] = json!("claude-sonnet");
        s["maxAttempts"] = json!(3);
        s["experiments"]["advisor"] = json!(true);
        s["experiments"]["heldOut"] = json!(true);
    });
    let d = daemon.request("pipeline.describe", json!({}));
    assert_eq!(
        ids(&d),
        [
            "brief",
            "plan",
            "brief-check",
            "base-checks",
            "implement",
            "checks",
            "held-out",
            "review",
            "judge",
            "land",
            "advisor",
            "question"
        ]
    );
    assert!(!ids(&d).iter().any(|i| i.contains("plan-review")));
    assert_eq!(stage(&d, "plan")["routeId"], "claude-opus");
    assert_eq!(stage(&d, "plan")["model"], "opus");
    assert_eq!(stage(&d, "review")["routeId"], "codex");
    assert_eq!(stage(&d, "implement")["routeId"], "claude-sonnet");
    assert_eq!(stage(&d, "implement")["model"], "sonnet");
    assert_eq!(stage(&d, "implement")["maxAttempts"], 3);
    assert_eq!(stage(&d, "advisor")["enabled"], true);
    assert_eq!(stage(&d, "held-out")["enabled"], true);
    assert_eq!(stage(&d, "advisor")["kind"], "side");
    assert_eq!(stage(&d, "land")["kind"], "main");
    assert!(stage(&d, "plan").get("state").is_none());
    let presets = d["presets"].as_array().unwrap();
    assert_eq!(presets.len(), 3);
    assert!(presets.iter().all(|p| p["builtin"] == true));
}

#[test]
fn preset_matches_settings_path() {
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();

    let via_settings = spawn();
    set_settings(&via_settings, |s| {
        s["review"] = json!("codex");
        s["briefCheckRoute"] = json!("");
        s["maxAttempts"] = json!(2);
        s["planner"] = json!("claude-opus");
        s["experiments"]["advisor"] = json!(true);
        s["experiments"]["heldOut"] = json!(true);
    });
    let expected = via_settings.request("pipeline.describe", json!({"repo": repo_path}));

    let via_preset = spawn();
    set_settings(&via_preset, |s| {
        s["presets"] = json!([{
            "id": "mine", "label": "Mine", "builtin": false,
            "values": {
                "briefCheck": false, "advisor": true, "heldOut": true,
                "planner": "claude-opus", "review": "codex", "maxAttempts": 2,
            },
        }]);
        s["repoPresets"] = json!({ repo_path: "mine" });
    });
    let got = via_preset.request("pipeline.describe", json!({"repo": repo_path}));
    assert_eq!(got["presetId"], "mine");
    assert_eq!(got["stages"], expected["stages"]);
}

#[test]
fn task_create_applies_repo_preset() {
    let daemon = spawn();
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();
    set_settings(&daemon, |s| {
        s["presets"] = json!([{
            "id": "strict", "label": "Strict",
            "values": {
                "briefCheck": false, "advisor": true, "heldOut": true,
                "review": "claude-opus", "maxAttempts": 6,
            },
        }]);
        s["repoPresets"] = json!({ repo_path: "strict" });
    });
    let task = create(&daemon, repo.path(), json!({}));
    let got = daemon.request("task.get", json!({"id": task["id"]}));
    let v = &got["variant"];
    assert_eq!(v["reviewRoute"], "claude-opus");
    assert_eq!(v["briefCheckRoute"], "");
    assert_eq!(v["maxAttempts"], 6);
    assert_eq!(v["advisor"], true);
    assert!(v.get("groundedChecks").is_none());
    let described = daemon.request("pipeline.describe", json!({"taskId": got["id"]}));
    assert_eq!(stage(&described, "held-out")["enabled"], true);

    // An explicit preset wins over the repo's, and a variant over both.
    let fast = create(
        &daemon,
        repo.path(),
        json!({"preset": "fast", "variant": {"maxAttempts": 5}}),
    );
    let v = &fast["variant"];
    assert_eq!(v["maxAttempts"], 5);
    assert_eq!(v["advisor"], false);
    assert_eq!(v["heldOut"], false);
    assert_eq!(v["briefCheckRoute"], "");
    assert!(v.get("reviewRoute").is_none());
}

#[test]
fn task_states_and_question_branch() {
    let daemon = spawn();
    let repo = init_git_repo();

    let running = create(&daemon, repo.path(), json!({}));
    let rid = running["id"].as_str().unwrap();
    rewrite(&daemon, rid, |t| {
        t["status"] = json!("running");
        t["attempts"] = json!([attempt(1, "implement", "running", json!({"endedAt": null}))]);
    });
    let d = daemon.request("pipeline.describe", json!({"taskId": rid}));
    assert_eq!(stage(&d, "implement")["state"], "running");
    assert_eq!(stage(&d, "brief")["state"], "done");
    assert_eq!(stage(&d, "land")["state"], "pending");
    assert_eq!(stage(&d, "review")["state"], "pending");
    assert!(d.get("branch").is_none());

    let failed = create(&daemon, repo.path(), json!({}));
    let fid = failed["id"].as_str().unwrap();
    rewrite(&daemon, fid, |t| {
        t["status"] = json!("failed");
        t["attempts"] = json!([attempt(
            1,
            "implement",
            "failed",
            json!({
                "verify": [{"command": "make test", "code": 1, "tail": "", "ms": 2000}],
                "failure": {"kind": "verify", "detail": "tests failed", "signature": "verify"},
            })
        )]);
    });
    let d = daemon.request("pipeline.describe", json!({"taskId": fid}));
    assert_eq!(stage(&d, "implement")["state"], "failed");
    assert_eq!(stage(&d, "checks")["state"], "failed");
    assert_eq!(stage(&d, "checks")["durationMs"], 2000);
    let implement = stage(&d, "implement");
    assert_eq!(implement["costUsd"], 0.5);
    assert!(implement["durationMs"].as_i64().unwrap() > 0);

    for (held, to) in [(true, "overseer"), (false, "owner")] {
        let waiting = create(&daemon, repo.path(), json!({}));
        let wid = waiting["id"].as_str().unwrap();
        rewrite(&daemon, wid, |t| {
            t["status"] = json!("waiting");
            t["question"] = json!({
                "text": "which way?", "options": ["a", "b"], "kind": "agent_question",
                "askedBy": "review", "heldForOverseer": held,
            });
        });
        let d = daemon.request("pipeline.describe", json!({"taskId": wid}));
        assert_eq!(d["branch"], json!({"from": "review", "to": to}));
        assert_eq!(stage(&d, "question")["state"], "running");
    }
}

#[test]
fn shared_checks_are_not_optional() {
    let daemon = spawn();
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();
    for preset in ["fast", "careful", "docs"] {
        set_settings(&daemon, |s| s["repoPresets"] = json!({ repo_path: preset }));
        let d = daemon.request("pipeline.describe", json!({"repo": repo_path}));
        assert_eq!(d["presetId"], preset);
        for id in ["checks", "base-checks"] {
            let s = stage(&d, id);
            assert_eq!(s["ownerOnly"], true, "{preset} {id}");
            assert_eq!(s["optional"], false, "{preset} {id}");
            assert_eq!(s["enabled"], true, "{preset} {id}");
        }
    }
    let fast = {
        set_settings(&daemon, |s| s["repoPresets"] = json!({ repo_path: "fast" }));
        daemon.request("pipeline.describe", json!({"repo": repo_path}))
    };
    assert_eq!(stage(&fast, "brief-check")["enabled"], false);
    assert_eq!(stage(&fast, "advisor")["enabled"], false);
    assert_eq!(stage(&fast, "implement")["maxAttempts"], 2);
}

#[test]
fn held_out_defaults_on_and_its_switch_leaves_regular_checks_enabled() {
    let daemon = spawn();
    let repo = init_git_repo();
    let default = create(&daemon, repo.path(), json!({}));
    let described = daemon.request("pipeline.describe", json!({"taskId": default["id"]}));
    assert_eq!(stage(&described, "held-out")["enabled"], true);
    assert_eq!(stage(&described, "held-out")["optional"], true);

    for enabled in [false, true] {
        let task = create(
            &daemon,
            repo.path(),
            json!({
                "variant": {"heldOut": enabled},
                "criteria": ["the marker exists"],
                "checks": [{"criterion": 0, "run": "test -f marker.txt"}],
                "heldOut": {"criterion": 0, "run": "test -s marker.txt"},
            }),
        );
        assert_eq!(task["checks"][0]["run"], "test -f marker.txt");
        assert_eq!(task["heldOut"]["run"], "test -s marker.txt");
        assert!(task["variant"].get("groundedChecks").is_none());
        let described = daemon.request("pipeline.describe", json!({"taskId": task["id"]}));
        assert_eq!(stage(&described, "held-out")["enabled"], enabled);
        for id in ["checks", "base-checks"] {
            assert_eq!(stage(&described, id)["enabled"], true);
            assert_eq!(stage(&described, id)["optional"], false);
        }
    }
}

#[test]
fn settings_set_rejects_bad_presets() {
    let daemon = spawn();
    let err = set_error(&daemon, |s| {
        s["presets"] = json!([{"id": "x", "label": "X", "values": {"review": "nope"}}]);
    });
    assert!(err.contains("not a configured route"), "{err}");
    let err = set_error(&daemon, |s| {
        s["presets"] = json!([{"id": "x", "label": "X", "values": {"planner": "nope"}}]);
    });
    assert!(err.contains("not a configured route"), "{err}");
    let err = set_error(&daemon, |s| {
        s["presets"] = json!([
            {"id": "x", "label": "X", "values": {}},
            {"id": "x", "label": "Y", "values": {}},
        ]);
    });
    assert!(err.contains("duplicated"), "{err}");
    let err = set_error(&daemon, |s| {
        s["presets"] = json!([{"id": "fast", "label": "F", "values": {}}]);
    });
    assert!(err.contains("built-in"), "{err}");
    let err = set_error(&daemon, |s| {
        s["repoPresets"] = json!({"/some/repo": "ghost"})
    });
    assert!(err.contains("unknown preset"), "{err}");
    // "" and "auto" are fine for the two route switches.
    set_settings(&daemon, |s| {
        s["presets"] = json!([
            {"id": "x", "label": "X", "values": {"review": "", "briefCheckRoute": "auto"}}
        ]);
    });
}

#[test]
fn task_create_rejects_an_unknown_preset_and_a_variant_route_that_is_not_configured() {
    let daemon = spawn();
    let repo = init_git_repo();
    let err = daemon.request_error(
        "task.create",
        json!({"repo": repo.path().to_str().unwrap(), "title": "t", "goal": "g",
               "start": false, "preset": "ghost"}),
    );
    assert!(err.contains("unknown preset"), "{err}");
    let err = daemon.request_error(
        "task.create",
        json!({"repo": repo.path().to_str().unwrap(), "title": "t", "goal": "g",
               "start": false, "variant": {"reviewRoute": "nope"}}),
    );
    assert!(err.contains("reviewRoute"), "{err}");
}

#[test]
fn task_preset_is_refused_after_an_implement_attempt() {
    let daemon = spawn();
    let repo = init_git_repo();
    let fresh = create(&daemon, repo.path(), json!({}));
    let id = fresh["id"].as_str().unwrap();
    let ok = daemon.request("task.preset", json!({"id": id, "preset": "careful"}));
    assert_eq!(ok["variant"]["maxAttempts"], 4);
    assert_eq!(ok["variant"]["advisor"], true);
    let got = daemon.request("task.get", json!({"id": id}));
    assert!(got["variant"].get("groundedChecks").is_none());
    let described = daemon.request("pipeline.describe", json!({"taskId": id}));
    assert_eq!(stage(&described, "held-out")["enabled"], true);
    let err = daemon.request_error("task.preset", json!({"id": id, "preset": "ghost"}));
    assert!(err.contains("unknown preset"), "{err}");

    rewrite(&daemon, id, |t| {
        t["attempts"] = json!([attempt(1, "implement", "failed", json!({}))]);
    });
    let err = daemon.request_error("task.preset", json!({"id": id, "preset": "fast"}));
    assert!(err.contains("implement attempt"), "{err}");
}
