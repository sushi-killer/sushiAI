//! Black-box integration tests (past work): the repo notes RPCs and the
//! `## Past work` section of a plan brief, built from finished task records.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

fn planner_daemon() -> (Daemon, tempfile::TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-planner.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    daemon.request("settings.set", json!({"settings": settings}));
    (daemon, scripts)
}

/// One attempt record of a seeded task.
fn attempt(
    stage: &str,
    files: &[&str],
    failure: Option<&str>,
    verify: &[(&str, i32)],
    handoff: Option<&str>,
) -> Value {
    let mut a = json!({
        "n": 1, "stage": stage, "routeId": "claude-sonnet", "harness": "claude",
        "model": "sonnet", "reason": "seed", "resumed": false, "startedAt": 1,
        "status": if failure.is_some() { "failed" } else { "passed" },
        "changedFiles": files,
        "verify": verify.iter().map(|(c, code)| json!({"command": c, "code": code, "tail": "", "ms": 1})).collect::<Vec<_>>(),
    });
    if let Some(kind) = failure {
        a["failure"] = json!({"kind": kind, "detail": "d", "signature": "s"});
    }
    if let Some(h) = handoff {
        a["handoff"] = json!(h);
    }
    a
}

struct Seed<'a> {
    title: &'a str,
    request: Option<&'a str>,
    status: &'a str,
    attempts: Vec<Value>,
    updated_at: i64,
    eval_set: Option<&'a str>,
}

impl<'a> Seed<'a> {
    fn new(title: &'a str, status: &'a str, attempts: Vec<Value>) -> Self {
        Seed {
            title,
            request: None,
            status,
            attempts,
            updated_at: 1000,
            eval_set: None,
        }
    }
}

/// Creates a task record without running it, then rewrites it as a finished
/// one. Returns the repo root as orchd stores it.
fn seed(daemon: &Daemon, repo: &Path, s: Seed) -> String {
    let task = daemon.request(
        "task.create",
        json!({"repo": repo.to_str().unwrap(), "title": s.title, "goal": "g", "start": false}),
    );
    let id = task["id"].as_str().unwrap();
    let path = daemon.data_dir().join("tasks").join(id).join("task.json");
    let mut t: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    t["status"] = json!(s.status);
    t["attempts"] = json!(s.attempts);
    t["updatedAt"] = json!(s.updated_at);
    if let Some(r) = s.request {
        t["request"] = json!(r);
    }
    if let Some(e) = s.eval_set {
        t["evalSet"] = json!(e);
    }
    std::fs::write(&path, t.to_string()).unwrap();
    task["repo"].as_str().unwrap().to_string()
}

/// Creates and starts a drafting task and returns its plan brief.
fn plan_brief(daemon: &Daemon, repo: &Path, request: &str, extra: Value) -> String {
    let mut params = json!({"repo": repo.to_str().unwrap(), "request": request, "start": true});
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    let task = daemon.request("task.create", params);
    let id = task["id"].as_str().unwrap().to_string();
    poll_until(daemon, &id, Duration::from_secs(20), |s| {
        matches!(s, "done" | "failed" | "stopped" | "waiting")
    });
    run_file(daemon, &id, "plan/brief.md")
}

fn section(brief: &str) -> &str {
    let start = brief.find("## Past work").expect("no Past work section");
    let end = brief.find("## Instructions").unwrap();
    brief[start..end].trim_end()
}

#[test]
fn a_request_naming_a_file_lists_the_past_task_that_changed_it() {
    let (daemon, _s) = planner_daemon();
    let repo = init_git_repo();
    seed(
        &daemon,
        repo.path(),
        Seed::new(
            "Rework the scheduler",
            "done",
            vec![
                attempt(
                    "implement",
                    &["src/engine/plan.rs", "src/other.rs"],
                    Some("verify"),
                    &[
                        ("cargo test scheduler_case", 1),
                        ("held-out check (criterion 1)", 1),
                        ("cargo fmt --check", 0),
                    ],
                    None,
                ),
                attempt(
                    "implement",
                    &["src/engine/plan.rs"],
                    None,
                    &[],
                    Some("remember the plan lock"),
                ),
            ],
        ),
    );
    let brief = plan_brief(
        &daemon,
        repo.path(),
        "Fix the bug in `src/engine/plan.rs`, please.",
        json!({}),
    );
    let past = brief.find("## Past work").expect("section");
    assert!(brief.find("## Request").unwrap() < past);
    assert!(past < brief.find("## Instructions").unwrap());
    let s = section(&brief);
    assert!(s.contains("Rework the scheduler"), "{s}");
    assert!(s.contains("done, 2 implement attempt(s)"), "{s}");
    assert!(s.contains("failure kinds: verify"), "{s}");
    assert!(s.contains("cargo test scheduler_case"), "{s}");
    assert!(!s.contains("cargo fmt --check"), "{s}");
    assert!(s.contains("remember the plan lock"), "{s}");
    assert!(s.contains("<untrusted-data>"), "{s}");
    assert!(!s.contains("held-out check"), "{s}");
    daemon.shutdown_and_wait();
}

