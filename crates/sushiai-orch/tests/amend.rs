//! Black-box integration tests for `task.amend`: the amendment of a task with
//! a live loop reaches the loop, grounded checks are baselined on the base,
//! and dependency replacements are honored before implementation.

mod common;

use common::repository::git_out;

use common::harness::fake_harness_script;
use common::polling::{poll_task_status, poll_until, remove_dir_retrying, wait_until};
use common::process::Daemon;
use common::repository::init_git_repo;

use serde_json::json;
use std::path::Path;
use std::time::Duration;

fn dependency_daemon(scripts: &Path) -> Daemon {
    let script = fake_harness_script(
        scripts,
        "fake-claude.sh",
        r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *sushi-plan*)
    touch "$AMEND_GATE_DIR/plan-started"
    while [ ! -f "$AMEND_GATE_DIR/plan-go" ]; do sleep 0.1; done
    printf '%s\n' '{"type":"result","result":"```sushi-plan\n{\"title\":\"Planned task\",\"goal\":\"Make a change\",\"verify\":[\"test -f CHANGED_MARKER.txt\"]}\n```"}'
    exit 0 ;;
  *AMEND_BLOCK_DEP*)
    touch "$AMEND_GATE_DIR/dependency-started"
    while [ ! -f "$AMEND_GATE_DIR/dependency-go" ]; do sleep 0.1; done ;;
  *AMEND_BLOCK_SLOT*)
    touch "$AMEND_GATE_DIR/slot-started"
    while [ ! -f "$AMEND_GATE_DIR/slot-go" ]; do sleep 0.1; done ;;
esac
echo changed > CHANGED_MARKER.txt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#,
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("AMEND_GATE_DIR", scripts.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["parallel"] = json!(2);
    daemon.request("settings.set", json!({"settings": settings}));
    daemon
}

fn dependency_task(
    daemon: &Daemon,
    repo: &Path,
    title: &str,
    goal: &str,
    start: bool,
) -> serde_json::Value {
    daemon.request(
        "task.create",
        json!({
            "repo": repo.to_str().unwrap(), "title": title, "goal": goal,
            "verify": ["test -f CHANGED_MARKER.txt"], "start": start,
        }),
    )
}

fn assert_dependency_decisions(task: &serde_json::Value, waits: &str) {
    let decisions = task["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(|d| d
            .as_str()
            .is_some_and(|d| d.starts_with("Amended:") && d.contains("dependsOn"))),
        "{task}"
    );
    assert!(decisions.contains(&json!(waits)), "{task}");
}

