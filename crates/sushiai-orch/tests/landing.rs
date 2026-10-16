//! Black-box integration tests (landing queue): a `land` task carries its
//! work onto its base branch by itself. Real daemon, real git repo.

mod common;

use common::harness::fake_harness_script;
use common::polling::{holds_for, poll_until, wait_until, ABSENCE_WINDOW};
use common::process::Daemon;
use common::repository::{git_out, init_git_repo};

use std::path::Path;

/// Writes FILE_<name>.txt for the task whose goal names `FILE_<name>`; a
/// conflicted `shared.txt` is resolved by keeping both sides.
const LAND_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
if grep -q '<<<<<<<' shared.txt 2>/dev/null; then printf 'a\nb\n' > shared.txt; name=; fi
name=${name-$(printf '%s' "$input" | grep -o 'FILE_[a-z]*' | head -1 | sed 's/FILE_//')}
if [ -z "$name" ]; then :
elif [ "$name" = shared ]; then echo b > shared.txt
else
  echo "$name" > "$name.txt"
fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

fn daemon() -> (Daemon, tempfile::TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", LAND_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    (daemon, scripts)
}

/// A repo with the work branch `work` checked out (the default branch is
/// left alone).
fn repo_on_work_branch() -> tempfile::TempDir {
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    repo
}

fn create(daemon: &Daemon, repo: &Path, name: &str, verify: &str, land: bool) -> serde_json::Value {
    daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "title": format!("Add {name}"),
            "goal": format!("FILE_{name}: write the file"),
            "verify": [verify],
            "land": land,
            "start": false,
        }),
    )
}

fn until_done(daemon: &Daemon, id: &str) -> serde_json::Value {
    poll_until(daemon, id, |s| matches!(s, "done" | "failed" | "stopped"))
}

fn until_dirty_landing(daemon: &Daemon, id: &str) -> serde_json::Value {
    wait_until("the dirty checkout landing wait", || {
        let task = daemon.request("task.get", serde_json::json!({"id": id}));
        let terminal = matches!(task["status"].as_str(), Some("done" | "failed" | "stopped"));
        let waiting = task["status"] == "landing"
            && task["dirtySince"].is_i64()
            && decisions(&task).contains("uncommitted changes");
        (terminal || waiting).then_some(task)
    })
}

fn decisions(task: &serde_json::Value) -> String {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn commits(repo: &Path, branch: &str) -> Vec<String> {
    git_out(repo, &["log", "--format=%s", branch])
        .lines()
        .map(str::to_string)
        .collect()
}

fn create_on(daemon: &Daemon, repo: &Path, name: &str, base: &str) -> serde_json::Value {
    daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "title": format!("Add {name}"),
            "goal": format!("FILE_{name}: write the file"),
            "verify": [format!("test -f {name}.txt")],
            "land": true,
            "base": base,
            "start": false,
        }),
    )
}

fn create_from(
    daemon: &Daemon,
    repo: &Path,
    name: &str,
    base: &str,
    land: bool,
) -> serde_json::Value {
    daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "title": format!("Add {name}"),
            "goal": format!("FILE_{name}: write the file"),
            "verify": [format!("test -f {name}.txt")],
            "land": land,
            "base": base,
            "start": false,
        }),
    )
}

fn create_default_land(
    daemon: &Daemon,
    repo: &Path,
    name: &str,
    land: Option<bool>,
) -> serde_json::Value {
    let mut params = serde_json::json!({
        "repo": repo.to_str().unwrap(),
        "title": format!("Add {name}"),
        "goal": format!("FILE_{name}: write the file"),
        "verify": [format!("test -f {name}.txt")],
        "start": false,
    });
    if let Some(land) = land {
        params["land"] = serde_json::json!(land);
    }
    daemon.request("task.create", params)
}

/// A land task whose final check makes another commit on the base branch
/// each of its first two runs in the task's worktree with `pre.txt` (a carried
/// tree): the first during the attempt's own final checks, the second while
/// the landing re-runs them after carrying onto that commit, so the branch
/// moves under the landing. From the third run on, the check insists the
/// second commit's file is in the tree it runs on.
fn land_race_task(
    daemon: &Daemon,
    root: &Path,
    scratch: &Path,
    mut request: serde_json::Value,
    (second_file, second_content): (&str, &str),
) -> String {
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["sandbox"] = serde_json::json!("host");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let (repo, count) = (root.display(), scratch.join("count").display().to_string());
    let final_check = format!(
        "case \"$PWD\" in *worktrees*) ;; *) exit 0 ;; esac; test -f pre.txt || exit 0; \
         echo x >> '{count}'; n=$(wc -l < '{count}' | tr -d ' '); \
         if [ \"$n\" = 1 ]; then cd '{repo}' && echo one > first.txt && git add first.txt && git commit -q -m 'owner commit one'; \
         elif [ \"$n\" = 2 ]; then cd '{repo}' && printf '{second_content}' > '{second_file}' && git add '{second_file}' && git commit -q -m 'owner commit two'; \
         else test -f '{second_file}'; fi"
    );
    request["repo"] = serde_json::json!(root.to_str().unwrap());
    request["finalVerify"] = serde_json::json!([final_check]);
    request["land"] = serde_json::json!(true);
    request["start"] = serde_json::json!(false);
    let task = daemon.request("task.create", request);
    // The branch moves after the task recorded its base, so the task is
    // carried onto it before its first attempt.
    std::fs::write(root.join("pre.txt"), "pre\n").unwrap();
    git_out(root, &["add", "pre.txt"]);
    git_out(root, &["commit", "-q", "-m", "earlier landing"]);
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    id
}

/// One fake for every role. The implementer of the task whose goal names
/// `FILE_x` changes line 1 of `shared.txt`, the one named `FILE_y` changes line
/// 9; the reviewer (brief opens with "## Review") passes and logs one line per
/// review of the task to `$GATES/reviews`.
const REREVIEW_SCRIPT: &str = r###"#!/bin/sh
input="$(cat)"
emit() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
name=$(printf '%s' "$input" | grep -o 'FILE_[a-z]*' | head -1 | sed 's/FILE_//')
case "$input" in
'## Route the review'*) exit 0 ;;
"## Review"*)
  echo "$name" >> "$GATES/reviews"
  emit '"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"' ;;
*"You check a coding task brief"*)
  emit '"{\"contradiction\": false, \"conflict\": \"\"}"' ;;
*"You judge"*)
  emit '"{\"verdict\": \"feasible\", \"reason\": \"ok\"}"' ;;
*)
  case "$name" in
    x) sed 's/^l1$/X1/' shared.txt > shared.tmp && mv shared.tmp shared.txt ;;
    y) sed 's/^l9$/Y9/' shared.txt > shared.tmp && mv shared.tmp shared.txt ;;
  esac
  emit '"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"' ;;
esac
"###;

#[path = "landing/carry.rs"]
mod carry_tests;
#[path = "landing/queue.rs"]
mod queue_tests;
#[path = "landing/review.rs"]
mod review_tests;
#[path = "landing/targets.rs"]
mod targets_tests;
