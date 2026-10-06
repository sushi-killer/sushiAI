//! Black-box integration tests (evolution): spawn the real `orchd` binary,
//! seed `<data>/evolution/signals.jsonl` and let fake harness scripts play
//! the proposer.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const EVENTS: &str = "tasks/seed/runs/1/events.jsonl";

/// A proposer that records its argv, cwd and brief next to the script, and
/// answers a proposal brief with the JSON result in `proposal-result.json`.
/// Plan and implement briefs get the ordinary fake replies, so an approved
/// proposal's task can run to done.
fn fake_proposer(dir: &Path, reply: &str) -> PathBuf {
    let result = json!({
        "type": "result",
        "total_cost_usd": 0.31,
        "usage": {"input_tokens": 10, "output_tokens": 5},
        "result": reply,
    });
    std::fs::write(dir.join("proposal-result.json"), result.to_string()).unwrap();
    let body = format!(
        r#"#!/bin/sh
input="$(cat)"
d={d}
case "$input" in
  *sushi-proposal*)
    printf '%s\n' "$@" > "$d/argv.txt"
    pwd > "$d/cwd.txt"
    printf '%s\n' "$input" > "$d/brief.md"
    printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-prop"}}'
    cat "$d/proposal-result.json"
    echo
    ;;
  *sushi-plan*)
    printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-plan"}}'
    printf '%s\n' '{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":1,"output_tokens":1}},"result":"```sushi-plan\n{{\"title\":\"Evolve\",\"goal\":\"Apply the proposal\",\"tier\":\"hard\",\"criteria\":[\"Done\"],\"verify\":[\"true\"],\"questions\":[]}}\n```"}}'
    ;;
  *)
    echo changed > CHANGED_MARKER.txt
    printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-impl"}}'
    printf '%s\n' '{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":1,"output_tokens":1}},"result":"```sushi-report\n{{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}}\n```"}}'
    ;;
esac
"#,
        d = dir.display()
    );
    fake_harness_script(dir, "fake-claude-proposer.sh", &body)
}

fn proposal_reply(overrides: Value) -> String {
    let mut block = json!({
        "track": "repo",
        "form": "script",
        "change": "Add scripts/find-usages.sh that lists the callers of a symbol.",
        "evidence": "Three tasks grepped the same symbol by hand.",
        "metric": "Repeated symbol greps per task.",
        "test": "Run the script on a known symbol.",
    });
    for (k, v) in overrides.as_object().unwrap() {
        block[k] = v.clone();
    }
    format!("Here is my proposal.\n\n```sushi-proposal\n{block}\n```")
}

/// Settings with the sandbox untouched and evolution thresholds set.
fn set_evolution(daemon: &Daemon, evolution: Value) {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["evolution"] = evolution;
    settings["review"] = json!("");
    daemon.request("settings.set", json!({"settings": settings}));
}

fn seed_events(daemon: &Daemon, lines: usize) {
    let path = daemon.data_dir().join(EVENTS);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = (1..=lines)
        .map(|n| format!("{{\"raw\":\"event-line-{n}\"}}\n"))
        .collect();
    std::fs::write(path, text).unwrap();
}

/// A minimal finished task, so its signals count as live.
fn seed_task(daemon: &Daemon, id: &str, repo: &str, archived: bool) {
    let dir = daemon.data_dir().join("tasks").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let task = json!({
        "id": id, "title": id, "goal": "g", "criteria": [], "verify": ["true"],
        "repo": repo, "worktree": "/nowhere", "branch": "b", "baseSha": "0",
        "status": "done", "tier": "standard", "attempts": [], "archived": archived,
        "createdAt": 1_000, "updatedAt": 2_000,
    });
    std::fs::write(dir.join("task.json"), task.to_string()).unwrap();
}

/// One signal per task id, all the same kind and detail.
fn seed_signals(daemon: &Daemon, repo: &str, detail: &str, tasks: &[&str], calls: u32) {
    let dir = daemon.data_dir().join("evolution");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("signals.jsonl");
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    for task in tasks {
        seed_task(daemon, task, repo, false);
        let line = json!({
            "kind": "loop",
            "taskId": task,
            "repo": repo,
            "detail": detail,
            "wastedCalls": calls,
            "wastedUsd": 0.1,
            "excerptRef": {"file": EVENTS, "fromLine": 30, "toLine": 32},
            "ts": 1,
        });
        text.push_str(&format!("{line}\n"));
    }
    std::fs::write(path, text).unwrap();
}

