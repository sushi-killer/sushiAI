//! Subtasks run derived checks in their own tree; parents keep the full gate.
mod common;

use common::process::Daemon;

use common::harness::fake_harness_script;
use common::polling::{remove_dir_retrying, settle};
use common::repository::{git_out, init_git_repo};

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const FAKE: &str = r#"#!/bin/sh
input="$(cat)"
case "$input" in
  *"You review a coding task plan"*)
    printf '%s\n' '{"type":"result","result":"{\"findings\": []}"}'
    exit 0 ;;
  *sushi-plan*)
    case "$input" in
      *PART_A*) cat "$PLAN_CHILD" ;;
      *PART_B*) cat "$PLAN_OTHER" ;;
      *) cat "$PLAN_PARENT" ;;
    esac
    exit 0 ;;
esac
files="$IMPL_FILES"
case "$input" in *PART_B*) files="b.txt" ;; esac
for file in $files; do
  mkdir -p "$(dirname "$file")"
  echo changed >> "$file"
done
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

const TOOL: &str = r#"#!/bin/sh
printf '%s\t%s %s\n' "$PWD" "$(basename "$0")" "$*" >> "$CHECK_LOG"
"#;

fn write(repo: &Path, file: &str, text: &str) {
    let path = repo.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn decisions(task: &Value) -> Vec<&str> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap())
        .collect()
}

fn assert_parent_reuses_check(parent: &Value, command: &str) {
    assert_eq!(parent["finalVerify"], json!([command]));
    let prefix = format!("Orchestrator: {command} already passed on tree ");
    assert!(
        decisions(parent)
            .iter()
            .any(|d| d.starts_with(&prefix) && d.ends_with("; not run again")),
        "{parent}"
    );
}

fn canon(path: &Path) -> PathBuf {
    path.ancestors()
        .find_map(|head| {
            head.canonicalize()
                .ok()
                .map(|real| real.join(path.strip_prefix(head).unwrap()))
        })
        .unwrap_or_else(|| path.to_path_buf())
}

fn commands_in(log: &str, task: &Value) -> Vec<String> {
    let wt = canon(Path::new(task["worktree"].as_str().unwrap()));
    log.lines()
        .filter_map(|line| {
            let (dir, command) = line.split_once('\t')?;
            (canon(Path::new(dir)) == wt).then(|| command.to_string())
        })
        .collect()
}

fn fixture() -> tempfile::TempDir {
    let repo = init_git_repo();
    write(
        repo.path(),
        "Cargo.toml",
        "[workspace]\nmembers = ['a', 'b']\n",
    );
    for (dir, name) in [("a", "package-a"), ("b", "package-b")] {
        write(
            repo.path(),
            &format!("{dir}/Cargo.toml"),
            &format!("[package]\nname = '{name}'\nversion = '0.1.0'\n"),
        );
        write(repo.path(), &format!("{dir}/src/lib.rs"), "// base\n");
    }
    write(repo.path(), "Cargo.lock", "# base\n");
    write(repo.path(), "src/pair.js", "// base\n");
    write(repo.path(), "tests/pair.test.cjs", "// base\n");
    write(repo.path(), "tests/other.test.cjs", "// base\n");
    write(
        repo.path(),
        ".sushiai/setup.json",
        r#"{"compilerCache":false}"#,
    );
    git_out(repo.path(), &["add", "."]);
    git_out(repo.path(), &["commit", "-qm", "fixture"]);
    repo
}

/// The plan turn's reply: one `sushi-plan` block.
fn plan_reply(dir: &Path, name: &str, plan: Value) -> PathBuf {
    let text = format!("```sushi-plan\n{plan}\n```");
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!("{}\n", json!({"type": "result", "result": text})),
    )
    .unwrap();
    path
}

