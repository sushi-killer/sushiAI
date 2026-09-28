//! Black-box integration tests (eval ab): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[test]
fn task_create_rejects_an_unknown_variant_flag() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let result = raw_request_with_params(
        &daemon.socket,
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "t",
            "goal": "g",
            "variant": {"retrymode": "fresh"},
        }),
        Some(&daemon.token),
    );
    assert!(
        result
            .to_string()
            .contains("unknown variant flag: retrymode"),
        "{result}"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn the_planner_s_tier_routes_the_task_only_when_the_variant_asks_for_it() {
    for (planner_tier, route) in [(true, "claude-opus"), (false, "claude-sonnet")] {
        let scripts_dir = tempfile::tempdir().unwrap();
        let script =
            fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
        let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
        let mut settings = daemon.request("settings.get", serde_json::json!({}));
        settings["review"] = serde_json::json!("");
        daemon.request("settings.set", serde_json::json!({"settings": settings}));

        let repo = init_git_repo();
        let task = daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "request": "add dark mode to the settings screen",
                "variant": {"plannerTier": planner_tier},
                "start": true,
            }),
        );
        let task_id = task["id"].as_str().unwrap().to_string();
        let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
            s == "done" || s == "failed" || s == "stopped" || s == "waiting"
        });
        assert_eq!(settled["status"], "done", "{settled}");
        assert_eq!(settled["plannedTier"], "hard", "{settled}");
        // The plan run's cost counts toward the task, not only the implement run's.
        assert_eq!(settled["attempts"][0]["costUsd"], 0.01, "{settled}");
        assert!(
            (settled["costUsd"].as_f64().unwrap() - 0.02).abs() < 1e-9,
            "{settled}"
        );
        assert_eq!(settled["attempts"][1]["routeId"], route, "{settled}");
        let decisions = settled["decisions"].as_array().unwrap();
        let noted = decisions
            .iter()
            .any(|d| d == "Planner: tier hard -> route claude-opus");
        assert_eq!(noted, planner_tier, "{settled}");
        // With `plannerTier: false` and no classifier key configured (the
        // default), the tier falls back to standard instead of the
        // planner's "hard" -- `plannedTier` above still records the
        // planner's choice, just unused for routing.
        let fell_back = decisions.iter().any(|d| {
            d == "Jev: tier unavailable (no classifier key) -> fallback standard, route claude-sonnet"
        });
        assert_eq!(fell_back, !planner_tier, "{settled}");
        if planner_tier {
            assert!(settled.get("tierFallback").is_none(), "{settled}");
        } else {
            assert_eq!(settled["tierFallback"], "no classifier key", "{settled}");
        }

        let worktree = task["worktree"].as_str().unwrap().to_string();
        daemon.shutdown_and_wait();
        let _ = std::fs::remove_dir_all(worktree);
    }
}

/// Runs `orchd eval run` against `daemon` from `repo`; `(exit ok, stdout, stderr)`.
fn eval_run(daemon: &Daemon, repo: &Path, extra: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_orchd"))
        .args(["eval", "run", "--data"])
        .arg(daemon.data_dir())
        .arg("--socket")
        .arg(&daemon.socket)
        .arg("--repo")
        .arg(repo)
        .args(extra)
        .output()
        .expect("run orchd eval");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn eval_run_creates_one_task_at_the_resolved_base_with_its_eval_fields() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    std::fs::write(repo.path().join("second.txt"), "2\n").unwrap();
    git_out(repo.path(), &["add", "."]);
    git_out(repo.path(), &["commit", "-q", "-m", "second"]);
    let parent = git_out(repo.path(), &["rev-parse", "HEAD^"]);

    let sets = tempfile::tempdir().unwrap();
    let set = sets.path().join("set-x.json");
    std::fs::write(
        &set,
        serde_json::json!({"tasks": [
            {"name": "one", "base": "HEAD^", "request": "add dark mode"},
            {"name": "two", "base": "HEAD", "request": "something else"},
        ]})
        .to_string(),
    )
    .unwrap();

    let (ok, stdout, stderr) = eval_run(
        &daemon,
        repo.path(),
        &[
            "--set",
            set.to_str().unwrap(),
            "--only",
            "one",
            "--variant",
            r#"{"retryMode":"fresh"}"#,
        ],
    );
    assert!(ok, "stderr: {stderr}");
    let created: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(created["tasks"].as_array().unwrap().len(), 1, "{stdout}");
    let id = created["tasks"][0]["id"].as_str().unwrap();

    let list = daemon.request("task.list", serde_json::json!({}));
    assert_eq!(list.as_array().unwrap().len(), 1, "{list}");
    let task = daemon.request("task.get", serde_json::json!({"id": id}));
    assert_eq!(task["baseSha"], parent.as_str(), "{task}");
    assert_eq!(task["evalSet"], "set-x");
    assert_eq!(task["evalName"], "one");
    assert_eq!(task["variant"]["retryMode"], "fresh");

    // Ad-hoc pair: one task per arm, same request and base, one shared name.
    let (ok, stdout, stderr) = eval_run(
        &daemon,
        repo.path(),
        &[
            "--request",
            "try the thing",
            "--arms",
            r#"[{"retryMode":"fresh"},{"contract":true}]"#,
        ],
    );
    assert!(ok, "stderr: {stderr}");
    let created: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
    let tasks = created["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2, "{stdout}");
    assert_eq!(tasks[0]["evalSet"], "adhoc");
    assert_eq!(tasks[0]["evalName"], tasks[1]["evalName"]);
    let head = git_out(repo.path(), &["rev-parse", "HEAD"]);
    for t in tasks {
        let full = daemon.request("task.get", serde_json::json!({"id": t["id"]}));
        assert_eq!(full["baseSha"], head.as_str());
    }

    let worktrees: Vec<String> = daemon
        .request("task.list", serde_json::json!({}))
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["worktree"].as_str().unwrap().to_string())
        .collect();
    daemon.shutdown_and_wait();
    for w in worktrees {
        let _ = std::fs::remove_dir_all(w);
    }
}