#[test]
fn a_directory_in_the_request_matches_files_under_it() {
    let (daemon, _s) = planner_daemon();
    let repo = init_git_repo();
    seed(
        &daemon,
        repo.path(),
        Seed::new(
            "Touch the store",
            "failed",
            vec![attempt(
                "implement",
                &["src/store/io.rs"],
                Some("review"),
                &[],
                None,
            )],
        ),
    );
    let brief = plan_brief(&daemon, repo.path(), "Tidy up src/store", json!({}));
    let s = section(&brief);
    assert!(
        s.contains("Touch the store") && s.contains("failed, 1 implement"),
        "{s}"
    );
    assert!(s.contains("failure kinds: review"), "{s}");
    daemon.shutdown_and_wait();
}

#[test]
fn shared_words_list_a_task_when_no_path_matches_and_unrelated_ones_stay_out() {
    let (daemon, _s) = planner_daemon();
    let repo = init_git_repo();
    let mut related = Seed::new(
        "Migrate database schema versioning",
        "done",
        vec![attempt("implement", &["db/migrate.sql"], None, &[], None)],
    );
    related.request = Some("Migrate database schema versioning");
    seed(&daemon, repo.path(), related);
    seed(
        &daemon,
        repo.path(),
        Seed::new(
            "Rename schema helper",
            "done",
            vec![attempt("implement", &["misc/x.txt"], None, &[], None)],
        ),
    );
    let brief = plan_brief(
        &daemon,
        repo.path(),
        "Improve schema versioning for the database",
        json!({}),
    );
    let s = section(&brief);
    assert!(s.contains("Migrate database schema versioning"), "{s}");
    assert!(!s.contains("Rename schema helper"), "{s}");
    daemon.shutdown_and_wait();
}

#[test]
fn at_most_three_are_listed_ranked_by_overlap_then_recency_and_never_unfinished_or_eval_ones() {
    let (daemon, _s) = planner_daemon();
    let repo = init_git_repo();
    let done = |title: &'static str, files: &[&str], updated: i64| {
        let mut s = Seed::new(
            title,
            "done",
            vec![attempt("implement", files, None, &[], None)],
        );
        s.updated_at = updated;
        s
    };
    seed(&daemon, repo.path(), done("Old single", &["src/c.rs"], 100));
    seed(
        &daemon,
        repo.path(),
        done("Double overlap", &["src/a.rs", "src/b.rs"], 50),
    );
    seed(
        &daemon,
        repo.path(),
        done("Newest single", &["src/c.rs"], 300),
    );
    seed(
        &daemon,
        repo.path(),
        done("Middle single", &["src/c.rs"], 200),
    );
    let mut queued = done("Still queued", &["src/a.rs"], 400);
    queued.status = "queued";
    seed(&daemon, repo.path(), queued);
    let mut eval = done("Eval replay", &["src/a.rs"], 500);
    eval.eval_set = Some("set1");
    seed(&daemon, repo.path(), eval);
    // Another repo's task is not this repo's history.
    let other = init_git_repo();
    seed(
        &daemon,
        other.path(),
        done("Other repo", &["src/a.rs"], 600),
    );

    let brief = plan_brief(&daemon, repo.path(), "Refactor src/", json!({}));
    let s = section(&brief);
    assert_eq!(s.matches("Past task:").count(), 3, "{s}");
    let at = |t: &str| s.find(t).unwrap_or_else(|| panic!("{t} missing: {s}"));
    assert!(
        at("Double overlap") < at("Newest single") && at("Newest single") < at("Middle single"),
        "{s}"
    );
    for gone in ["Old single", "Still queued", "Eval replay", "Other repo"] {
        assert!(!s.contains(gone), "{gone} listed: {s}");
    }
    daemon.shutdown_and_wait();
}

#[test]
fn an_eval_task_gets_no_past_work_section_even_when_it_names_a_changed_path() {
    let (daemon, _s) = planner_daemon();
    let repo = init_git_repo();
    seed(
        &daemon,
        repo.path(),
        Seed::new(
            "Earlier work",
            "done",
            vec![attempt("implement", &["src/a.rs"], None, &[], None)],
        ),
    );
    let note = daemon.request(
        "repo.notes.add",
        json!({"repo": repo.path().to_str().unwrap(), "text": "Prefer small commits"}),
    );
    assert_eq!(note["source"], "owner");
    let brief = plan_brief(
        &daemon,
        repo.path(),
        "Change src/a.rs",
        json!({"evalSet": "set1", "evalName": "case1"}),
    );
    assert!(!brief.contains("## Past work"), "{brief}");
    assert!(!brief.contains("Earlier work"));
    daemon.shutdown_and_wait();
}

