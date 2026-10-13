//! A done top-level task that could not land on the default branch gets a
//! pull request drafted by a read-only helper and opened through `task.pr`.

mod common;

use common::*;
use std::time::{Duration, Instant};

const URL: &str = "https://github.example/acme/app/pull/7";

const FAKE_GH: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
printf '%s\n' "$*" >> "$dir/gh.log"
case "$1 $2" in
  "pr list") [ -f "$dir/created" ] && echo https://github.example/acme/app/pull/7; exit 0 ;;
  "pr create") touch "$dir/created"; echo https://github.example/acme/app/pull/7 ;;
esac
"#;

/// One fake claude for both roles: the drafter (brief opens with "## Pull
/// request draft") replies with `reply`, every other run implements the task.
fn script(reply: &str) -> String {
    let result = serde_json::json!({"type": "result", "result": reply});
    format!(
        r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Pull request draft"*)
printf '%s' "$brief" > "$(dirname "$0")/draft-brief.md"
cat <<'JSON'
{result}
JSON
;;
*)
echo pr.txt > pr.txt
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-fake"}}'
printf '%s\n' '{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":1,"output_tokens":1}},"result":"```sushi-report\n{{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}}\n```"}}'
;;
esac
"###
    )
}

struct Setup {
    daemon: Daemon,
    tools: tempfile::TempDir,
    repo: tempfile::TempDir,
    _remote: tempfile::TempDir,
}

impl Setup {
    fn gh_log(&self) -> String {
        std::fs::read_to_string(self.tools.path().join("gh.log")).unwrap_or_default()
    }

    fn create(&self, extra: serde_json::Value) -> String {
        let mut params = serde_json::json!({
            "repo": self.repo.path().to_str().unwrap(),
            "title": "Add pr file",
            "goal": "write pr.txt",
            "criteria": ["pr.txt exists"],
            "verify": ["test -f pr.txt"],
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

    fn pr_url(&self, id: &str) -> Option<String> {
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            let task = self
                .daemon
                .request("task.get", serde_json::json!({"id": id}));
            if let Some(url) = task["prUrl"].as_str() {
                return Some(url.to_string());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "no pull request: {}",
            self.daemon
                .request("task.get", serde_json::json!({"id": id}))
        )
    }
}

fn setup(reply: &str, enabled: bool) -> Setup {
    let tools = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(tools.path(), "fake-claude.sh", &script(reply));
    let gh = fake_harness_script(tools.path(), "fake-gh.sh", FAKE_GH);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
        ("ORCHD_GH_BIN", gh.to_str().unwrap()),
    ]);
    let repo = init_git_repo();
    git_out(repo.path(), &["branch", "-M", "main"]);
    let remote = tempfile::tempdir().unwrap();
    git_out(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
    git_out(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git_out(repo.path(), &["push", "-q", "origin", "main"]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    if enabled {
        settings["prOnDoneRepos"] = serde_json::json!({ repo.path().to_str().unwrap(): true });
    }
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    cheap_route_on_claude(&daemon);
    Setup {
        daemon,
        tools,
        repo,
        _remote: remote,
    }
}

fn done(s: &Setup, id: &str) -> serde_json::Value {
    let task = poll_until(&s.daemon, id, |st| {
        matches!(st, "done" | "failed" | "stopped")
    });
    assert_eq!(task["status"], "done", "{task}");
    task
}

fn settle() {
    std::thread::sleep(Duration::from_secs(2));
}

#[test]
fn a_done_task_refused_on_the_default_branch_opens_the_drafted_pull_request() {
    let reply = "```json\n{\"branch\":\"feature/drafted-pr\",\"title\":\"feat: drafted title\",\"body\":\"Drafted body for the owner\"}\n```";
    let s = setup(reply, true);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let log = s.gh_log();
    assert!(
        log.contains("pr create --base main --head feature/drafted-pr --title feat: drafted title --body Drafted body for the owner"),
        "{log}"
    );
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert_eq!(task["status"], "done");
    let decisions = task["decisions"].to_string();
    assert!(
        decisions.contains("PR: drafted by claude-cheap"),
        "{decisions}"
    );
    assert!(
        decisions.contains(&format!("PR: opened {URL}")),
        "{decisions}"
    );
    let brief = std::fs::read_to_string(s.tools.path().join("draft-brief.md")).unwrap();
    assert!(
        brief.contains("CONTRIBUTING.md") && brief.contains("Add pr file"),
        "{brief}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn no_pull_request_is_opened_while_the_setting_is_off() {
    let s = setup("{}", false);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    settle();
    assert!(s.gh_log().is_empty(), "{}", s.gh_log());
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(task.get("prUrl").is_none(), "{task}");
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_subtask_never_opens_a_pull_request_only_its_top_level_parent_does() {
    let reply = "{\"branch\":\"feature/x\",\"title\":\"t\",\"body\":\"b\"}";
    let s = setup(reply, true);
    let parent = s.create(serde_json::json!({"start": false}));
    let child = s.create(serde_json::json!({"parent": parent, "title": "Child", "start": true}));
    done(&s, &child);
    // The parent finishes once its only child landed and opens its own pull request.
    assert_eq!(s.pr_url(&parent).as_deref(), Some(URL), "{}", s.gh_log());
    settle();
    let child_task = s
        .daemon
        .request("task.get", serde_json::json!({"id": child}));
    assert!(child_task.get("prUrl").is_none(), "{child_task}");
    assert!(
        !child_task["decisions"].to_string().contains("PR:"),
        "{child_task}"
    );
    assert_eq!(s.gh_log().matches("pr create").count(), 1, "{}", s.gh_log());
    s.daemon.shutdown_and_wait();
}

#[test]
fn an_unparseable_draft_opens_the_pull_request_with_the_tasks_own_title_and_branch() {
    let s = setup("I could not decide.", true);
    let id = s.create(serde_json::json!({}));
    let task = done(&s, &id);
    let branch = task["branch"].as_str().unwrap().to_string();
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let log = s.gh_log();
    assert!(
        log.contains(&format!(
            "pr create --base main --head {branch} --title Add pr file"
        )),
        "{log}"
    );
    assert!(log.contains("- [x] pr.txt exists"), "{log}");
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(
        task["decisions"].to_string().contains("PR: draft failed"),
        "{task}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn settings_refuse_a_pr_route_that_is_no_configured_route() {
    let s = setup("{}", false);
    let mut settings = s.daemon.request("settings.get", serde_json::json!({}));
    settings["prRoute"] = serde_json::json!("nope");
    let err = s
        .daemon
        .request_error("settings.set", serde_json::json!({"settings": settings}));
    assert!(err.contains("prRoute"), "{err}");
    s.daemon.shutdown_and_wait();
}