/// Runs one subtask at its parent. With a `check` the planner splits the
/// parent into a part that names it; without one the subtask is created by
/// hand and has no planner check.
fn settle_graph(
    daemon: &Daemon,
    repo: &Path,
    command: &str,
    planned: bool,
    parent_lists_check: bool,
) -> (Value, Value) {
    if planned {
        let parent = daemon.request(
            "task.create",
            json!({"repo": repo.to_str().unwrap(), "request": "combine the parts", "start": true}),
        );
        let parent = settle(daemon, parent["id"].as_str().unwrap());
        let listed = daemon.request("task.list", json!({"repo": parent["repo"]}));
        let child = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["parent"] == parent["id"] && t["title"] == "Part")
            .unwrap_or_else(|| panic!("the planner created a child: {listed}"))
            .clone();
        (parent, settle(daemon, child["id"].as_str().unwrap()))
    } else {
        let parent = daemon.request("task.create", json!({
            "repo":repo.to_str().unwrap(), "title":"Parent", "goal":"Combined tree", "verify":["true"],
            "finalVerify": if parent_lists_check { vec![command] } else { vec![] }, "start":false,
        }));
        let child = daemon.request("task.create", json!({
            "repo":repo.to_str().unwrap(), "title":"Part", "goal":"Write files", "verify":[command], "parent":parent["id"],
        }));
        let child = settle(daemon, child["id"].as_str().unwrap());
        (settle(daemon, parent["id"].as_str().unwrap()), child)
    }
}

fn run(
    files: &str,
    command: &str,
    check: Option<&str>,
    gate_at: &str,
    parent_lists_check: bool,
) -> (Value, Value, Vec<String>, Vec<String>) {
    let repo = fixture();
    let bin = tempfile::tempdir().unwrap();
    let log = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(bin.path(), "fake-claude.sh", FAKE);
    for tool in ["cargo", "npm", "node"] {
        fake_harness_script(bin.path(), tool, TOOL);
    }
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let check_log = log.path().join("checks.log");
    let part = json!({
        "key": "a", "title": "Part", "request": "PART_A write files",
        "paths": files.split_whitespace().collect::<Vec<_>>(),
        "check": check.unwrap_or_default(),
    });
    // A single part is the task itself: a second, unrelated part keeps the split.
    let other = json!({
        "key": "b", "title": "Other", "request": "PART_B write b.txt",
        "paths": ["b.txt"], "check": "test -f b.txt",
    });
    let other_plan = plan_reply(
        log.path(),
        "plan-other.json",
        json!({"title": "Other", "goal": "PART_B: write b.txt", "verify": ["test -f b.txt"]}),
    );
    let parent_plan = plan_reply(
        log.path(),
        "plan-parent.json",
        json!({
            "title": "Parent", "goal": "Combined tree", "verify": ["true"],
            "finalVerify": if parent_lists_check { vec![command] } else { vec![] },
            "subtasks": [part, other],
        }),
    );
    let child_plan = plan_reply(
        log.path(),
        "plan-child.json",
        json!({"title": "Part", "goal": "PART_A: write files", "verify": ["true"], "finalVerify": [command]}),
    );
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
        ("PLAN_PARENT", parent_plan.to_str().unwrap()),
        ("PLAN_CHILD", child_plan.to_str().unwrap()),
        ("PLAN_OTHER", other_plan.to_str().unwrap()),
        ("IMPL_FILES", files),
        ("CHECK_LOG", check_log.to_str().unwrap()),
        ("PATH", &path),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["briefCheckRoute"] = json!("");
    settings["planReview"] = json!("");
    settings["answerPolicy"] = json!(true);
    settings["sandbox"] = json!("host");
    settings["gateAt"] = json!({repo.path().to_str().unwrap(): gate_at});
    daemon.request("settings.set", json!({"settings":settings}));
    let (parent, child) = settle_graph(
        &daemon,
        repo.path(),
        command,
        check.is_some(),
        parent_lists_check,
    );
    assert_eq!(child["status"], "done", "{child}");
    assert_eq!(parent["status"], "done", "{parent}");
    let log = std::fs::read_to_string(&check_log).unwrap();
    let child_commands = commands_in(&log, &child);
    let parent_commands = commands_in(&log, &parent);
    daemon.shutdown_and_wait();
    for task in [&child, &parent] {
        remove_dir_retrying(task["worktree"].as_str().unwrap());
    }
    (child, parent, child_commands, parent_commands)
}

