//! Black-box integration tests (feature report and leadTouch): a finished
//! top-level task or graph leaves a deterministic markdown report, and a
//! landing that was rewritten or edited by someone else marks the task
//! touched. Real daemon, real git repo.

mod common;

use common::*;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

const GRAPH_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
out() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
report='"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"handoff\":\"Wire the docs page next\",\"decisions\":[],\"question\":\"\"}\n```"'
case "$input" in
  *sushi-plan*)
    case "$input" in
      *PART_A*) out '"```sushi-plan\n{\"title\":\"Part A\",\"goal\":\"PART_A: create a.txt\",\"criteria\":[\"a.txt exists\",\"a.txt is documented\"],\"verify\":[\"test -f a.txt\"]}\n```"' ;;
      *PART_B*) out '"```sushi-plan\n{\"title\":\"Part B\",\"goal\":\"PART_B: create b.txt\",\"criteria\":[\"b.txt exists\"],\"verify\":[\"test -f b.txt\"]}\n```"' ;;
      *) out '"```sushi-plan\n{\"title\":\"Both parts\",\"goal\":\"Create a.txt and b.txt\",\"criteria\":[\"both files exist\"],\"verify\":[\"test -f a.txt && test -f b.txt\"],\"subtasks\":[{\"key\":\"a\",\"title\":\"Part A\",\"request\":\"PART_A create a.txt\"},{\"key\":\"b\",\"title\":\"Part B\",\"request\":\"PART_B create b.txt\"}]}\n```"' ;;
    esac
    ;;
  *PART_A*) echo a > a.txt; out "$report" ;;
  *PART_B*) echo b > b.txt; out "$report" ;;
esac
"#;

fn review_off(daemon: &Daemon) {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["answerPolicy"] = json!(false);
    daemon.request("settings.set", json!({"settings": settings}));
}

fn task_json_path(daemon: &Daemon, id: &str) -> std::path::PathBuf {
    daemon.data_dir().join("tasks").join(id).join("task.json")
}

#[test]
fn a_done_graph_gets_a_report_with_children_assumption_dropped_criterion_and_costs() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", GRAPH_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    // The parent's verify fails on the base by design: the policy keeps it.
    let mut settings = daemon.request("settings.get", json!({}));
    settings["answerPolicy"] = json!(true);
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let parent = daemon.request(
        "task.create",
        json!({"repo": repo.path().to_str().unwrap(), "request": "build both parts", "start": true}),
    );
    let parent_id = parent["id"].as_str().unwrap().to_string();
    let parent = settle(&daemon, &parent_id);
    assert_eq!(parent["status"], "done", "{parent}");

    // Written when the graph finished.
    let report_md = daemon
        .data_dir()
        .join("tasks")
        .join(&parent_id)
        .join("report.md");
    let first = std::fs::read_to_string(&report_md).unwrap();
    assert!(first.starts_with("# Both parts"), "{first}");
    assert_eq!(parent["report"], first.as_str());
    assert!(parent["reportAt"].is_number());

    // Give the first child an assumption and a dropped criterion.
    let tasks = daemon.request("task.list", json!({"repo": parent["repo"]}));
    let child = tasks
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == "Part A")
        .unwrap();
    let path = task_json_path(&daemon, child["id"].as_str().unwrap());
    let mut stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    stored["assumptions"] = json!([{
        "question": "Which format for a.txt?",
        "answer": "plain text",
        "evidence": "the repo only has text files",
        "by": "judge",
        "overturned": false
    }]);
    stored["decisions"].as_array_mut().unwrap().push(json!(
        "Orchestrator: dropped criterion 1 (a.txt has a man page) as the owner answered"
    ));
    std::fs::write(&path, serde_json::to_string(&stored).unwrap()).unwrap();

    let via_rpc = daemon.request("task.report", json!({"id": parent_id}));
    let report = via_rpc["report"].as_str().unwrap();
    assert_eq!(std::fs::read_to_string(&report_md).unwrap(), report);
    for needle in [
        "# Both parts",
        "**Part A**",
        "**Part B**",
        "Which format for a.txt? -> plain text (answered by the cheap judge; overturnable",
        "- a.txt has a man page - dropped",
        "- a.txt exists - unchecked",
        "| plan |",
        "| implement |",
        "## Attempts",
        "- Part B: 1 attempt",
        "Wire the docs page next",
        "files changed",
        "`a.txt` +1 -0",
    ] {
        assert!(report.contains(needle), "missing {needle:?} in:\n{report}");
    }
    assert!(report.contains("Lead touch: unknown"), "{report}");

    let err = daemon.request_error("task.report", json!({"id": child["id"]}));
    assert!(err.contains("top-level"), "{err}");
    daemon.shutdown_and_wait();
}