#[test]
fn the_section_stays_within_1500_characters_notes_first_and_entries_whole() {
    let (daemon, _s) = planner_daemon();
    let repo = init_git_repo();
    let repo_str = repo.path().to_str().unwrap();
    for i in 0..2 {
        daemon.request(
            "repo.notes.add",
            json!({"repo": repo_str, "text": format!("Note {i}: {}", "standing guidance ".repeat(22))}),
        );
    }
    for (i, title) in ["First big", "Second big", "Third big"]
        .into_iter()
        .enumerate()
    {
        let mut s = Seed::new(
            title,
            "failed",
            vec![attempt(
                "implement",
                &["src/a.rs"],
                Some("verify"),
                &[("cargo test big", 1)],
                Some(&"long handoff ".repeat(40)),
            )],
        );
        s.updated_at = 100 + i as i64;
        seed(&daemon, repo.path(), s);
    }
    let brief = plan_brief(&daemon, repo.path(), "Change src/a.rs", json!({}));
    let s = section(&brief);
    assert!(s.chars().count() <= 1500, "{} chars", s.chars().count());
    assert!(s.find("Note 0").unwrap() < s.find("Note 1").unwrap());
    let tasks = s.matches("Past task:").count();
    assert!(tasks < 3, "expected entries to be dropped: {s}");
    if let Some(first_task) = s.find("Past task:") {
        assert!(s.find("Note 1").unwrap() < first_task);
    }
    assert_eq!(
        s.matches("<untrusted-data>").count(),
        s.matches("</untrusted-data>").count()
    );
    assert!(s.ends_with("</untrusted-data>"), "cut mid-entry: {s}");
    // Every listed task entry carries its title block and failure kinds.
    assert_eq!(s.matches("failure kinds:").count(), tasks);
    assert_eq!(s.matches("Title --").count(), tasks);
    daemon.shutdown_and_wait();
}

#[test]
fn a_repo_note_added_by_rpc_comes_first_in_the_next_plan_brief() {
    let (daemon, _s) = planner_daemon();
    let repo = init_git_repo();
    let repo_str = repo.path().to_str().unwrap();
    seed(
        &daemon,
        repo.path(),
        Seed::new(
            "Earlier work",
            "done",
            vec![attempt("implement", &["src/a.rs"], None, &[], None)],
        ),
    );
    daemon.request(
        "repo.notes.add",
        json!({"repo": repo_str, "text": "Always run the linter"}),
    );
    let brief = plan_brief(&daemon, repo.path(), "Change src/a.rs", json!({}));
    let s = section(&brief);
    assert!(
        s.find("Always run the linter").unwrap() < s.find("Earlier work").unwrap(),
        "{s}"
    );
    daemon.shutdown_and_wait();
}

#[test]
fn an_oversized_note_keeps_the_section_header_but_is_not_shown() {
    let (daemon, _s) = planner_daemon();
    let repo = init_git_repo();
    let big = "z".repeat(2000);
    daemon.request(
        "repo.notes.add",
        json!({"repo": repo.path().to_str().unwrap(), "text": big}),
    );
    let brief = plan_brief(&daemon, repo.path(), "Change src/a.rs", json!({}));
    let s = section(&brief);
    assert!(s.chars().count() <= 1500, "{s}");
    assert!(!s.contains("zzzz"), "{s}");
    assert!(brief.find("## Request").unwrap() < brief.find("## Past work").unwrap());
    daemon.shutdown_and_wait();
}

#[test]
fn no_notes_and_no_matches_means_no_section() {
    let (daemon, _s) = planner_daemon();
    let repo = init_git_repo();
    let brief = plan_brief(&daemon, repo.path(), "Change src/a.rs", json!({}));
    assert!(!brief.contains("## Past work"));
    daemon.shutdown_and_wait();
}