fn wait_for_proposals(daemon: &Daemon, want: usize) -> Value {
    let start = Instant::now();
    loop {
        let list = daemon.request("evolution.list", json!({}));
        if list.as_array().unwrap().len() >= want {
            return list;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "proposals did not arrive: {list}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Runs `evolution.run` until every started run has stored its proposal.
fn run_and_wait(daemon: &Daemon, total: usize) -> (Value, Value) {
    let run = daemon.request("evolution.run", json!({}));
    (run, wait_for_proposals(daemon, total))
}

fn thresholds(min_tasks: u32, max: u32) -> Value {
    json!({"minTasks": min_tasks, "minWastedCalls": 100000, "minWastedUsd": 1000.0, "maxProposals": max})
}

#[test]
fn evolution_run_stores_a_valid_reply_as_proposed_and_lists_it() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_proposer(scripts.path(), &proposal_reply(json!({})));
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();
    set_evolution(&daemon, thresholds(3, 3));
    seed_events(&daemon, 80);
    seed_signals(
        &daemon,
        repo_path,
        "reads foo.rs 4 times",
        &["a", "b", "c"],
        2,
    );

    let (run, list) = run_and_wait(&daemon, 1);
    assert_eq!(run["started"].as_array().unwrap().len(), 1, "{run}");
    let p = &list[0];
    assert_eq!(p["status"], "proposed", "{p}");
    assert_eq!(p["track"], "repo");
    assert_eq!(p["form"], "script");
    assert_eq!(p["kind"], "loop");
    assert_eq!(p["repo"], repo_path);
    assert_eq!(p["costUsd"], 0.31);
    assert_eq!(p["clusterKey"], run["started"][0]["clusterKey"]);
    assert!(p["clusterKey"].as_str().unwrap().starts_with("loop:"));
    assert!(p["change"].as_str().unwrap().contains("find-usages.sh"));
    assert!(p.get("evalCommand").is_none(), "{p}");

    let id = p["id"].as_str().unwrap();
    let stored: Value = serde_json::from_str(
        &std::fs::read_to_string(
            daemon
                .data_dir()
                .join("evolution/proposals")
                .join(format!("{id}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(&stored, p);
    let run_dir = daemon.data_dir().join("evolution/proposals").join(id);
    assert!(run_dir.join("brief.md").exists());
    assert!(std::fs::read_to_string(run_dir.join("events.jsonl"))
        .unwrap()
        .contains("sess-prop"));

    // Read-only review flags, the hard tier's route, the cluster's repo.
    let argv = std::fs::read_to_string(scripts.path().join("argv.txt")).unwrap();
    assert!(argv.contains("--permission-mode\nplan\n"), "{argv}");
    assert!(argv.contains("--tools\nRead,Grep,Glob\n"), "{argv}");
    assert!(argv.contains("--setting-sources\n\n"), "{argv}");
    assert!(!argv.contains("acceptEdits"), "{argv}");
    let settings = daemon.request("settings.get", json!({}));
    let hard = settings["tiers"]["hard"].as_str().unwrap();
    let route = settings["routes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == hard)
        .unwrap();
    let model = route["model"].as_str().unwrap();
    assert!(argv.contains(&format!("--model\n{model}\n")), "{argv}");
    let cwd = std::fs::read_to_string(scripts.path().join("cwd.txt")).unwrap();
    assert_eq!(
        Path::new(cwd.trim()).canonicalize().unwrap(),
        repo.path().canonicalize().unwrap()
    );

    let filtered = daemon.request("evolution.list", json!({"repo": "/somewhere/else"}));
    assert!(filtered.as_array().unwrap().is_empty());
    daemon.shutdown_and_wait();
}

#[test]
fn evolution_brief_quotes_raw_events_and_forbids_weakening_checks() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_proposer(scripts.path(), &proposal_reply(json!({})));
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    set_evolution(&daemon, thresholds(7, 3));
    seed_events(&daemon, 200);
    // Seven occurrences: at most five are quoted.
    seed_signals(
        &daemon,
        repo.path().to_str().unwrap(),
        "reads foo.rs 4 times",
        &["a", "b", "c", "d", "e", "f", "g"],
        1,
    );
    let (_, _) = run_and_wait(&daemon, 1);

    let brief = std::fs::read_to_string(scripts.path().join("brief.md")).unwrap();
    assert!(brief.contains("{\"raw\":\"event-line-31\"}"), "{brief}");
    // A 3-line excerpt is padded, and never past 40 raw lines.
    assert!(brief.contains("event-line-31"));
    assert!(brief.contains("```sushi-proposal"));
    let lower = brief.to_lowercase();
    assert!(lower.contains("never weaken or remove"), "{brief}");
    assert!(
        brief.matches("{\"raw\":\"event-line-31\"}").count() <= 5,
        "at most five occurrences"
    );
    assert!(
        brief.matches("{\"raw\":\"event-line-31\"}").count() >= 2,
        "several occurrences are quoted"
    );
    assert!(
        brief.matches("{\"raw\":\"event-line-").count() <= 5 * 40,
        "at most forty lines each"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn evolution_thresholds_each_admit_a_cluster_and_below_all_three_does_not() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_proposer(scripts.path(), &proposal_reply(json!({})));
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();
    seed_events(&daemon, 80);
    // 2 tasks, 10 calls, $0.20 in all.
    seed_signals(&daemon, repo_path, "reads foo.rs 4 times", &["a", "b"], 5);

    let below =
        json!({"minTasks": 3, "minWastedCalls": 11, "minWastedUsd": 0.21, "maxProposals": 3});
    set_evolution(&daemon, below);
    let run = daemon.request("evolution.run", json!({}));
    assert!(run["started"].as_array().unwrap().is_empty(), "{run}");

    for (name, evolution) in [
        (
            "tasks",
            json!({"minTasks": 2, "minWastedCalls": 11, "minWastedUsd": 0.21}),
        ),
        (
            "calls",
            json!({"minTasks": 3, "minWastedCalls": 10, "minWastedUsd": 0.21}),
        ),
        (
            "usd",
            json!({"minTasks": 3, "minWastedCalls": 11, "minWastedUsd": 0.19}),
        ),
    ] {
        let mut evolution = evolution;
        evolution["maxProposals"] = json!(3);
        set_evolution(&daemon, evolution);
        let run = daemon.request("evolution.run", json!({}));
        assert_eq!(run["started"].as_array().unwrap().len(), 1, "{name}: {run}");
        wait_for_proposals(&daemon, 1);
        // Clear the stored proposal so the next bound is tested afresh.
        let list = daemon.request("evolution.list", json!({}));
        let id = list[0]["id"].as_str().unwrap();
        std::fs::remove_file(
            daemon
                .data_dir()
                .join("evolution/proposals")
                .join(format!("{id}.json")),
        )
        .unwrap();
    }
    daemon.shutdown_and_wait();
}

#[test]
fn evolution_run_starts_at_most_max_proposals_and_never_reproposes() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_proposer(scripts.path(), &proposal_reply(json!({})));
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();
    set_evolution(&daemon, thresholds(2, 2));
    seed_events(&daemon, 80);
    for detail in ["alpha problem", "beta problem", "gamma problem"] {
        seed_signals(&daemon, repo_path, detail, &["a", "b"], 1);
    }

    let (first, list) = run_and_wait(&daemon, 2);
    assert_eq!(first["started"].as_array().unwrap().len(), 2, "{first}");
    assert_eq!(list.as_array().unwrap().len(), 2);

    let (second, list) = run_and_wait(&daemon, 3);
    assert_eq!(second["started"].as_array().unwrap().len(), 1, "{second}");
    assert_eq!(list.as_array().unwrap().len(), 3);
    let keys: std::collections::HashSet<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["clusterKey"].as_str().unwrap())
        .collect();
    assert_eq!(keys.len(), 3, "each cluster is proposed once");

    let third = daemon.request("evolution.run", json!({}));
    assert!(third["started"].as_array().unwrap().is_empty(), "{third}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        daemon
            .request("evolution.list", json!({}))
            .as_array()
            .unwrap()
            .len(),
        3
    );
    daemon.shutdown_and_wait();
}

#[test]
fn evolution_reply_without_a_block_stores_nothing_and_is_retried() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_proposer(scripts.path(), "I found nothing worth proposing.");
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    set_evolution(&daemon, thresholds(2, 3));
    seed_events(&daemon, 80);
    seed_signals(
        &daemon,
        repo.path().to_str().unwrap(),
        "same thing",
        &["a", "b"],
        1,
    );

    let first = daemon.request("evolution.run", json!({}));
    assert_eq!(first["started"].as_array().unwrap().len(), 1);
    // Wait until the run has returned (the brief file was written by it).
    poll_until_file(&scripts.path().join("brief.md"));
    std::thread::sleep(Duration::from_millis(500));
    assert!(daemon
        .request("evolution.list", json!({}))
        .as_array()
        .unwrap()
        .is_empty());
    let second = daemon.request("evolution.run", json!({}));
    assert_eq!(second["started"].as_array().unwrap().len(), 1, "{second}");
    daemon.shutdown_and_wait();
}

fn poll_until_file(path: &Path) {
    let start = Instant::now();
    while !path.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "{path:?} never appeared"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn rejected_case(overrides: Value) -> Value {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_proposer(scripts.path(), &proposal_reply(overrides));
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    set_evolution(&daemon, thresholds(2, 3));
    seed_events(&daemon, 80);
    seed_signals(
        &daemon,
        repo.path().to_str().unwrap(),
        "same thing",
        &["a", "b"],
        1,
    );
    let (_, list) = run_and_wait(&daemon, 1);
    let p = list[0].clone();
    // A rejected cluster is not proposed again.
    let again = daemon.request("evolution.run", json!({}));
    assert!(again["started"].as_array().unwrap().is_empty(), "{again}");
    daemon.shutdown_and_wait();
    p
}

#[test]
fn evolution_gate_rejects_a_change_that_weakens_a_check() {
    let p = rejected_case(json!({"change": "Commit with --no-verify and skip the flaky test."}));
    assert_eq!(p["status"], "rejected", "{p}");
    assert!(!p["reason"].as_str().unwrap().is_empty());
}

#[test]
fn evolution_gate_rejects_a_repo_change_that_targets_agents_md() {
    let p = rejected_case(
        json!({"form": "doc", "change": "Add a rule to AGENTS.md about symbol lookup."}),
    );
    assert_eq!(p["status"], "rejected", "{p}");
    assert!(!p["reason"].as_str().unwrap().is_empty());
}

#[test]
fn evolution_approve_creates_a_task_and_adopts_it_once_done() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_proposer(scripts.path(), &proposal_reply(json!({})));
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    set_evolution(&daemon, thresholds(2, 3));
    seed_events(&daemon, 80);
    seed_signals(
        &daemon,
        repo.path().to_str().unwrap(),
        "same thing",
        &["a", "b"],
        1,
    );
    let (_, list) = run_and_wait(&daemon, 1);
    let id = list[0]["id"].as_str().unwrap().to_string();

    let approved = daemon.request("evolution.approve", json!({"id": id}));
    assert_eq!(approved["status"], "approved", "{approved}");
    let task_id = approved["taskId"].as_str().unwrap().to_string();
    let task = daemon.request("task.get", json!({"id": task_id}));
    assert_eq!(task["id"], task_id.as_str());
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(20), |s| {
        s == "done" || s == "failed" || s == "stopped"
    });
    assert_eq!(settled["status"], "done", "{settled}");

    // Approving twice is refused.
    let again = raw_request_with_params(
        &daemon.socket,
        "evolution.approve",
        json!({"id": id}),
        Some(&daemon.token),
    );
    assert!(again["error"].is_object(), "{again}");

    // The next run sees the finished task and adopts the proposal.
    daemon.request("evolution.run", json!({}));
    let list = daemon.request("evolution.list", json!({}));
    assert_eq!(list[0]["status"], "adopted", "{list}");
    assert_eq!(list[0]["adoptedAt"], settled["updatedAt"], "{list}");
    assert!(list[0]["before"].is_object() && list[0]["after"].is_object());
    daemon.shutdown_and_wait();
}

#[test]
fn evolution_harness_proposal_has_an_eval_command_and_cannot_be_approved() {
    let scripts = tempfile::tempdir().unwrap();
    let reply = proposal_reply(json!({
        "track": "harness",
        "form": "prompt",
        "change": "Tell the implementer to read the touched files once.",
        "arm": {"readOnce": true},
    }));
    let script = fake_proposer(scripts.path(), &reply);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    set_evolution(&daemon, thresholds(2, 3));
    seed_events(&daemon, 80);
    seed_signals(
        &daemon,
        repo.path().to_str().unwrap(),
        "same thing",
        &["a", "b"],
        1,
    );
    let (_, list) = run_and_wait(&daemon, 1);
    let p = &list[0];
    let id = p["id"].as_str().unwrap().to_string();
    assert_eq!(p["status"], "proposed", "{p}");
    let cmd = p["evalCommand"].as_str().unwrap();
    assert!(cmd.starts_with("orchd eval run"), "{cmd}");
    assert!(cmd.contains("--arms"), "{cmd}");
    assert!(cmd.contains("readOnce"), "{cmd}");
    assert!(cmd.contains("--socket"), "{cmd}");

    let refused = raw_request_with_params(
        &daemon.socket,
        "evolution.approve",
        json!({"id": id}),
        Some(&daemon.token),
    );
    assert!(refused["error"].is_object(), "{refused}");

    let adopted = daemon.request("evolution.adopt", json!({"id": id}));
    assert_eq!(adopted["status"], "adopted");
    assert!(adopted["adoptedAt"].as_i64().unwrap() > 0);
    daemon.shutdown_and_wait();
}

#[test]
fn evolution_reject_records_the_reason() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_proposer(scripts.path(), &proposal_reply(json!({})));
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    set_evolution(&daemon, thresholds(2, 3));
    seed_events(&daemon, 80);
    seed_signals(
        &daemon,
        repo.path().to_str().unwrap(),
        "same thing",
        &["a", "b"],
        1,
    );
    let (_, list) = run_and_wait(&daemon, 1);
    let id = list[0]["id"].as_str().unwrap();
    let rejected = daemon.request("evolution.reject", json!({"id": id, "reason": "not now"}));
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["reason"], "not now");
    daemon.shutdown_and_wait();
}

#[test]
fn evolution_cli_runs_and_adopts_over_the_default_socket() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_proposer(scripts.path(), &proposal_reply(json!({})));
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    set_evolution(&daemon, thresholds(2, 3));
    seed_events(&daemon, 80);
    seed_signals(
        &daemon,
        repo.path().to_str().unwrap(),
        "same thing",
        &["a", "b"],
        1,
    );
    let bin = env!("CARGO_BIN_EXE_orchd");
    let data = daemon.data_dir().to_str().unwrap();

    let out = Command::new(bin)
        .args(["evolve", "--data", data])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed["started"].as_array().unwrap().len(), 1, "{printed}");

    let list = wait_for_proposals(&daemon, 1);
    let id = list[0]["id"].as_str().unwrap();
    let out = Command::new(bin)
        .args(["evolve", "--data", data, "--adopt", id])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(printed["status"], "adopted");
    assert_eq!(
        daemon.request("evolution.list", json!({}))[0]["status"],
        "adopted"
    );

    let bad = Command::new(bin).args(["evolve"]).output().unwrap();
    assert_eq!(bad.status.code(), Some(2));
    let missing = Command::new(bin)
        .args(["evolve", "--data", data, "--adopt", "nope"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(1));
    daemon.shutdown_and_wait();
}

#[test]
fn evolution_run_ignores_signals_of_archived_and_deleted_tasks() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_proposer(scripts.path(), &proposal_reply(json!({})));
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    let repo = repo.path().to_str().unwrap();
    set_evolution(&daemon, thresholds(3, 3));
    // Three tasks would qualify, but one is archived and one was deleted.
    seed_signals(&daemon, repo, "reads foo", &["t1", "t2", "t3"], 1);
    seed_task(&daemon, "t2", repo, true);
    std::fs::remove_dir_all(daemon.data_dir().join("tasks").join("t3")).unwrap();
    let run = daemon.request("evolution.run", json!({}));
    assert_eq!(run["started"].as_array().unwrap().len(), 0, "{run}");
    daemon.shutdown_and_wait();
}