fn landing_daemon() -> (Daemon, tempfile::TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts.path(),
        "fake-claude.sh",
        r#"#!/bin/sh
cat > /dev/null
echo landed > landed.txt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#,
    );
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    review_off(&daemon);
    (daemon, scripts)
}

fn land_one(daemon: &Daemon, repo: &Path) -> serde_json::Value {
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.to_str().unwrap(),
            "title": "Add landed",
            "goal": "write landed.txt",
            "criteria": ["landed.txt exists"],
            "verify": ["test -f landed.txt"],
            "land": true,
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let done = poll_until(daemon, &id, Duration::from_secs(60), |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    assert_eq!(done["status"], "done", "{done}");
    assert!(done["landedSha"].is_string(), "{done}");
    done
}

#[test]
fn lead_touch_is_set_and_cleared_by_the_owner_and_a_rewritten_landing_marks_it_by_auto() {
    let (daemon, _scripts) = landing_daemon();
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    let base = git_out(repo.path(), &["rev-parse", "HEAD"]);
    let done = land_one(&daemon, repo.path());
    let id = done["id"].as_str().unwrap().to_string();
    assert!(done["report"]
        .as_str()
        .unwrap()
        .contains("landed on `work`"));
    // A single-task report names its task in the heading, not on each line.
    let report = done["report"].as_str().unwrap();
    assert!(!report.contains("Add landed:"), "{report}");
    assert!(report.contains("\n- 1 attempt"), "{report}");
    assert!(done["leadTouch"].is_null(), "{done}");

    // A clean landing stays unmarked.
    let rep = daemon.request("task.report", json!({"id": id}));
    assert!(rep["report"]
        .as_str()
        .unwrap()
        .contains("Lead touch: unknown"));
    assert!(daemon.request("task.get", json!({"id": id}))["leadTouch"].is_null());

    // The owner sets and clears it.
    let set = daemon.request(
        "task.leadTouch",
        json!({"id": id, "touched": true, "note": "fixed the docs"}),
    );
    assert_eq!(set["leadTouch"]["touched"], true);
    assert_eq!(set["leadTouch"]["by"], "owner");
    assert_eq!(set["leadTouch"]["note"], "fixed the docs");
    assert!(set["report"]
        .as_str()
        .unwrap()
        .contains("needed a fix (by owner): fixed the docs"));
    let summary = daemon.request("costs.summary", json!({"groupBy": ["stage"]}));
    assert_eq!(summary["leadTouch"]["marked"], 1, "{summary}");
    assert_eq!(summary["leadTouch"]["rate"], 1.0);
    assert_eq!(summary["leadTouch"]["byRepo"][0]["touched"], 1);
    let clean = daemon.request("task.leadTouch", json!({"id": id, "touched": false}));
    assert_eq!(clean["leadTouch"]["touched"], false);
    let summary = daemon.request("costs.summary", json!({}));
    assert_eq!(summary["leadTouch"]["rate"], 0.0);
    let cleared = daemon.request("task.leadTouch", json!({"id": id}));
    assert!(cleared["leadTouch"].is_null(), "{cleared}");

    // The landing is rewritten: work goes back to where it began.
    git_out(repo.path(), &["reset", "-q", "--hard", &base]);
    let rep = daemon.request("task.report", json!({"id": id}));
    assert!(
        rep["report"]
            .as_str()
            .unwrap()
            .contains("needed a fix (by auto)"),
        "{rep}"
    );
    let got = daemon.request("task.get", json!({"id": id}));
    assert_eq!(got["leadTouch"]["touched"], true);
    assert_eq!(got["leadTouch"]["by"], "auto");
    assert!(got["leadTouch"]["note"]
        .as_str()
        .unwrap()
        .contains("rewritten or reverted"));

    // The owner's word wins over the automatic mark.
    let clean = daemon.request("task.leadTouch", json!({"id": id, "touched": false}));
    assert_eq!(clean["leadTouch"]["by"], "owner");
    daemon.request("task.report", json!({"id": id}));
    assert_eq!(
        daemon.request("task.get", json!({"id": id}))["leadTouch"]["touched"],
        false
    );
    daemon.shutdown_and_wait();
}

#[test]
fn a_foreign_commit_on_a_landed_file_marks_the_task_touched_on_report() {
    let (daemon, _scripts) = landing_daemon();
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    let done = land_one(&daemon, repo.path());
    let id = done["id"].as_str().unwrap().to_string();
    // An unrelated file does not count.
    std::fs::write(repo.path().join("other.txt"), "x\n").unwrap();
    git_out(repo.path(), &["add", "."]);
    git_out(repo.path(), &["commit", "-q", "-m", "other"]);
    daemon.request("task.report", json!({"id": id}));
    assert!(daemon.request("task.get", json!({"id": id}))["leadTouch"].is_null());
    // A person's edit of the task's file does.
    std::fs::write(repo.path().join("landed.txt"), "fixed by hand\n").unwrap();
    git_out(repo.path(), &["commit", "-q", "-am", "hand fix"]);
    daemon.request("task.report", json!({"id": id}));
    let got = daemon.request("task.get", json!({"id": id}));
    assert_eq!(got["leadTouch"]["touched"], true, "{got}");
    assert!(got["leadTouch"]["note"]
        .as_str()
        .unwrap()
        .contains("not made by orchd"));
    daemon.shutdown_and_wait();
}

fn listed(daemon: &Daemon, done: &serde_json::Value) -> usize {
    daemon
        .request("task.list", json!({"repo": done["repo"]}))
        .as_array()
        .unwrap()
        .len()
}

fn follow_ups_of(task: &serde_json::Value) -> Vec<String> {
    task["followUps"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn decisions_mention(task: &serde_json::Value, id: &str) -> bool {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d.as_str().unwrap().contains(id))
}

#[test]
fn an_owner_mark_with_a_note_creates_one_follow_up_from_the_landed_branch() {
    let (daemon, _scripts) = landing_daemon();
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    let done = land_one(&daemon, repo.path());
    let id = done["id"].as_str().unwrap().to_string();
    let landed = done["landedSha"].as_str().unwrap().to_string();
    let before = listed(&daemon, &done);

    let set = daemon.request(
        "task.leadTouch",
        json!({"id": id, "touched": true, "note": "  the docs page is missing  "}),
    );
    assert_eq!(set["leadTouch"]["touched"], true, "{set}");
    assert_eq!(set["leadTouch"]["by"], "owner", "{set}");
    assert_eq!(
        set["leadTouch"]["note"], "the docs page is missing",
        "{set}"
    );
    let ups = follow_ups_of(&set);
    assert_eq!(ups.len(), 1, "{set}");
    assert_eq!(listed(&daemon, &done), before + 1);
    let up = daemon.request("task.get", json!({"id": ups[0]}));
    assert_eq!(up["followUpOf"], id.as_str());
    assert_eq!(up["repo"], done["repo"]);
    assert!(up["parent"].is_null(), "{up}");
    assert_eq!(up["variant"]["land"], true, "{up}");
    assert_eq!(up["variant"], done["variant"]);
    let request = up["request"].as_str().unwrap();
    assert!(
        request.starts_with("the docs page is missing\n\n"),
        "{request}"
    );
    for needle in [
        "Add landed",
        "write landed.txt",
        "landed.txt exists",
        "## Criteria",
        "- landed.txt exists - ",
        "## What changed",
        "`landed.txt`",
        "## Follow-ups",
    ] {
        assert!(
            request.contains(needle),
            "missing {needle:?} in:\n{request}"
        );
    }
    let at = |s: &str| request.find(s).unwrap();
    assert!(
        at("Add landed") > "the docs page is missing".len(),
        "{request}"
    );
    assert!(at("## What changed") > at("Add landed"), "{request}");
    assert!(at("## Criteria") > at("## What changed"), "{request}");
    assert!(at("## Follow-ups") > at("## Criteria"), "{request}");
    assert_eq!(up["baseRef"], "work", "{up}");
    let base_sha = up["baseSha"].as_str().unwrap();
    assert_eq!(
        git_out(
            repo.path(),
            &["merge-base", "--is-ancestor", &landed, base_sha]
        ),
        ""
    );
    let original = daemon.request("task.get", json!({"id": id}));
    assert!(decisions_mention(&original, &ups[0]), "{original}");
    assert!(decisions_mention(&up, &id), "{up}");
    assert!(decisions_mention(&original, "docs page is missing"));
    daemon.shutdown_and_wait();
}

#[test]
fn a_landing_reset_away_gives_a_follow_up_started_from_the_original_branch() {
    let (daemon, _scripts) = landing_daemon();
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    let base = git_out(repo.path(), &["rev-parse", "HEAD"]);
    let done = land_one(&daemon, repo.path());
    let id = done["id"].as_str().unwrap().to_string();
    let landed = done["landedSha"].as_str().unwrap().to_string();
    git_out(repo.path(), &["reset", "-q", "--hard", &base]);

    let set = daemon.request(
        "task.leadTouch",
        json!({"id": id, "touched": true, "note": "redo it"}),
    );
    let ups = follow_ups_of(&set);
    assert_eq!(ups.len(), 1, "{set}");
    let up = daemon.request("task.get", json!({"id": ups[0]}));
    assert_eq!(up["baseRef"], done["branch"], "{up}");
    let base_sha = up["baseSha"].as_str().unwrap();
    assert_eq!(
        git_out(
            repo.path(),
            &["rev-parse", done["branch"].as_str().unwrap()]
        ),
        base_sha
    );
    let on_work = std::process::Command::new("git")
        .current_dir(repo.path())
        .args(["merge-base", "--is-ancestor", &landed, "work"])
        .status()
        .unwrap();
    assert!(!on_work.success());
    daemon.shutdown_and_wait();
}

#[test]
fn follow_ups_are_idempotent_per_note_and_survive_clearing_the_mark() {
    let (daemon, _scripts) = landing_daemon();
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    let done = land_one(&daemon, repo.path());
    let id = done["id"].as_str().unwrap().to_string();
    let base = git_out(repo.path(), &["rev-parse", "HEAD~1"]);
    let count = || listed(&daemon, &done);
    let start = count();

    // Nothing that is not an owner's touched mark with a note creates a task.
    for params in [
        json!({"id": id, "touched": false, "note": "looks wrong"}),
        json!({"id": id, "touched": true}),
        json!({"id": id, "touched": true, "note": "   "}),
        json!({"id": id, "note": "looks wrong"}),
    ] {
        let got = daemon.request("task.leadTouch", params);
        assert!(follow_ups_of(&got).is_empty(), "{got}");
        assert_eq!(count(), start);
    }
    git_out(repo.path(), &["reset", "-q", "--hard", &base]);
    daemon.request("task.report", json!({"id": id}));
    let got = daemon.request("task.get", json!({"id": id}));
    assert_eq!(got["leadTouch"]["by"], "auto");
    assert!(follow_ups_of(&got).is_empty(), "{got}");
    assert_eq!(count(), start);

    let first = daemon.request(
        "task.leadTouch",
        json!({"id": id, "touched": true, "note": "fix A"}),
    );
    assert_eq!(follow_ups_of(&first).len(), 1);
    assert_eq!(count(), start + 1);
    let again = daemon.request(
        "task.leadTouch",
        json!({"id": id, "touched": true, "note": " fix A "}),
    );
    assert_eq!(follow_ups_of(&again), follow_ups_of(&first));
    assert_eq!(count(), start + 1);

    let cleared = daemon.request("task.leadTouch", json!({"id": id}));
    assert_eq!(follow_ups_of(&cleared).len(), 1, "{cleared}");
    let clean = daemon.request("task.leadTouch", json!({"id": id, "touched": false}));
    assert_eq!(follow_ups_of(&clean).len(), 1);
    assert_eq!(count(), start + 1);
    let remark = daemon.request(
        "task.leadTouch",
        json!({"id": id, "touched": true, "note": "fix A"}),
    );
    assert_eq!(follow_ups_of(&remark).len(), 1);
    assert_eq!(count(), start + 1);

    let other = daemon.request(
        "task.leadTouch",
        json!({"id": id, "touched": true, "note": "fix B"}),
    );
    let ups = follow_ups_of(&other);
    assert_eq!(ups.len(), 2, "{other}");
    assert_eq!(count(), start + 2);
    for up in &ups {
        assert_eq!(
            daemon.request("task.get", json!({"id": up}))["followUpOf"],
            id.as_str()
        );
    }
    daemon.shutdown_and_wait();
}