#[test]
fn variant_route_overrides_pick_the_planner_and_the_tier_s_implement_route() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    // Settings plan and implement `hard` on claude-opus; this task moves both
    // to claude-sonnet. The planner's tier (hard) routes it.
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "variant": {
                "plannerTier": true,
                "plannerRoute": "claude-sonnet",
                "tierRoutes": {"hard": "claude-sonnet"},
            },
            "start": true,
        }),
    );
    assert_eq!(task["variant"]["plannerRoute"], "claude-sonnet", "{task}");
    assert_eq!(
        task["variant"]["tierRoutes"],
        serde_json::json!({"hard": "claude-sonnet"}),
        "{task}"
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(15), |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    assert_eq!(settled["status"], "done", "{settled}");
    assert_eq!(settled["tier"], "hard", "{settled}");
    assert_eq!(settled["attempts"][0]["stage"], "plan", "{settled}");
    assert_eq!(
        settled["attempts"][0]["routeId"], "claude-sonnet",
        "{settled}"
    );
    assert_eq!(settled["attempts"][1]["stage"], "implement", "{settled}");
    assert_eq!(
        settled["attempts"][1]["routeId"], "claude-sonnet",
        "{settled}"
    );
    let decisions = settled["decisions"].as_array().unwrap();
    for line in [
        "Variant: planner -> route claude-sonnet (override)",
        "Variant: tier hard -> route claude-sonnet (override)",
        "Planner: tier hard -> route claude-sonnet",
    ] {
        assert!(decisions.iter().any(|d| d == line), "{line}: {settled}");
    }

    // The override was this task's only: the next one plans on the settings'
    // planner and records no override.
    let other = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add dark mode to the settings screen",
            "start": false,
        }),
    );
    assert!(other["variant"].get("plannerRoute").is_none(), "{other}");
    assert!(other["variant"].get("tierRoutes").is_none(), "{other}");
    let other_id = other["id"].as_str().unwrap().to_string();
    let other_settled = poll_until(&daemon, &other_id, Duration::from_secs(15), |s| {
        s == "stopped" || s == "done" || s == "failed"
    });
    assert_eq!(
        other_settled["attempts"][0]["routeId"], "claude-opus",
        "{other_settled}"
    );
    assert!(
        !other_settled["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d.as_str().unwrap().starts_with("Variant:")),
        "{other_settled}"
    );

    daemon.shutdown_and_wait();
    for t in [&task, &other] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn eval_run_with_an_unknown_name_or_an_unresolvable_base_creates_nothing() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let sets = tempfile::tempdir().unwrap();
    let set = sets.path().join("set-y.json");
    std::fs::write(
        &set,
        serde_json::json!({"tasks": [
            {"name": "good", "base": "HEAD", "request": "r"},
            {"name": "bad-base", "base": "no-such-ref^", "request": "r"},
        ]})
        .to_string(),
    )
    .unwrap();

    let (ok, _, stderr) = eval_run(
        &daemon,
        repo.path(),
        &["--set", set.to_str().unwrap(), "--only", "good,nope"],
    );
    assert!(!ok);
    assert!(
        stderr.contains("unknown task name in --only: nope"),
        "{stderr}"
    );

    let (ok, _, stderr) = eval_run(&daemon, repo.path(), &["--set", set.to_str().unwrap()]);
    assert!(!ok);
    assert!(
        stderr.contains("bad-base") && stderr.contains("does not resolve"),
        "{stderr}"
    );

    let list = daemon.request("task.list", serde_json::json!({}));
    assert!(list.as_array().unwrap().is_empty(), "{list}");
    daemon.shutdown_and_wait();
}

#[test]
fn task_create_rejects_a_route_override_naming_an_unknown_route_and_creates_nothing() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    for (variant, expected) in [
        (
            serde_json::json!({"tierRoutes": {"hard": "claude-nope"}}),
            "variant tierRoutes.hard \"claude-nope\" is not a configured route",
        ),
        (
            serde_json::json!({"plannerRoute": "claude-nope"}),
            "variant plannerRoute \"claude-nope\" is not a configured route",
        ),
    ] {
        let result = raw_request_with_params(
            &daemon.socket,
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "t",
                "goal": "g",
                "variant": variant,
            }),
            Some(&daemon.token),
        );
        assert_eq!(result["error"]["message"], expected, "{result}");
    }
    let tasks = daemon.request("task.list", serde_json::json!({}));
    assert_eq!(tasks, serde_json::json!([]), "{tasks}");
    let worktrees = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&worktrees.stdout);
    assert_eq!(listed.matches("worktree ").count(), 1, "{listed}");
    daemon.shutdown_and_wait();
}