#[test]
fn depends_on_amend_drafting_task_keeps_the_replacement_when_planning_stops_for_review() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = dependency_daemon(scripts.path());
    let repo = init_git_repo();
    let dependency = dependency_task(
        &daemon,
        repo.path(),
        "Plan dependency",
        "AMEND_BLOCK_DEP",
        true,
    );
    wait_until("the dependency to run", || {
        scripts
            .path()
            .join("dependency-started")
            .exists()
            .then_some(())
    });
    let task = daemon.request("task.create", json!({
        "repo": repo.path().to_str().unwrap(), "request": "Make a planned change", "start": false,
    }));
    wait_until("the planner to run", || {
        scripts.path().join("plan-started").exists().then_some(())
    });
    let amended = daemon.request(
        "task.amend",
        json!({"id": task["id"], "dependsOn": [dependency["id"]]}),
    );
    assert_eq!(amended["pending"], true, "{amended}");
    std::fs::write(scripts.path().join("plan-go"), "").unwrap();
    let stopped = wait_until("planning to stop for review", || {
        let t = daemon.request("task.get", json!({"id": task["id"]}));
        (t["status"] == "stopped" && t["dependsOn"] == json!([dependency["id"]])).then_some(t)
    });
    assert_eq!(stopped["dependsOn"], json!([dependency["id"]]), "{stopped}");
    assert!(
        stopped["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["stage"] == "plan"),
        "{stopped}"
    );
    assert_dependency_decisions(&stopped, "Dependencies: waits for \"Plan dependency\"");
    std::fs::write(scripts.path().join("dependency-go"), "").unwrap();
    assert_eq!(
        poll_task_status(&daemon, dependency["id"].as_str().unwrap())["status"],
        "done"
    );
    assert_eq!(
        daemon.request("task.get", json!({"id": task["id"]}))["status"],
        "stopped"
    );
    daemon.shutdown_and_wait();
    for t in [&task, &dependency] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn depends_on_amend_queued_task_waits_for_a_running_dependency_then_starts_itself() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = dependency_daemon(scripts.path());
    let repo = init_git_repo();
    let dependency = dependency_task(&daemon, repo.path(), "Dependency", "AMEND_BLOCK_DEP", true);
    wait_until("the dependency to run", || {
        scripts
            .path()
            .join("dependency-started")
            .exists()
            .then_some(())
    });
    let task = dependency_task(&daemon, repo.path(), "Amended task", "Make a change", false);
    let amended = daemon.request(
        "task.amend",
        json!({"id": task["id"], "dependsOn": [dependency["id"]]}),
    );
    assert_eq!(amended["pending"], false, "{amended}");

    std::thread::sleep(Duration::from_millis(500));
    let waiting = daemon.request("task.get", json!({"id": task["id"]}));
    assert_eq!(waiting["status"], "queued", "{waiting}");
    assert!(
        waiting["attempts"].as_array().unwrap().is_empty(),
        "{waiting}"
    );
    assert_eq!(waiting["dependsOn"], json!([dependency["id"]]));
    assert_dependency_decisions(&waiting, "Dependencies: waits for \"Dependency\"");
    assert_eq!(
        daemon.request("task.get", json!({"id": dependency["id"]}))["status"],
        "running"
    );

    std::fs::write(scripts.path().join("dependency-go"), "").unwrap();
    let dependency_done = poll_task_status(&daemon, dependency["id"].as_str().unwrap());
    assert_eq!(dependency_done["status"], "done", "{dependency_done}");
    let done = poll_task_status(&daemon, task["id"].as_str().unwrap());
    assert_eq!(done["status"], "done", "{done}");
    assert!(
        done["attempts"][0]["startedAt"].as_i64().unwrap()
            >= dependency_done["updatedAt"].as_i64().unwrap(),
        "{done} started before {dependency_done} reached done"
    );
    assert_dependency_decisions(&done, "Dependencies: waits for \"Dependency\"");
    daemon.shutdown_and_wait();
    for t in [&task, &dependency] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

/// Commits a setup that fails only in the failing task's worktree until
/// `fixed` exists: the task ends `failed` and can be requeued once it does.
fn commit_failing_setup(repo: &Path, fixed: &Path) {
    let setup = json!({"setup": format!(
        "case \"$ORCHD_WORKTREE_PATH\" in *failing-task*) test -f {} || {{ echo setup-broke >&2; exit 4; }};; esac",
        fixed.display()
    )});
    std::fs::create_dir_all(repo.join(".sushiai")).unwrap();
    std::fs::write(repo.join(".sushiai/setup.json"), setup.to_string()).unwrap();
    for args in [
        &["add", "-f", "."][..],
        &["commit", "-q", "-m", "add setup"][..],
    ] {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }
}

#[test]
fn depends_on_amend_failed_task_waits_for_a_dependency_then_starts_itself() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = dependency_daemon(scripts.path());
    let repo = init_git_repo();
    let fixed = scripts.path().join("fixed");
    commit_failing_setup(repo.path(), &fixed);
    let failing = dependency_task(&daemon, repo.path(), "Failing task", "Make a change", true);
    let failed = poll_until(&daemon, failing["id"].as_str().unwrap(), |s| {
        matches!(s, "failed" | "done" | "waiting")
    });
    assert_eq!(failed["status"], "failed", "{failed}");

    let dependency = dependency_task(&daemon, repo.path(), "Dependency", "AMEND_BLOCK_DEP", true);
    wait_until("the dependency to run", || {
        scripts
            .path()
            .join("dependency-started")
            .exists()
            .then_some(())
    });
    // The fix the dependency stands for: from now on the setup passes.
    std::fs::write(&fixed, "").unwrap();
    let amended = daemon.request(
        "task.amend",
        json!({"id": failing["id"], "dependsOn": [dependency["id"]]}),
    );
    assert_eq!(amended["pending"], false, "{amended}");

    std::thread::sleep(Duration::from_millis(500));
    let waiting = daemon.request("task.get", json!({"id": failing["id"]}));
    assert_eq!(waiting["status"], "queued", "{waiting}");
    assert_eq!(waiting["dependsOn"], json!([dependency["id"]]), "{waiting}");
    assert_dependency_decisions(&waiting, "Dependencies: waits for \"Dependency\"");
    assert!(
        waiting["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["status"] != "running"),
        "{waiting}"
    );
    let attempts_before = waiting["attempts"].as_array().unwrap().len();

    std::fs::write(scripts.path().join("dependency-go"), "").unwrap();
    let dependency_done = poll_until(&daemon, dependency["id"].as_str().unwrap(), |s| {
        matches!(s, "done" | "failed")
    });
    assert_eq!(dependency_done["status"], "done", "{dependency_done}");
    let done = poll_until(&daemon, failing["id"].as_str().unwrap(), |s| {
        matches!(s, "done" | "failed" | "waiting")
    });
    assert_eq!(done["status"], "done", "{done}");
    let attempts = done["attempts"].as_array().unwrap();
    assert!(attempts.len() > attempts_before, "{done}");
    assert!(
        attempts[attempts_before]["startedAt"].as_i64().unwrap()
            >= dependency_done["updatedAt"].as_i64().unwrap(),
        "{done} started before {dependency_done} reached done"
    );
    daemon.shutdown_and_wait();
    for t in [&failing, &dependency] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn depends_on_amend_empty_list_starts_without_waiting_for_the_old_dependency() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = dependency_daemon(scripts.path());
    let repo = init_git_repo();
    let dependency = dependency_task(
        &daemon,
        repo.path(),
        "Old dependency",
        "AMEND_BLOCK_DEP",
        true,
    );
    wait_until("the dependency to run", || {
        scripts
            .path()
            .join("dependency-started")
            .exists()
            .then_some(())
    });
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(), "title": "Remove wait", "goal": "Make a change",
            "verify": ["test -f CHANGED_MARKER.txt"], "dependsOn": [dependency["id"]],
        }),
    );
    std::thread::sleep(Duration::from_millis(500));
    let waiting = daemon.request("task.get", json!({"id": task["id"]}));
    assert_eq!(waiting["status"], "queued", "{waiting}");
    assert!(
        waiting["attempts"].as_array().unwrap().is_empty(),
        "{waiting}"
    );

    let amended = daemon.request("task.amend", json!({"id": task["id"], "dependsOn": []}));
    assert_eq!(amended["pending"], false, "{amended}");
    let done = poll_task_status(&daemon, task["id"].as_str().unwrap());
    assert_eq!(done["status"], "done", "{done}");
    assert!(done.get("dependsOn").is_none(), "{done}");
    assert_dependency_decisions(&done, "Dependencies: waits for nothing");
    let unfinished = daemon.request("task.get", json!({"id": dependency["id"]}));
    assert_eq!(unfinished["status"], "running", "{unfinished}");
    assert!(!scripts.path().join("dependency-go").exists());
    std::fs::write(scripts.path().join("dependency-go"), "").unwrap();
    assert_eq!(
        poll_task_status(&daemon, dependency["id"].as_str().unwrap())["status"],
        "done"
    );
    daemon.shutdown_and_wait();
    for t in [&task, &dependency] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn depends_on_amend_live_loop_rechecks_the_graph_before_its_first_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = dependency_daemon(scripts.path());
    let repo = init_git_repo();
    let dependency = dependency_task(
        &daemon,
        repo.path(),
        "Live dependency",
        "AMEND_BLOCK_DEP",
        true,
    );
    let holder = dependency_task(
        &daemon,
        repo.path(),
        "Slot holder",
        "AMEND_BLOCK_SLOT",
        true,
    );
    wait_until("both slots to be held", || {
        (scripts.path().join("dependency-started").exists()
            && scripts.path().join("slot-started").exists())
        .then_some(())
    });
    let task = dependency_task(
        &daemon,
        repo.path(),
        "Live amended task",
        "Make a change",
        true,
    );
    std::thread::sleep(Duration::from_millis(500));
    let queued = daemon.request("task.get", json!({"id": task["id"]}));
    assert_eq!(queued["status"], "queued", "{queued}");
    assert!(
        queued["attempts"].as_array().unwrap().is_empty(),
        "{queued}"
    );
    let amended = daemon.request(
        "task.amend",
        json!({"id": task["id"], "dependsOn": [dependency["id"]]}),
    );
    assert_eq!(amended["pending"], true, "{amended}");
    std::fs::write(scripts.path().join("slot-go"), "").unwrap();
    assert_eq!(
        poll_task_status(&daemon, holder["id"].as_str().unwrap())["status"],
        "done"
    );
    let applied = wait_until("the live loop to apply its dependencies", || {
        let t = daemon.request("task.get", json!({"id": task["id"]}));
        (t["dependsOn"] == json!([dependency["id"]]) && t["status"] == "queued").then_some(t)
    });
    assert_dependency_decisions(&applied, "Dependencies: waits for \"Live dependency\"");
    std::thread::sleep(Duration::from_millis(500));
    let waiting = daemon.request("task.get", json!({"id": task["id"]}));
    assert_eq!(waiting["status"], "queued", "{waiting}");
    assert!(
        waiting["attempts"].as_array().unwrap().is_empty(),
        "{waiting}"
    );
    assert_eq!(
        daemon.request("task.get", json!({"id": dependency["id"]}))["status"],
        "running"
    );
    std::fs::write(scripts.path().join("dependency-go"), "").unwrap();
    let dependency_done = poll_task_status(&daemon, dependency["id"].as_str().unwrap());
    assert_eq!(dependency_done["status"], "done", "{dependency_done}");
    let done = poll_task_status(&daemon, task["id"].as_str().unwrap());
    assert_eq!(done["status"], "done", "{done}");
    assert!(
        done["attempts"][0]["startedAt"].as_i64().unwrap()
            >= dependency_done["updatedAt"].as_i64().unwrap(),
        "{done} started before {dependency_done} reached done"
    );
    daemon.shutdown_and_wait();
    for t in [&task, &dependency, &holder] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn depends_on_amend_clears_an_ended_dependency_question_and_starts_itself() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = dependency_daemon(scripts.path());
    let repo = init_git_repo();
    let dependency = dependency_task(
        &daemon,
        repo.path(),
        "Stopped dependency",
        "AMEND_BLOCK_DEP",
        true,
    );
    wait_until("the dependency to run", || {
        scripts
            .path()
            .join("dependency-started")
            .exists()
            .then_some(())
    });
    daemon.request("task.stop", json!({"id": dependency["id"]}));
    let stopped = poll_task_status(&daemon, dependency["id"].as_str().unwrap());
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    let task = daemon.request("task.create", json!({
        "repo": repo.path().to_str().unwrap(), "title": "Remove ended dependency", "goal": "Make a change",
        "verify": ["test -f CHANGED_MARKER.txt"], "dependsOn": [dependency["id"]],
    }));
    let waiting = poll_task_status(&daemon, task["id"].as_str().unwrap());
    assert_eq!(waiting["status"], "waiting", "{waiting}");
    assert!(
        waiting["question"]["text"]
            .as_str()
            .unwrap()
            .starts_with("A task this one waits for ended"),
        "{waiting}"
    );
    let amended = daemon.request("task.amend", json!({"id": task["id"], "dependsOn": []}));
    assert_eq!(amended["pending"], false, "{amended}");
    let done = poll_task_status(&daemon, task["id"].as_str().unwrap());
    assert_eq!(done["status"], "done", "{done}");
    assert!(done["question"].is_null(), "{done}");
    assert_dependency_decisions(&done, "Dependencies: waits for nothing");
    assert_eq!(
        daemon.request("task.get", json!({"id": dependency["id"]}))["status"],
        "stopped"
    );
    daemon.shutdown_and_wait();
    for t in [&task, &dependency] {
        remove_dir_retrying(t["worktree"].as_str().unwrap());
    }
}

