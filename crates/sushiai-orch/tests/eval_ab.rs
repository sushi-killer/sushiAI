//! Black-box integration tests (eval ab): spawn the real `sushiai daemon` and drive
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
fn task_create_rejects_a_retired_variant_flag_with_a_clear_message() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    for flag in [
        "retryMode",
        "plannerTier",
        "contract",
        "reviewOtherFamily",
        "deferHeavyChecks",
        "leanOutput",
        "reviewBlind",
    ] {
        let result = raw_request_with_params(
            &daemon.socket,
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": "t",
                "goal": "g",
                "variant": {flag: true},
            }),
        );
        assert!(
            result
                .to_string()
                .contains(&format!("variant flag {flag} was retired")),
            "{flag}: {result}"
        );
    }
    daemon.shutdown_and_wait();
}

#[test]
fn the_planner_s_tier_routes_the_task() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
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
    assert_eq!(
        settled["attempts"][1]["routeId"], "claude-opus",
        "{settled}"
    );
    let decisions = settled["decisions"].as_array().unwrap();
    assert!(
        decisions
            .iter()
            .any(|d| d == "Planner: tier hard -> route claude-opus"),
        "{settled}"
    );
    assert!(settled.get("tierFallback").is_none(), "{settled}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

/// Runs `sushiai orch eval run` against `daemon` from `repo`; `(exit ok, stdout, stderr)`.
fn eval_run(daemon: &Daemon, repo: &Path, extra: &[&str]) -> (bool, String, String) {
    let out = Command::new(sushiai_bin())
        .args(["orch", "eval", "run"])
        .env("SUSHIAI_HOME", daemon.home.path())
        .arg("--repo")
        .arg(repo)
        .args(extra)
        .output()
        .expect("run sushiai orch eval");
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
            r#"{"advisor":true}"#,
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
    assert_eq!(task["variant"]["advisor"], true);

    // Ad-hoc pair: one task per arm, same request and base, one shared name.
    let (ok, stdout, stderr) = eval_run(
        &daemon,
        repo.path(),
        &[
            "--request",
            "try the thing",
            "--arms",
            r#"[{"advisor":true},{"reviewEvidence":true}]"#,
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

#[test]
fn a_done_eval_task_counts_as_a_success_only_when_its_check_passes() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        FAKE_CLAUDE_PASS_NO_ARGS,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    fit_sandbox(&mut settings);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let create = |title: &str, check: &str| {
        daemon.request(
            "task.create",
            serde_json::json!({
                "repo": repo.path().to_str().unwrap(),
                "title": title,
                "goal": "Make a trivial change",
                "verify": ["true"],
                "evalSet": "set-x",
                "evalName": title,
                "evalCheck": check,
            }),
        )
    };
    // The check sees the task's final commit in a throwaway checkout, not
    // the task's own worktree.
    let good = create(
        "good",
        "test -f CHANGED_MARKER.txt && pwd | grep -q eval-check",
    );
    let bad = create("bad", "test -f NO_SUCH_FILE.txt; echo missing >&2; exit 3");
    let mut worktrees = vec![];
    for (task, code) in [(&good, 0), (&bad, 3)] {
        let id = task["id"].as_str().unwrap().to_string();
        let settled = poll_until(&daemon, &id, Duration::from_secs(30), |s| {
            s == "done" || s == "failed" || s == "stopped" || s == "waiting"
        });
        assert_eq!(settled["status"], "done", "{settled}");
        assert_eq!(settled["evalCheck"]["code"], code, "{settled}");
        if code != 0 {
            assert!(
                settled["evalCheck"]["tail"]
                    .as_str()
                    .unwrap()
                    .contains("missing"),
                "{settled}"
            );
        }
        worktrees.push(settled["worktree"].as_str().unwrap().to_string());
        let leftover = Path::new(&settled["worktree"].as_str().unwrap()).exists();
        assert!(!leftover, "a done task's worktree is removed");
    }

    // The daemon's data dir feeds the report exactly as `sushiai orch ab` reads it.
    let out = Command::new(sushiai_bin())
        .env_remove("SUSHIAI_HOME")
        .args(["orch", "ab", "--eval", "set-x", "--data"])
        .arg(daemon.data_dir())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(text.contains("1/2 = 50%"), "{text}");
    let listed = git_out(repo.path(), &["worktree", "list", "--porcelain"]);
    assert!(!listed.contains("eval-check"), "{listed}");

    daemon.shutdown_and_wait();
    for w in worktrees {
        let _ = std::fs::remove_dir_all(w);
    }
}

const FAKE_CLAUDE_PASS_NO_ARGS: &str = "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}'\nprintf '%s\\n' '{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":\"```sushi-report\\n{\\\"outcome\\\":\\\"complete\\\",\\\"summary\\\":\\\"done\\\",\\\"decisions\\\":[],\\\"question\\\":\\\"\\\"}\\n```\"}'\n";
