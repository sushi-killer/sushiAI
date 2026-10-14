//! A done task whose work does not carry onto the newest base of its pull
//! request is not left without one: a conflict gets one resolution attempt on
//! the new base, a planner test the base already has yields to the base, and
//! the manual `task.pr` carries like the automatic one.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::{Duration, Instant};

const URL: &str = "https://github.example/acme/app/pull/7";

const FAKE_GH: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
printf '%s\n' "$*" >> "$dir/gh.log"
case "$1 $2" in
  "pr list") [ -f "$dir/created" ] && echo https://github.example/acme/app/pull/7; exit 0 ;;
  "pr create")
    while [ $# -gt 0 ]; do
      case "$1" in --head) printf '%s' "$2" > "$dir/head" ;; --title) printf '%s' "$2" > "$dir/title" ;; esac
      shift
    done
    touch "$dir/created"; echo https://github.example/acme/app/pull/7 ;;
  "pr view")
    sha=$(git ls-remote origin "refs/heads/$(cat "$dir/head")" | cut -f1)
    [ -f "$dir/stale-head" ] && sha=0000000000000000000000000000000000000000
    printf '{"url":"https://github.example/acme/app/pull/7","headRefOid":"%s","title":"%s"}\n' "$sha" "$(cat "$dir/title")" ;;
esac
"#;

/// One fake claude for every role. A conflict-only attempt writes
/// `$dir/resolution` into `pr.txt` and keeps its brief; any other
/// implement run writes `pr.txt`; the planner answers with `plan.json`.
const FAKE_CLAUDE: &str = r###"#!/bin/sh
dir="$(dirname "$0")"
brief=$(cat)
case "$brief" in
"## Pull request draft"*)
printf '%s\n' '{"type":"result","result":"{\"branch\":\"\",\"title\":\"\",\"body\":\"\"}"}'
;;
"## Pull request body check"*)
printf '%s\n' '{"type":"result","result":"{\"ok\":true,\"corrections\":[]}"}'
;;
*sushi-plan*)
cat "$dir/plan.json"
;;
*"## Conflict-only attempt"*)
printf '%s' "$brief" > "$dir/resolve-brief.md"
if [ -f "$dir/remote" ] && [ ! -f "$dir/pushed" ]; then
touch "$dir/pushed"
t=$(mktemp -d)
git clone -q "$(cat "$dir/remote")" "$t"
(cd "$t" && echo theirs2 > pr.txt && git -c user.name=o -c user.email=o@example.invalid commit -qam second && git push -q origin HEAD:refs/heads/main)
fi
if [ -f .need-setup ] && [ ! -f "$(git rev-parse --git-dir)/setup-ran" ]; then echo nosetup > pr.txt; else cat "$dir/resolution" > pr.txt; fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"resolved\",\"decisions\":[],\"question\":\"\"}\n```"}'
;;
*)
if [ -f "$dir/first-done" ]; then cat "$dir/resolution" > pr.txt; else echo pr.txt > pr.txt; fi
touch "$dir/first-done"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
;;
esac
"###;

struct Setup {
    daemon: Daemon,
    tools: tempfile::TempDir,
    repo: tempfile::TempDir,
    remote: tempfile::TempDir,
}

impl Setup {
    fn gh_log(&self) -> String {
        std::fs::read_to_string(self.tools.path().join("gh.log")).unwrap_or_default()
    }

    fn task(&self, id: &str) -> Value {
        self.daemon.request("task.get", json!({"id": id}))
    }

    fn create(&self, verify: &str) -> String {
        self.create_with(verify, json!({}))
    }