#[test]
fn crate_a_runs_its_package_and_registers_the_workspace_check_at_the_parent() {
    let (child, parent, child_commands, parent_commands) = run(
        "a/src/lib.rs",
        "cargo test --workspace",
        Some("cargo test -p package-a"),
        "parent",
        false,
    );
    assert_eq!(child_commands, ["cargo test -p package-a"], "{child}");
    assert_eq!(parent_commands, ["cargo test --workspace"]);
    assert!(decisions(&child).contains(&"Orchestrator: subtask ran cargo test -p package-a instead of cargo test --workspace; the parent runs cargo test --workspace on the combined tree"), "{child}");
    let line = format!(
        "Orchestrator: added cargo test --workspace to finalVerify for subtask {}",
        &child["id"].as_str().unwrap()[..8]
    );
    assert!(decisions(&parent).contains(&line.as_str()), "{parent}");
    assert_eq!(parent["finalVerify"], json!(["cargo test --workspace"]));
}

#[test]
fn source_and_test_pair_runs_only_the_matching_node_test() {
    let (child, parent, child_commands, parent_commands) = run(
        "src/pair.js tests/pair.test.cjs",
        "npm run ci",
        Some("node --test tests/pair.test.cjs"),
        "parent",
        false,
    );
    assert_eq!(child_commands, ["node --test tests/pair.test.cjs"]);
    assert_eq!(parent_commands, ["npm run ci"]);
    assert!(decisions(&child).contains(&"Orchestrator: subtask ran node --test tests/pair.test.cjs instead of npm run ci; the parent runs npm run ci on the combined tree"), "{child}");
    assert_eq!(parent["finalVerify"], json!(["npm run ci"]));
}

#[test]
fn a_subtask_without_a_planner_check_runs_the_full_command_in_the_child() {
    let (child, parent, child_commands, parent_commands) = run(
        "a/src/lib.rs Cargo.lock",
        "cargo test --workspace",
        None,
        "parent",
        true,
    );
    assert_eq!(child_commands, ["cargo test --workspace"], "{child}");
    assert!(parent_commands.is_empty(), "{parent_commands:?}");
    assert_parent_reuses_check(&parent, "cargo test --workspace");
    assert!(
        !decisions(&child).iter().any(|d| d.contains("subtask ran")),
        "{child}"
    );
}

#[test]
fn every_gate_keeps_the_workspace_command_in_the_child() {
    let (child, parent, child_commands, parent_commands) = run(
        "a/src/lib.rs",
        "cargo test --workspace",
        None,
        "every",
        true,
    );
    assert_eq!(child_commands, ["cargo test --workspace"], "{child}");
    assert!(parent_commands.is_empty(), "{parent_commands:?}");
    assert_parent_reuses_check(&parent, "cargo test --workspace");
    assert!(
        !decisions(&child).iter().any(|d| d.contains("subtask ran")),
        "{child}"
    );
    assert!(
        !decisions(&parent).iter().any(|d| d.contains("added cargo")),
        "{parent}"
    );
}

#[test]
fn explicit_package_and_test_targets_stay_unchanged() {
    let (child, parent, child_commands, parent_commands) = run(
        "a/src/lib.rs",
        "cargo test -p package-a --test one",
        None,
        "parent",
        true,
    );
    assert_eq!(
        child_commands,
        ["cargo test -p package-a --test one"],
        "{child}"
    );
    assert!(parent_commands.is_empty(), "{parent_commands:?}");
    assert_parent_reuses_check(&parent, "cargo test -p package-a --test one");
    assert!(
        !decisions(&child).iter().any(|d| d.contains("subtask ran")),
        "{child}"
    );
}