#[test]
fn an_amendment_during_an_attempt_reaches_the_next_review_brief() {
    let scripts = tempfile::tempdir().unwrap();
    let started = scripts.path().join("started");
    let go = scripts.path().join("go");
    let claude = fake_harness_script(
        scripts.path(),
        "fake-claude.sh",
        &format!(
            "#!/bin/sh\ncat > /dev/null\necho changed > CHANGED_MARKER.txt\ntouch {started}\nwhile [ ! -f {go} ]; do sleep 0.1; done\nprintf '%s\\n' '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"sess-fake\"}}'\nprintf '%s\\n' '{{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}},\"result\":\"```sushi-report\\n{{\\\\\"outcome\\\\\":\\\\\"complete\\\\\",\\\\\"summary\\\\\":\\\\\"done\\\\\",\\\\\"decisions\\\\\":[],\\\\\"question\\\\\":\\\\\"\\\\\"}}\\n```\"}}'\n",
            started = started.display(),
            go = go.display()
        ),
    );
    let brief_log = scripts.path().join("codex-brief");
    let codex = fake_harness_script(
        scripts.path(),
        "fake-codex.sh",
        "#!/bin/sh\ncat > \"$BRIEF_LOG\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"```sushi-review\\n{\\\"verdict\\\":\\\"PASS\\\",\\\"findings\\\":[]}\\n```\"}}'\n",
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("ORCHD_CODEX_BIN", codex.to_str().unwrap()),
        ("BRIEF_LOG", brief_log.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("codex");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Amend",
            "goal": "Write a marker",
            "criteria": ["the OLDCRITERION holds"],
            "verify": ["true"],
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    wait_until("the attempt to begin", || started.exists().then_some(()));
    let amended = daemon.request(
        "task.amend",
        json!({"id": id, "criteria": ["the NEWCRITERION holds"]}),
    );
    assert_eq!(amended["pending"], true, "{amended}");
    std::fs::write(&go, "").unwrap();

    let done = poll_task_status(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    let brief = std::fs::read_to_string(&brief_log).unwrap();
    assert!(brief.contains("NEWCRITERION"), "{brief}");
    assert!(!brief.contains("OLDCRITERION"), "{brief}");
    assert_eq!(done["criteria"], json!(["the NEWCRITERION holds"]));
    assert!(done["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d.as_str().unwrap().starts_with("Amended: criteria")));

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn amended_checks_are_baselined_on_the_base_and_only_failing_ones_gate() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Grounded amend",
            "goal": "Something",
            "criteria": ["a", "b"],
                        "start": false,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let result = daemon.request(
        "task.amend",
        json!({
            "id": id,
            "checks": [
                {"criterion": 0, "run": "test -f NOPE.txt"},
                {"criterion": 1, "run": "test -f README.md"},
            ],
            "heldOut": {"criterion": 0, "run": "test -f NOPE2.txt"},
        }),
    );
    assert_eq!(result["pending"], false, "{result}");
    let saved = daemon.request("task.get", json!({"id": id}));
    assert_eq!(saved["checks"][0]["baseline"], "fail", "{saved}");
    assert_eq!(saved["checks"][1]["baseline"], "pass", "{saved}");
    assert_eq!(saved["heldOut"]["baseline"], "fail", "{saved}");
    let decisions: Vec<&str> = saved["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect();
    assert!(
        decisions.contains(&"Amended: checks, heldOut"),
        "{decisions:?}"
    );
    assert!(
        decisions
            .iter()
            .any(|d| d.contains("already passes on base")),
        "{decisions:?}"
    );
    assert!(
        decisions.iter().all(|d| !d.contains("NOPE2")),
        "the held-out command never appears: {decisions:?}"
    );
    assert_eq!(
        git_out(repo.path(), &["worktree", "list"]).lines().count(),
        2
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn amending_criteria_replaces_the_visual_flags_with_the_amended_ones() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Visual amend",
            "goal": "Something",
            "criteria": ["old"],
            "verify": ["true"],
            "start": false,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();

    daemon.request("task.amend", json!({"id": id, "criteria": ["a", "b"]}));
    let saved = daemon.request("task.get", json!({"id": id}));
    assert!(saved.get("visualCriteria").is_none(), "{saved}");

    daemon.request(
        "task.amend",
        json!({"id": id, "criteria": ["a", {"text": "b", "visual": true}]}),
    );
    let saved = daemon.request("task.get", json!({"id": id}));
    assert_eq!(saved["criteria"], json!(["a", "b"]), "{saved}");
    assert_eq!(saved["visualCriteria"], json!(["b"]), "{saved}");

    daemon.request("task.amend", json!({"id": id, "criteria": ["a", "b"]}));
    let saved = daemon.request("task.get", json!({"id": id}));
    assert!(saved.get("visualCriteria").is_none(), "{saved}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn amending_the_screenshot_command_sets_and_clears_it() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Shot amend",
            "goal": "Something",
            "criteria": ["old"],
            "verify": ["true"],
            "start": false,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();

    let amended = daemon.request(
        "task.amend",
        json!({"id": id, "screenshot": " npm run shot "}),
    );
    assert_eq!(amended["amended"], json!(["screenshot"]), "{amended}");
    let saved = daemon.request("task.get", json!({"id": id}));
    assert_eq!(saved["briefCheck"]["screenshot"], "npm run shot", "{saved}");

    daemon.request("task.amend", json!({"id": id, "screenshot": ""}));
    let saved = daemon.request("task.get", json!({"id": id}));
    assert!(saved["briefCheck"].get("screenshot").is_none(), "{saved}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

#[test]
fn addendum_survives_a_restart_in_order() {
    let mut daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Addendum amend",
            "goal": "Something",
            "criteria": ["old"],
            "verify": ["true"],
            "start": false,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();

    let amended = daemon.request("task.amend", json!({"id": id, "addendum": " first note "}));
    assert_eq!(amended["amended"], json!(["addendum"]), "{amended}");
    daemon.request("task.amend", json!({"id": id, "addendum": "second note"}));
    let saved = daemon.request("task.get", json!({"id": id}));
    assert_eq!(saved["addenda"], json!(["first note", "second note"]));
    assert!(saved["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d == "Amended: addendum"));
    assert_eq!(saved["criteria"], json!(["old"]));

    daemon.restart(&[]);
    let saved = daemon.request("task.get", json!({"id": id}));
    assert_eq!(saved["addenda"], json!(["first note", "second note"]));

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}
