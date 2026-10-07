//! Black-box integration tests (audit): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A fake harness for `repo.audit`: records its argv and brief next to the
/// script (never in the audited repo), then replies with `reply` as its
/// final result.
fn fake_audit_harness(dir: &Path, reply: &str) -> PathBuf {
    let result = serde_json::json!({
        "type": "result",
        "total_cost_usd": 0.42,
        "usage": {"input_tokens": 10, "output_tokens": 5},
        "result": reply,
    });
    let body = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > {d}/argv.txt\ncat > {d}/brief.md\ncat <<'JSON'\n{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-audit\"}}\n{result}\nJSON\n",
        d = dir.display()
    );
    fake_harness_script(dir, "fake-claude-audit.sh", &body)
}

fn poll_audit(daemon: &Daemon, id: &str) -> serde_json::Value {
    let start = Instant::now();
    loop {
        let audit = daemon.request("repo.audit.get", serde_json::json!({"id": id}));
        if audit["status"] != "running" {
            return audit;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "audit did not settle: {audit}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn repo_audit_stores_the_parsed_report_and_returns_it() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let reply = "Audit done.\n\n```sushi-audit\n{\"summary\":\"Decent, slow tests.\",\"items\":[{\"area\":\"2. Build and test commands\",\"grade\":\"weak\",\"evidence\":[\"README.md:1\",\"unmeasured: no test command documented\"],\"recommendation\":\"Document one test command in README.md.\",\"effort\":\"small\"}],\"topFixes\":[\"Document one test command in README.md.\"]}\n```";
    let script = fake_audit_harness(scripts_dir.path(), reply);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();

    let started = daemon.request("repo.audit", serde_json::json!({"repo": repo_path}));
    let id = started["id"].as_str().unwrap().to_string();
    assert_eq!(started["status"], "running");
    // The planner's route by default.
    assert_eq!(started["routeId"], "claude-opus");

    let audit = poll_audit(&daemon, &id);
    assert_eq!(audit["status"], "done", "{audit}");
    assert!(audit.get("error").is_none(), "{audit}");
    assert_eq!(audit["costUsd"], 0.42);
    let report = &audit["report"];
    assert_eq!(report["summary"], "Decent, slow tests.");
    assert_eq!(report["items"][0]["area"], "2. Build and test commands");
    assert_eq!(report["items"][0]["grade"], "weak");
    assert_eq!(report["items"][0]["effort"], "small");
    assert_eq!(report["items"][0]["evidence"][0], "README.md:1");
    assert_eq!(
        report["topFixes"][0],
        "Document one test command in README.md."
    );

    let dir = daemon.data_dir().join("audits").join(&id);
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(&stored, report);
    let brief = std::fs::read_to_string(dir.join("brief.md")).unwrap();
    assert!(brief.contains("```sushi-audit"));
    assert!(std::fs::read_to_string(dir.join("events.jsonl"))
        .unwrap()
        .contains("sess-audit"));
    // The harness got that same brief, and the read-only review flags.
    assert_eq!(
        std::fs::read_to_string(scripts_dir.path().join("brief.md")).unwrap(),
        brief
    );
    let argv = std::fs::read_to_string(scripts_dir.path().join("argv.txt")).unwrap();
    assert!(argv.contains("--permission-mode\nplan\n"), "{argv}");
    assert!(argv.contains("--tools\nRead,Grep,Glob\n"), "{argv}");
    assert!(!argv.contains("acceptEdits"), "{argv}");

    let list = daemon.request("repo.audit.list", serde_json::json!({"repo": repo_path}));
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], id.as_str());
    assert_eq!(list[0]["report"], *report);

    daemon.shutdown_and_wait();
}

#[test]
fn repo_audit_without_a_parsable_report_stores_an_error_and_no_report() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_audit_harness(scripts_dir.path(), "The repository looks fine to me.");
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = init_git_repo();

    let started = daemon.request(
        "repo.audit",
        serde_json::json!({"repo": repo.path().to_str().unwrap()}),
    );
    let id = started["id"].as_str().unwrap().to_string();
    let audit = poll_audit(&daemon, &id);
    assert_eq!(audit["status"], "failed", "{audit}");
    assert!(
        audit["error"]
            .as_str()
            .unwrap()
            .contains("no sushi-audit report"),
        "{audit}"
    );
    assert!(audit["report"].is_null(), "{audit}");
    assert_eq!(audit["costUsd"], 0.42);
    assert!(!daemon
        .data_dir()
        .join("audits")
        .join(&id)
        .join("report.json")
        .exists());
    daemon.shutdown_and_wait();
}

#[test]
fn repo_audit_refuses_a_directory_that_is_not_a_git_repository() {
    let daemon = Daemon::spawn(&[]);
    let plain = tempfile::tempdir().unwrap();
    let result = raw_request_with_params(
        &daemon.socket,
        "repo.audit",
        serde_json::json!({"repo": plain.path().to_str().unwrap()}),
    );
    assert_eq!(
        result["error"]["message"], "repo is not a git repository",
        "{result}"
    );
    let audits = daemon.data_dir().join("audits");
    let created = std::fs::read_dir(&audits).map(|d| d.count()).unwrap_or(0);
    assert_eq!(created, 0, "no audit may be recorded");
    daemon.shutdown_and_wait();
}
