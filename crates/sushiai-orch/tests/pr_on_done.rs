//! A done top-level task whose work did not land gets a pull request drafted
//! by a read-only helper and opened through `task.pr`, by default and without
//! the overseer.

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
fn script(reply: &str, hold: bool) -> String {
    let wait = if hold {
        r#"touch "$(dirname "$0")/draft-started"
while [ ! -f "$(dirname "$0")/go" ]; do sleep 0.1; done"#
    } else {
        ""
    };
    let result = serde_json::json!({"type": "result", "result": reply});
    format!(
        r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Pull request draft"*)
printf '%s' "$brief" > "$(dirname "$0")/draft-brief.md"
echo draft >> "$(dirname "$0")/drafts.log"
{wait}
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

/// What the settings say about pull requests.
#[derive(Clone, Copy, PartialEq)]
enum Pr {
    /// Settings untouched: the default.
    Default,
    /// A per-repo entry turns it on.
    On,
    /// `prOnDoneOff`.
    Off,
}

fn setup(reply: &str, enabled: bool) -> Setup {
    setup_with(reply, enabled, false, false)
}

fn setup_with(reply: &str, enabled: bool, hold: bool, land: bool) -> Setup {
    let pr = if enabled { Pr::On } else { Pr::Off };
    setup_full(reply, pr, hold, land, FAKE_GH, &[])
}

fn setup_full(
    reply: &str,
    pr: Pr,
    hold: bool,
    land: bool,
    gh_body: &str,
    env: &[(&str, &str)],
) -> Setup {
    let tools = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(tools.path(), "fake-claude.sh", &script(reply, hold));
    let gh = fake_harness_script(tools.path(), "fake-gh.sh", gh_body);
    let mut vars = vec![
        ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
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
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    match pr {
        Pr::On => {
            settings["prOnDoneRepos"] = serde_json::json!({ repo.path().to_str().unwrap(): true });
        }
        Pr::Off => settings["prOnDoneOff"] = serde_json::json!(true),
        // The shipped default (pinned in the model test); the shared test
        // setup turns it off for every other suite.
        Pr::Default => settings["prOnDoneOff"] = serde_json::json!(false),
    }
    if land {
        settings["landOnDefaultRepos"] = serde_json::json!({ repo.path().to_str().unwrap(): true });
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
    let reply = "{\"branch\":\"feature/drafted-pr\",\"title\":\"feat: drafted title\",\"body\":\"Drafted body for the owner\"}";
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

fn wait_for(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_decision(s: &Setup, id: &str, text: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
        if task["decisions"].to_string().contains(text) {
            return task;
        }
        assert!(Instant::now() < deadline, "no decision {text}: {task}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_task_that_lands_opens_no_pull_request() {
    let s = setup_with("{}", true, false, true);
    let id = s.create(serde_json::json!({}));
    let task = done(&s, &id);
    assert!(task["landedSha"].is_string(), "{task}");
    settle();
    assert!(s.gh_log().is_empty(), "{}", s.gh_log());
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(
        task.get("prUrl").is_none() && task.get("prPending").is_none(),
        "{task}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_drafted_base_branch_is_refused_and_the_tasks_own_branch_is_used() {
    let s = setup("{\"branch\":\"main\",\"title\":\"t\",\"body\":\"b\"}", true);
    let id = s.create(serde_json::json!({}));
    let task = done(&s, &id);
    let branch = task["branch"].as_str().unwrap().to_string();
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL));
    let task = wait_decision(&s, &id, "PR: drafted branch main refused");
    assert!(
        task["decisions"]
            .to_string()
            .contains(&format!("using {branch}")),
        "{task}"
    );
    assert!(
        s.gh_log().contains(&format!("--head {branch} --title t")),
        "{}",
        s.gh_log()
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_pull_request_opened_while_the_draft_runs_is_not_opened_twice() {
    let s = setup_with(
        "{\"branch\":\"feature/other\",\"title\":\"t\",\"body\":\"b\"}",
        true,
        true,
        false,
    );
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    wait_for(&s.tools.path().join("draft-started"));
    let opened = s.daemon.request("task.pr", serde_json::json!({"id": id}));
    assert_eq!(opened["prUrl"], URL, "{opened}");
    // A different head for the same task is refused while that one is open.
    let err = s.daemon.request_error(
        "task.pr",
        serde_json::json!({"id": id, "branch": "feature/second"}),
    );
    assert!(err.contains("already open"), "{err}");
    std::fs::write(s.tools.path().join("go"), "").unwrap();
    let task = wait_decision(&s, &id, "draft not used");
    assert!(task.get("prPending").is_none(), "{task}");
    assert_eq!(s.gh_log().matches("pr create").count(), 1, "{}", s.gh_log());
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_restart_resumes_the_pull_request_of_a_done_task() {
    let reply = "{\"branch\":\"feature/resumed\",\"title\":\"t\",\"body\":\"b\"}";
    let mut s = setup_with(reply, true, true, false);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    wait_for(&s.tools.path().join("draft-started"));
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert_eq!(task["prPending"], true, "{task}");
    let fresh = fake_harness_script(s.tools.path(), "fake-claude-2.sh", &script(reply, false));
    let gh = s.tools.path().join("fake-gh.sh");
    s.daemon.restart(&[
        ("ORCHD_CLAUDE_BIN", fresh.to_str().unwrap()),
        ("ORCHD_GH_BIN", gh.to_str().unwrap()),
    ]);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    std::fs::write(s.tools.path().join("go"), "").unwrap();
    assert!(
        s.gh_log().contains("--head feature/resumed"),
        "{}",
        s.gh_log()
    );
    s.daemon.shutdown_and_wait();
}

/// A gh that fails `pr list` while `$dir/fail-<kind>` exists with `stderr`.
const GH_FAILING: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
printf '%s\n' "$*" >> "$dir/gh.log"
case "$1 $2" in
  "pr list")
    if [ -f "$dir/auth-broken" ]; then
      echo "To get started with GitHub CLI, please run:  gh auth login" >&2
      exit 1
    fi
    n=$(cat "$dir/fails" 2>/dev/null || echo 0)
    if [ "$n" -lt "$(cat "$dir/fail-first" 2>/dev/null || echo 0)" ]; then
      echo $((n + 1)) > "$dir/fails"
      echo "error connecting to api.github.example: connection reset" >&2
      exit 1
    fi
    [ -f "$dir/created" ] && echo https://github.example/acme/app/pull/7; exit 0 ;;
  "pr create") touch "$dir/created"; echo https://github.example/acme/app/pull/7 ;;
esac
"#;

const FAST_RETRY: (&str, &str) = ("ORCHD_PR_RETRY_MS", "100,100,100,100");

/// Assistant notes in the repo's chat that mention a pull request.
fn notices(s: &Setup, id: &str) -> usize {
    note_texts(s, id).len()
}

/// Assistant notes in the repo's chat that mention a pull request.
fn note_texts(s: &Setup, id: &str) -> Vec<String> {
    let repo = s.daemon.request("task.get", serde_json::json!({"id": id}))["repo"]
        .as_str()
        .unwrap()
        .to_string();
    let thread = s
        .daemon
        .request("chat.get", serde_json::json!({"repo": repo}));
    thread["messages"]
        .as_array()
        .map(|m| {
            m.iter()
                .filter_map(|m| m["text"].as_str())
                .filter(|t| t.to_lowercase().contains("pull request"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn wait_notice(s: &Setup, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while notices(s, id) == 0 {
        assert!(Instant::now() < deadline, "no owner notice");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn default_settings_and_no_overseer_open_the_pull_request_of_a_refused_landing() {
    let reply = "{\"branch\":\"feature/by-default\",\"title\":\"feat: by default\",\"body\":\"b\"}";
    let s = setup_full(reply, Pr::Default, false, false, FAKE_GH, &[]);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let log = s.gh_log();
    assert!(
        log.contains("--head feature/by-default --title feat: by default"),
        "{log}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_task_with_land_off_gets_a_pull_request_too() {
    let s = setup_full("{}", Pr::Default, false, false, FAKE_GH, &[]);
    let id = s.create(serde_json::json!({"land": false}));
    let task = done(&s, &id);
    assert!(task.get("landedSha").is_none(), "{task}");
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_transient_gh_failure_is_retried_and_the_pull_request_opens_without_a_notice() {
    let s = setup_full("{}", Pr::Default, false, false, GH_FAILING, &[FAST_RETRY]);
    std::fs::write(s.tools.path().join("fail-first"), "2").unwrap();
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let task = wait_decision(&s, &id, "PR: try 2 failed");
    assert!(
        task["decisions"].to_string().contains("connection reset"),
        "{task}"
    );
    assert_eq!(s.gh_log().matches("pr create").count(), 1, "{}", s.gh_log());
    assert_eq!(notices(&s, &id), 0);
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_transient_failure_that_never_clears_stops_after_the_bound_with_one_notice() {
    let s = setup_full("{}", Pr::Default, false, false, GH_FAILING, &[FAST_RETRY]);
    std::fs::write(s.tools.path().join("fail-first"), "100").unwrap();
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    let task = wait_decision(&s, &id, "PR: not opened after 5 tries");
    assert_eq!(task["status"], "done", "{task}");
    wait_notice(&s, &id);
    settle();
    assert_eq!(s.gh_log().matches("pr list").count(), 5, "{}", s.gh_log());
    assert_eq!(notices(&s, &id), 1);
    s.daemon.shutdown_and_wait();
}

#[test]
fn an_auth_failure_sends_one_notice_does_not_loop_and_task_pr_opens_it_later() {
    let s = setup_full("{}", Pr::Default, false, false, GH_FAILING, &[FAST_RETRY]);
    std::fs::write(s.tools.path().join("auth-broken"), "").unwrap();
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    let task = wait_decision(&s, &id, "PR: not opened: ");
    assert_eq!(task["status"], "done", "{task}");
    assert!(
        task["decisions"].to_string().contains("gh auth login"),
        "{task}"
    );
    wait_notice(&s, &id);
    settle();
    assert_eq!(s.gh_log().matches("pr list").count(), 1, "{}", s.gh_log());
    assert_eq!(notices(&s, &id), 1);
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(task.get("prUrl").is_none() && task.get("prPending").is_none());
    // Logged in again: the owner opens it later.
    std::fs::remove_file(s.tools.path().join("auth-broken")).unwrap();
    let opened = s.daemon.request("task.pr", serde_json::json!({"id": id}));
    assert_eq!(opened["prUrl"], URL, "{opened}");
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_base_checkout_that_stays_dirty_leaves_the_work_on_its_branch_and_opens_the_pull_request() {
    let s = setup_full(
        "{}",
        Pr::Default,
        false,
        true,
        FAKE_GH,
        &[("ORCHD_LANDING_DIRTY_LIMIT_MS", "4000")],
    );
    std::fs::write(s.repo.path().join("README.md"), "hello\nmy edit\n").unwrap();
    let id = s.create(serde_json::json!({}));
    let waiting = poll_until(&s.daemon, &id, |st| {
        matches!(st, "landing" | "done" | "failed" | "stopped")
    });
    assert_eq!(waiting["status"], "landing", "{waiting}");
    // A retry before the bound changes nothing: the retry is its own loop, so
    // wait for its decision to be the only state we can see.
    s.daemon
        .request("task.start", serde_json::json!({"id": id}));
    std::thread::sleep(Duration::from_millis(1000));
    let early = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert_eq!(early["status"], "landing", "{early}");
    assert!(
        !early["decisions"].to_string().contains("Land: gave up"),
        "{early}"
    );
    std::thread::sleep(Duration::from_millis(3500));
    s.daemon
        .request("task.start", serde_json::json!({"id": id}));
    let task = done(&s, &id);
    assert!(task.get("landedSha").is_none(), "{task}");
    assert!(
        task["decisions"].to_string().contains("Land: gave up"),
        "{task}"
    );
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    assert_eq!(
        std::fs::read_to_string(s.repo.path().join("README.md")).unwrap(),
        "hello\nmy edit\n"
    );
    wait_notice(&s, &id);
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_dirty_give_up_does_not_promise_a_pull_request_when_they_are_off() {
    let s = setup_full(
        "{}",
        Pr::Off,
        false,
        true,
        FAKE_GH,
        &[("ORCHD_LANDING_DIRTY_LIMIT_MS", "300")],
    );
    std::fs::write(s.repo.path().join("README.md"), "hello\nmy edit\n").unwrap();
    let id = s.create(serde_json::json!({}));
    poll_until(&s.daemon, &id, |st| st == "landing");
    std::thread::sleep(Duration::from_millis(600));
    s.daemon
        .request("task.start", serde_json::json!({"id": id}));
    done(&s, &id);
    wait_notice(&s, &id);
    let texts = note_texts(&s, &id);
    assert!(texts.iter().all(|t| !t.contains("is opened")), "{texts:?}");
    assert!(s.gh_log().is_empty(), "{}", s.gh_log());
    s.daemon.shutdown_and_wait();
}

#[test]
fn landing_a_done_task_while_its_pull_request_is_drafted_cancels_the_pull_request() {
    let reply = "{\"branch\":\"feature/late\",\"title\":\"t\",\"body\":\"b\"}";
    let s = setup_full(reply, Pr::Default, true, false, FAKE_GH, &[]);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    wait_for(&s.tools.path().join("draft-started"));
    // The owner turns landing on and lands the task while the draft runs.
    let mut settings = s.daemon.request("settings.get", serde_json::json!({}));
    settings["landOnDefaultRepos"] = serde_json::json!({ s.repo.path().to_str().unwrap(): true });
    s.daemon
        .request("settings.set", serde_json::json!({"settings": settings}));
    s.daemon.request("task.land", serde_json::json!({"id": id}));
    let landed = poll_until(&s.daemon, &id, |st| st == "done" || st == "failed");
    assert!(landed["landedSha"].is_string(), "{landed}");
    std::fs::write(s.tools.path().join("go"), "").unwrap();
    wait_for(&s.tools.path().join("drafts.log"));
    settle();
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    assert!(
        task.get("prPending").is_none() && task.get("prUrl").is_none(),
        "{task}"
    );
    assert!(
        !task["decisions"].to_string().contains("PR: not opened"),
        "{task}"
    );
    assert!(!s.gh_log().contains("pr "), "{}", s.gh_log());
    assert_eq!(notices(&s, &id), 0);
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_restart_reuses_the_saved_draft_instead_of_drafting_again() {
    let reply = "{\"branch\":\"feature/kept\",\"title\":\"t\",\"body\":\"b\"}";
    let mut s = setup_full(reply, Pr::Default, false, false, GH_FAILING, &[]);
    std::fs::write(s.tools.path().join("fail-first"), "1").unwrap();
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    // The first try fails and the flow sleeps with the draft saved.
    wait_decision(&s, &id, "PR: try 1 failed");
    let drafts = |s: &Setup| {
        std::fs::read_to_string(s.tools.path().join("drafts.log"))
            .unwrap_or_default()
            .lines()
            .count()
    };
    assert_eq!(drafts(&s), 1);
    let other = s.tools.path().join("fake-claude-b.sh");
    std::fs::copy(s.tools.path().join("fake-claude.sh"), &other).unwrap();
    let gh = s.tools.path().join("fake-gh.sh");
    s.daemon.restart(&[
        ("ORCHD_CLAUDE_BIN", other.to_str().unwrap()),
        ("ORCHD_GH_BIN", gh.to_str().unwrap()),
    ]);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    assert_eq!(drafts(&s), 1, "drafted again after the restart");
    assert!(s.gh_log().contains("--head feature/kept"), "{}", s.gh_log());
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_repo_without_an_origin_is_noted_once_with_no_draft_and_no_notice() {
    let s = setup_full("{}", Pr::Default, false, false, FAKE_GH, &[]);
    git_out(s.repo.path(), &["remote", "remove", "origin"]);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    let task = wait_decision(&s, &id, "no origin remote");
    assert_eq!(task["status"], "done", "{task}");
    settle();
    assert!(
        !s.tools.path().join("drafts.log").exists(),
        "a draft ran without an origin"
    );
    assert!(s.gh_log().is_empty(), "{}", s.gh_log());
    assert_eq!(notices(&s, &id), 0);
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_remote_that_is_no_github_host_gets_no_draft_and_no_push() {
    let gh = GH_FAILING.replace(
        "case \"$1 $2\" in",
        "case \"$1 $2\" in\n  \"repo view\") echo \"none of the git remotes configured for this repository point to a known GitHub host\" >&2; exit 1 ;;",
    );
    let s = setup_full("{}", Pr::Default, false, false, &gh, &[FAST_RETRY]);
    let first = s.create(serde_json::json!({}));
    done(&s, &first);
    wait_decision(&s, &first, "known GitHub host");
    let second = s.create(serde_json::json!({"title": "Second"}));
    done(&s, &second);
    wait_decision(&s, &second, "known GitHub host");
    settle();
    assert!(
        !s.tools.path().join("drafts.log").exists(),
        "a draft ran for a non-GitHub remote"
    );
    assert_eq!(s.gh_log().matches("pr list").count(), 0, "{}", s.gh_log());
    let heads = git_out(s._remote.path(), &["for-each-ref", "refs/heads"]);
    assert_eq!(heads.lines().count(), 1, "pushed: {heads}");
    assert_eq!(notices(&s, &first), 0);
    s.daemon.shutdown_and_wait();
}

#[test]
fn the_overseer_cannot_open_a_pull_request_orchd_owns() {
    let reply = "{\"branch\":\"feature/own\",\"title\":\"t\",\"body\":\"b\"}";
    let s = setup_full(reply, Pr::Default, true, false, FAKE_GH, &[]);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    wait_for(&s.tools.path().join("draft-started"));
    let err = s.daemon.request_error(
        "task.pr",
        serde_json::json!({"id": id, "source": "overseer"}),
    );
    assert!(err.contains("orchd opens this task"), "{err}");
    assert!(!s.gh_log().contains("pr "), "{}", s.gh_log());
    // The owner may still open it.
    let opened = s.daemon.request("task.pr", serde_json::json!({"id": id}));
    assert_eq!(opened["prUrl"], URL, "{opened}");
    std::fs::write(s.tools.path().join("go"), "").unwrap();
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_done_task_whose_base_moved_is_carried_onto_the_new_base_before_the_push() {
    let s = setup_full("{}", Pr::Default, false, false, FAKE_GH, &[]);
    // Origin's main moves on while the local main (the task's base) stays.
    let other = tempfile::tempdir().unwrap();
    let remote = s._remote.path().to_str().unwrap();
    git_out(other.path(), &["clone", "-q", remote, "."]);
    git_out(
        other.path(),
        &["config", "user.email", "other@example.invalid"],
    );
    git_out(other.path(), &["config", "user.name", "other"]);
    std::fs::write(other.path().join("newer.txt"), "newer\n").unwrap();
    git_out(other.path(), &["add", "."]);
    git_out(other.path(), &["commit", "-q", "-m", "newer main"]);
    git_out(other.path(), &["push", "-q", "origin", "main"]);
    let newer = git_out(other.path(), &["rev-parse", "HEAD"]);
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    let branch = format!("refs/heads/{}", task["branch"].as_str().unwrap());
    let pushed = git_out(s._remote.path(), &["rev-parse", &branch]);
    git_out(
        s._remote.path(),
        &["merge-base", "--is-ancestor", &newer, &pushed],
    );
    let files = git_out(s._remote.path(), &["ls-tree", "-r", "--name-only", &pushed]);
    assert!(
        files.contains("pr.txt") && files.contains("newer.txt"),
        "{files}"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_workflow_scope_rejection_is_permanent_with_one_try_and_one_note() {
    let s = setup_full("{}", Pr::Default, false, false, FAKE_GH, &[FAST_RETRY]);
    let hook = s._remote.path().join("hooks/pre-receive");
    let log = s._remote.path().join("rejects.log");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\necho x >> '{}'\necho \"error: refusing to allow a Personal Access Token to create or update workflow \\`.github/workflows/ci.yml\\` without \\`workflow\\` scope\" >&2\nexit 1\n",
            log.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let id = s.create(serde_json::json!({}));
    done(&s, &id);
    let task = wait_decision(&s, &id, "PR: not opened: ");
    assert!(
        task["decisions"].to_string().contains("`workflow` scope"),
        "{task}"
    );
    wait_notice(&s, &id);
    settle();
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    assert_eq!(notices(&s, &id), 1);
    assert!(
        note_texts(&s, &id)[0].contains("workflow"),
        "{:?}",
        note_texts(&s, &id)
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn an_unreviewed_task_is_not_landed_and_its_pull_request_opens_with_the_unreviewed_line() {
    // The fake answers a review brief with a report and no verdict, so no
    // route reads the attempt, even with landing on.
    let reply = "{\"branch\":\"\",\"title\":\"\",\"body\":\"\"}";
    let s = setup_with(reply, true, false, true);
    let mut settings = s.daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("claude-opus");
    s.daemon
        .request("settings.set", serde_json::json!({"settings": settings}));
    let id = s.create(serde_json::json!({}));
    let task = done(&s, &id);
    assert_eq!(task["unreviewed"], true, "{task}");
    assert!(task.get("landedSha").is_none(), "{task}");
    assert_eq!(s.pr_url(&id).as_deref(), Some(URL), "{}", s.gh_log());
    let log = s.gh_log();
    assert!(log.contains("pr create"), "{log}");
    assert!(
        log.contains("Unreviewed: no review route gave a verdict"),
        "{log}"
    );
    s.daemon.shutdown_and_wait();
}