    fn create_with(&self, verify: &str, extra: Value) -> String {
        let mut params = json!({
            "repo": self.repo.path().to_str().unwrap(),
            "title": "Add pr file",
            "goal": "write pr.txt",
            "criteria": ["pr.txt exists"],
            "verify": [verify],
            "start": true,
        });
        for (k, v) in extra.as_object().unwrap() {
            params[k] = v.clone();
        }
        self.daemon.request("task.create", params)["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Commits `files` to origin's main from a second clone, so the local
    /// main the task starts from stays behind.
    fn advance_origin(&self, files: &[(&str, &str)]) -> String {
        let other = tempfile::tempdir().unwrap();
        git_out(
            other.path(),
            &["clone", "-q", self.remote.path().to_str().unwrap(), "."],
        );
        git_out(
            other.path(),
            &["config", "user.email", "other@example.invalid"],
        );
        git_out(other.path(), &["config", "user.name", "other"]);
        for (name, body) in files {
            let path = other.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        git_out(other.path(), &["add", "."]);
        git_out(other.path(), &["commit", "-q", "-m", "newer main"]);
        git_out(other.path(), &["push", "-q", "origin", "main"]);
        git_out(other.path(), &["rev-parse", "HEAD"])
    }

    fn pushed(&self, task: &Value) -> String {
        let branch = format!("refs/heads/{}", task["branch"].as_str().unwrap());
        git_out(self.remote.path(), &["rev-parse", &branch])
    }

    fn wait_task(&self, id: &str, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let task = self.task(id);
            if pred(&task) {
                return task;
            }
            assert!(Instant::now() < deadline, "{what}: {task}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn chat(&self) -> Vec<String> {
        let repo = self.repo.path().canonicalize().unwrap();
        let thread = self
            .daemon
            .request("chat.get", json!({"repo": repo.to_str().unwrap()}));
        thread["messages"]
            .as_array()
            .map(|m| {
                m.iter()
                    .filter_map(|m| m["text"].as_str())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn setup(auto_pr: bool, env: &[(&str, &str)]) -> Setup {
    let tools = tempfile::tempdir().unwrap();
    let claude = fake_harness_script(tools.path(), "fake-claude.sh", FAKE_CLAUDE);
    let gh = fake_harness_script(tools.path(), "fake-gh.sh", FAKE_GH);
    std::fs::write(tools.path().join("resolution"), "resolved\n").unwrap();
    let mut vars = vec![
        ("ORCHD_CLAUDE_BIN", claude.to_str().unwrap()),
        ("ORCHD_GH_BIN", gh.to_str().unwrap()),
    ];
    vars.extend_from_slice(env);
    let daemon = Daemon::spawn(&vars);
    let repo = init_git_repo();
    git_out(repo.path(), &["branch", "-M", "main"]);
    let remote = tempfile::tempdir().unwrap();
    git_out(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
    git_out(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git_out(repo.path(), &["push", "-q", "origin", "main"]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["prOnDoneOff"] = json!(!auto_pr);
    daemon.request("settings.set", json!({"settings": settings}));
    cheap_route_on_claude(&daemon);
    Setup {
        daemon,
        tools,
        repo,
        remote,
    }
}

fn blob(repo: &Path, rev: &str, path: &str) -> String {
    git_out(repo, &["show", &format!("{rev}:{path}")])
}

#[test]
fn a_conflicting_carry_gets_one_resolution_attempt_and_then_the_pull_request() {
    let s = setup(true, &[]);
    let newer = s.advance_origin(&[("pr.txt", "theirs\n")]);
    let id = s.create("test -f pr.txt");
    let task = s.wait_task(&id, "no pull request after the resolution", |t| {
        t["prUrl"].is_string()
    });
    assert_eq!(task["prUrl"], URL, "{task}");
    assert_eq!(task["status"], "done", "{task}");
    assert!(task["prConflictTried"].is_null(), "{task}");
    let decisions = task["decisions"].to_string();
    assert!(
        decisions.contains("PR: the work conflicts with origin/main in pr.txt"),
        "{decisions}"
    );
    assert!(
        !decisions.contains("could not be carried"),
        "silent dead end: {decisions}"
    );
    let brief = std::fs::read_to_string(s.tools.path().join("resolve-brief.md")).unwrap();
    assert!(brief.contains("pr.txt"), "{brief}");
    let pushed = s.pushed(&task);
    git_out(
        s.remote.path(),
        &["merge-base", "--is-ancestor", &newer, &pushed],
    );
    assert_eq!(blob(s.remote.path(), &pushed, "pr.txt"), "resolved");
    assert!(
        s.gh_log().contains("pr create --base main"),
        "{}",
        s.gh_log()
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_resolution_that_fails_ends_the_task_failed_with_one_notice_and_no_pull_request() {
    let s = setup(true, &[("ORCHD_STUCK_PAUSE_SECS", "0")]);
    std::fs::write(s.tools.path().join("resolution"), "<<<<<<< left in\n").unwrap();
    s.advance_origin(&[("pr.txt", "theirs\n")]);
    let id = s.create("test -f pr.txt && ! grep -q '<<<<<<<' pr.txt");
    let task = s.wait_task(&id, "the task never failed", |t| t["status"] == "failed");
    assert!(task["prUrl"].is_null(), "{task}");
    assert!(!s.gh_log().contains("pr create"), "{}", s.gh_log());
    std::thread::sleep(Duration::from_secs(2));
    let notices = s
        .chat()
        .into_iter()
        .filter(|t| t.contains("Add pr file") && t.contains("failed"))
        .count();
    assert_eq!(notices, 1, "{:?}", s.chat());
    assert_eq!(task["prConflictTried"], true, "{task}");
    let again = s.daemon.request("task.start", json!({"id": id}));
    assert!(again["prConflictTried"].is_null(), "{again}");
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_base_that_already_holds_the_work_opens_no_pull_request_and_leaves_one_decision_line() {
    let s = setup(true, &[]);
    // The same fix is already on origin's main.
    s.advance_origin(&[("pr.txt", "pr.txt\n")]);
    let id = s.create("test -f pr.txt");
    let task = s.wait_task(&id, "the base never held the work", |t| {
        t["decisions"]
            .to_string()
            .contains("already contains this work")
    });
    assert_eq!(task["status"], "done", "{task}");
    let contains = task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d.as_str().unwrap().contains("already contains this work"))
        .count();
    assert_eq!(contains, 1, "{task}");
    std::thread::sleep(Duration::from_secs(2));
    assert!(s.task(&id)["prUrl"].is_null(), "{task}");
    assert!(!s.gh_log().contains("pr create"), "{}", s.gh_log());
    assert!(s.chat().is_empty(), "{:?}", s.chat());
    s.daemon.shutdown_and_wait();
}

#[test]
fn task_pr_carries_the_branch_onto_the_newest_base_like_the_automatic_pull_request() {
    let s = setup(false, &[]);
    let id = s.create("test -f pr.txt");
    poll_until(&s.daemon, &id, |st| st == "done");
    let newer = s.advance_origin(&[("newer.txt", "newer\n")]);
    // The reply comes before the carry: the pull request opens afterwards.
    let reply = s.daemon.request("task.pr", json!({"id": id}));
    assert!(reply["prUrl"].is_null(), "{reply}");
    assert_eq!(reply["prPending"], true, "{reply}");
    let opened = s.wait_task(&id, "no pull request after the carry", |t| {
        t["prUrl"].is_string()
    });
    assert_eq!(opened["prUrl"], URL, "{opened}");
    let pushed = s.pushed(&opened);
    git_out(
        s.remote.path(),
        &["merge-base", "--is-ancestor", &newer, &pushed],
    );
    let files = git_out(s.remote.path(), &["ls-tree", "-r", "--name-only", &pushed]);
    assert!(
        files.contains("pr.txt") && files.contains("newer.txt"),
        "{files}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_conflict_in_a_parent_task_is_resolved_before_its_checks_and_the_pull_request_opens() {
    let s = setup(true, &[]);
    let newer = s.advance_origin(&[("pr.txt", "theirs\n")]);
    let parent = s.create_with("test -f pr.txt", json!({"start": false}));
    let child = s.daemon.request(
        "task.create",
        json!({
            "repo": s.repo.path().to_str().unwrap(), "title": "Child", "goal": "write pr.txt",
            "criteria": ["pr.txt exists"], "verify": ["test -f pr.txt"],
            "parent": parent, "start": true,
        }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    poll_until(&s.daemon, &child, |st| st == "done");
    let task = s.wait_task(&parent, "no pull request for the parent", |t| {
        t["prUrl"].is_string()
    });
    let pushed = s.pushed(&task);
    git_out(
        s.remote.path(),
        &["merge-base", "--is-ancestor", &newer, &pushed],
    );
    let text = blob(s.remote.path(), &pushed, "pr.txt");
    assert_eq!(text, "resolved", "markers or old content pushed: {text}");
    assert!(
        std::fs::read_to_string(s.tools.path().join("resolve-brief.md")).is_ok(),
        "no conflict-only attempt ran"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn the_resolution_attempt_runs_in_a_worktree_with_its_setup_done() {
    let s = setup(true, &[]);
    let setup_json = json!({"setup": "touch .need-setup-unused; touch \"$(git rev-parse --git-dir)/setup-ran\""});
    std::fs::create_dir_all(s.repo.path().join(".sushiai")).unwrap();
    std::fs::write(
        s.repo.path().join(".sushiai/setup.json"),
        setup_json.to_string(),
    )
    .unwrap();
    std::fs::write(s.repo.path().join(".need-setup"), "").unwrap();
    git_out(s.repo.path(), &["add", "-f", "."]);
    git_out(s.repo.path(), &["commit", "-q", "-m", "add setup"]);
    git_out(s.repo.path(), &["push", "-q", "origin", "main"]);
    s.advance_origin(&[("pr.txt", "theirs\n")]);
    let id = s.create("test -f pr.txt");
    let task = s.wait_task(&id, "no pull request after the resolution", |t| {
        t["prUrl"].is_string()
    });
    let pushed = s.pushed(&task);
    assert_eq!(blob(s.remote.path(), &pushed, "pr.txt"), "resolved");
    s.daemon.shutdown_and_wait();
}

#[test]
fn task_pr_keeps_the_owners_title_through_a_resolution_attempt() {
    let s = setup(false, &[]);
    let id = s.create("test -f pr.txt");
    poll_until(&s.daemon, &id, |st| st == "done");
    s.advance_origin(&[("pr.txt", "theirs\n")]);
    let reply = s
        .daemon
        .request("task.pr", json!({"id": id, "title": "Owner chosen title"}));
    assert!(reply["prUrl"].is_null(), "{reply}");
    s.wait_task(&id, "no pull request after the resolution", |t| {
        t["prUrl"].is_string()
    });
    assert!(
        s.gh_log().contains("--title Owner chosen title"),
        "{}",
        s.gh_log()
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_second_conflict_after_the_resolution_ends_the_task_failed_with_one_notice() {
    let s = setup(true, &[]);
    std::fs::write(
        s.tools.path().join("remote"),
        s.remote.path().to_str().unwrap(),
    )
    .unwrap();
    s.advance_origin(&[("pr.txt", "theirs\n")]);
    let id = s.create("test -f pr.txt");
    let task = s.wait_task(&id, "the task never failed", |t| t["status"] == "failed");
    assert!(task["prUrl"].is_null(), "{task}");
    assert!(
        task["decisions"]
            .to_string()
            .contains("still conflicts with origin/main after its resolution attempt"),
        "{task}"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert!(!s.gh_log().contains("pr create"), "{}", s.gh_log());
    let notices = s
        .chat()
        .into_iter()
        .filter(|t| t.contains("Add pr file") && t.contains("failed"))
        .count();
    assert_eq!(notices, 1, "{:?}", s.chat());
    s.daemon.shutdown_and_wait();
}
