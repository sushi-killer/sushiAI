//! Repo-declared policy shares the settings filters and is pinned to the base.
mod common;

use common::*;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const FAKE: &str = r#"#!/bin/sh
cat > "$LOG_DIR/brief.$$"
mkdir -p "$(dirname "$IMPL_FILE")"
echo changed > "$IMPL_FILE"
if [ -n "$EDIT_SETUP" ]; then
  mkdir -p .sushiai
  printf '%s\n' "$EDIT_SETUP" > .sushiai/setup.json
fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

const MARKER_CMD: &str = r#"printf '%s\n' "$PWD" >> "$LOG_DIR/checks.log""#;

fn run(
    file: &str,
    planned: &[&str],
    repo_paths: &[&str],
    setting_paths: &[&str],
    edit: bool,
) -> (Value, Vec<PathBuf>) {
    let repo = init_git_repo();
    std::fs::create_dir_all(repo.path().join(".sushiai")).unwrap();
    let setup =
        json!({"compilerCache":false,"scopedChecks":[{"command":MARKER_CMD,"paths":repo_paths}]});
    std::fs::write(repo.path().join(".sushiai/setup.json"), setup.to_string()).unwrap();
    git_out(repo.path(), &["add", "."]);
    git_out(repo.path(), &["commit", "-qm", "setup policy"]);
    let bin = tempfile::tempdir().unwrap();
    let log = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(bin.path(), "fake-claude.sh", FAKE);
    let edit_setup = if edit {
        json!({"compilerCache":false,"scopedChecks":[{"command":MARKER_CMD,"paths":["never/**"]}]})
            .to_string()
    } else {
        String::new()
    };
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
        ("IMPL_FILE", file),
        ("LOG_DIR", log.path().to_str().unwrap()),
        ("EDIT_SETUP", &edit_setup),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["sandbox"] = json!("host");
    if !setting_paths.is_empty() {
        settings["scopedChecks"] = json!([{"command":MARKER_CMD,"paths":setting_paths}]);
    }
    daemon.request("settings.set", json!({"settings":settings}));
    let task = daemon.request(
        "task.create",
        json!({
            "repo":repo.path().to_str().unwrap(), "title":"Scoped repo", "goal":"Write file",
            "verify":[MARKER_CMD,"true"], "finalVerify":[MARKER_CMD], "paths":planned,
        }),
    );
    let done = settle(&daemon, task["id"].as_str().unwrap());
    assert_eq!(done["status"], "done", "{done}");
    let runs: Vec<PathBuf> = std::fs::read_to_string(log.path().join("checks.log"))
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .collect();
    daemon.shutdown_and_wait();
    remove_dir_retrying(done["worktree"].as_str().unwrap());
    (done, runs)
}

fn skipped(task: &Value) -> Vec<&str> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .filter(|d| d.starts_with("Orchestrator: skipped"))
        .collect()
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

fn task_runs(task: &Value, runs: &[PathBuf]) -> usize {
    let wt = canon(Path::new(task["worktree"].as_str().unwrap()));
    runs.iter().filter(|dir| canon(dir) == wt).count()
}

#[test]
fn repo_policy_skips_diff_and_base_preflight_outside_its_paths() {
    let (task, runs) = run(
        "orchd/src/x.rs",
        &["orchd/**"],
        &["electron/**", "src/app/**"],
        &[],
        false,
    );
    assert!(
        runs.is_empty(),
        "base and worktree check must both be skipped: {runs:?}"
    );
    assert_eq!(
        skipped(&task),
        [format!(
            "Orchestrator: skipped {MARKER_CMD}: no change under electron/**, src/app/**"
        )]
    );
}

#[test]
fn repo_policy_runs_diff_and_base_preflight_inside_its_paths() {
    let (task, runs) = run(
        "electron/main.cjs",
        &["electron/**"],
        &["electron/**"],
        &[],
        false,
    );
    assert_eq!(task_runs(&task, &runs), 1, "{runs:?}");
    assert!(
        runs.len() > task_runs(&task, &runs),
        "missing base preflight: {runs:?}"
    );
    assert!(skipped(&task).is_empty(), "{task}");
}

#[test]
fn settings_and_repo_scopes_are_merged_by_the_same_filter() {
    let (task, runs) = run(
        "electron/main.cjs",
        &["electron/main.cjs"],
        &["src/app/**"],
        &["electron/**"],
        false,
    );
    assert_eq!(task_runs(&task, &runs), 1);
    assert!(
        runs.len() > 1,
        "planned paths should match the setting: {runs:?}"
    );
    assert!(skipped(&task).is_empty());
    let (task, runs) = run(
        "src/app/view.ts",
        &["src/app/view.ts"],
        &["src/app/**"],
        &["electron/**"],
        false,
    );
    assert_eq!(task_runs(&task, &runs), 1);
    assert!(
        runs.len() > 1,
        "planned paths should match the repo declaration: {runs:?}"
    );
    assert!(skipped(&task).is_empty());
}

#[test]
fn a_task_cannot_edit_setup_to_skip_its_own_check() {
    let (task, runs) = run(
        "electron/main.cjs",
        &["electron/main.cjs"],
        &["electron/**"],
        &[],
        true,
    );
    assert_eq!(task_runs(&task, &runs), 1, "{runs:?}");
    assert!(skipped(&task).is_empty(), "{task}");
}

#[test]
fn the_reader_uses_the_commit_and_rejects_unknown_keys() {
    let repo = init_git_repo();
    let path = repo.path().join(".sushiai/setup.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"scopedChecks":[{"command":"check","paths":["src/**"]}]}"#,
    )
    .unwrap();
    git_out(repo.path(), &["add", "."]);
    git_out(repo.path(), &["commit", "-qm", "scope"]);
    let base = git_out(repo.path(), &["rev-parse", "HEAD"]);
    std::fs::write(
        &path,
        r#"{"scopedChecks":[{"command":"check","paths":["never/**"]}]}"#,
    )
    .unwrap();
    let scopes = sushiai_orch::worktree_env::scoped_checks_at(repo.path(), &base).unwrap();
    assert_eq!(scopes[0].repo.as_deref(), repo.path().to_str());
    assert_eq!(scopes[0].paths, ["src/**"]);
    for text in [
        r#"{"typo":true}"#,
        r#"{"scopedChecks":[{"command":"check","paths":[],"repo":"elsewhere"}]}"#,
        r#"{"scopedChecks":[{"command":"check","paths":"src/**"}]}"#,
    ] {
        std::fs::write(&path, text).unwrap();
        git_out(repo.path(), &["add", "."]);
        git_out(repo.path(), &["commit", "-qm", "invalid policy"]);
        assert!(
            sushiai_orch::worktree_env::scoped_checks_at(repo.path(), "HEAD").is_err(),
            "{text}"
        );
    }
    assert!(sushiai_orch::worktree_env::scoped_checks_at(repo.path(), "bad-revision").is_err());
    assert!(
        sushiai_orch::git::show_file(repo.path(), &base, "absent.json")
            .unwrap()
            .is_none()
    );
}