#[test]
fn repo_notes_round_trip_through_a_subdirectory_and_a_restart() {
    let data = tempfile::tempdir().unwrap();
    let socket = data.path().join("orchd1.sock");
    let child = spawn_orchd_raw(data.path(), &socket, &[]);
    wait_for_socket(&socket);
    let token = read_control_token(data.path());
    let repo = init_git_repo();
    std::fs::create_dir_all(repo.path().join("sub")).unwrap();
    let repo_str = repo.path().to_str().unwrap();

    let note = request_on(
        &socket,
        "repo.notes.add",
        json!({"repo": repo_str, "text": " Keep it small "}),
        Some(&token),
    );
    assert_eq!(note["text"], "Keep it small");
    assert_eq!(note["source"], "owner");
    assert!(note["id"].as_str().is_some_and(|i| !i.is_empty()));
    assert!(note["createdAt"].as_i64().is_some_and(|t| t > 0));
    let id = note["id"].as_str().unwrap().to_string();

    let sub = repo.path().join("sub");
    let listed = request_on(
        &socket,
        "repo.notes.list",
        json!({"repo": sub.to_str().unwrap()}),
        Some(&token),
    );
    assert_eq!(listed["notes"], json!([note]));

    let empty = raw_request_with_params(
        &socket,
        "repo.notes.add",
        json!({"repo": repo_str, "text": "  "}),
        Some(&token),
    );
    assert!(empty["error"].is_object(), "{empty}");
    let unknown = raw_request_with_params(
        &socket,
        "repo.notes.remove",
        json!({"repo": repo_str, "id": "nope"}),
        Some(&token),
    );
    assert!(unknown["error"].is_object(), "{unknown}");

    // Its own file, not settings.json.
    assert!(data.path().join("repo-notes.json").exists());
    let settings = std::fs::read_to_string(data.path().join("settings.json")).unwrap_or_default();
    assert!(!settings.contains("Keep it small"));

    let _ = request_on(&socket, "shutdown", json!({}), Some(&token));
    let _ = wait_for_exit(child, Duration::from_secs(15));
    let socket2 = data.path().join("orchd2.sock");
    let second = spawn_orchd_raw(data.path(), &socket2, &[]);
    wait_for_socket(&socket2);
    let token2 = read_control_token(data.path());
    let after = request_on(
        &socket2,
        "repo.notes.list",
        json!({"repo": repo_str}),
        Some(&token2),
    );
    assert_eq!(after["notes"][0]["id"], id.as_str(), "{after}");

    request_on(
        &socket2,
        "repo.notes.remove",
        json!({"repo": repo_str, "id": id}),
        Some(&token2),
    );
    let gone = request_on(
        &socket2,
        "repo.notes.list",
        json!({"repo": repo_str}),
        Some(&token2),
    );
    assert_eq!(gone["notes"], json!([]));
    let _ = request_on(&socket2, "shutdown", json!({}), Some(&token2));
    let _ = wait_for_exit(second, Duration::from_secs(15));
}

fn approve_case(form: &str, change: &str) -> (Value, Value) {
    let (daemon, _s) = planner_daemon();
    let repo = init_git_repo();
    let root = seed(
        &daemon,
        repo.path(),
        Seed::new(
            "Unrelated",
            "done",
            vec![attempt("implement", &["z.txt"], None, &[], None)],
        ),
    );
    let dir = daemon.data_dir().join("evolution/proposals");
    std::fs::create_dir_all(&dir).unwrap();
    let proposal = json!({
        "id": "11111111-1111-4111-8111-111111111111", "clusterKey": "k", "kind": "waste", "repo": root,
        "track": "repo", "form": form, "change": change, "evidence": "e",
        "metric": "m", "test": "t", "status": "proposed", "createdAt": 1,
    });
    std::fs::write(
        dir.join("11111111-1111-4111-8111-111111111111.json"),
        proposal.to_string(),
    )
    .unwrap();
    let approved = daemon.request(
        "evolution.approve",
        json!({"id": "11111111-1111-4111-8111-111111111111"}),
    );
    assert_eq!(approved["status"], "approved", "{approved}");
    let task_id = approved["taskId"].as_str().unwrap().to_string();
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(20), |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    assert_eq!(settled["status"], "done", "{settled}");
    let notes = daemon.request("repo.notes.list", json!({"repo": root}));
    daemon.shutdown_and_wait();
    (approved, notes)
}

#[test]
fn approving_a_doc_or_command_proposal_adds_a_note_and_still_starts_the_task() {
    for form in ["doc", "command"] {
        let (_, notes) = approve_case(form, "Document the release steps");
        let list = notes["notes"].as_array().unwrap();
        assert_eq!(list.len(), 1, "{form}: {notes}");
        assert_eq!(list[0]["text"], "Document the release steps");
        assert_eq!(
            list[0]["source"],
            "proposal:11111111-1111-4111-8111-111111111111"
        );
    }
}

#[test]
fn approving_a_script_proposal_adds_no_note() {
    let (_, notes) = approve_case("script", "Add scripts/x.sh");
    assert_eq!(notes["notes"], json!([]), "{notes}");
}
