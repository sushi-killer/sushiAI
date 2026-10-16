//! After orchd opens a pull request it watches the PR's GitHub checks. A red
//! check that is the task's own goes back to the task for one repair step and
//! the fix is pushed to the same pull request; a red check that is not the
//! task's (the failed jobs pass on a rerun, or the base branch fails the same
//! test) is told to the owner once and the task is left alone.

mod common;

use common::harness::{cheap_route_on_claude, fake_harness_script};
use common::polling::{poll_until, wait_until};
use common::process::Daemon;
use common::repository::{git_out, init_git_repo};
use std::time::Duration;

const URL: &str = "https://github.example/acme/app/pull/7";

/// A gh whose checks follow `$dir/scenario`: `flaky` passes after a rerun of
/// the failed jobs, `base` fails the same test on the base branch too, `own`
/// fails until the pull request head moves. `pr checks` exits 8 while pending
/// and 1 while failing, like the real gh.
const FAKE_GH: &str = r####"#!/bin/sh
dir="$(dirname "$0")"
echo "$*" >> "$dir/gh.log"
scenario=$(cat "$dir/scenario")
url=https://github.example/acme/app/pull/7
head_sha() { git ls-remote origin "refs/heads/$(cat "$dir/head")" | cut -f1; }
pr_state() {
  sha=$(head_sha)
  [ -f "$dir/first-sha" ] || printf '%s' "$sha" > "$dir/first-sha"
  reruns=$(cat "$dir/reruns" 2>/dev/null || echo 0)
  if [ "$sha" != "$(cat "$dir/first-sha")" ]; then echo pass
  elif [ "$scenario" = flaky ] && [ "$reruns" -ge 1 ]; then echo pass
  else echo fail
  fi
}
case "$1 $2" in
  "pr list") [ -f "$dir/created" ] && echo "$url"; exit 0 ;;
  "pr create")
    while [ $# -gt 0 ]; do
      case "$1" in --head) printf '%s' "$2" > "$dir/head" ;; --title) printf '%s' "$2" > "$dir/title" ;; esac
      shift
    done
    touch "$dir/created"; echo "$url" ;;
  "pr view")
    echo '{"url":"'"$url"'","headRefOid":"'"$(head_sha)"'","title":"'"$(cat "$dir/title")"'"}' ;;
  "pr checks")
    n=$(cat "$dir/checks-calls" 2>/dev/null || echo 0)
    echo $((n + 1)) > "$dir/checks-calls"
    bucket=$(pr_state); code=0
    [ "$bucket" = fail ] && code=1
    if [ "$n" -lt 1 ]; then bucket=pending; code=8; fi
    echo '[{"name":"unit","bucket":"'"$bucket"'","workflow":"ci","link":"https://github.example/acme/app/actions/runs/555/job/1"}]'
    exit $code ;;
  "run list")
    case "$*" in
      *"--branch main"*) id=900; c=success; [ "$scenario" = base ] && c=failure ;;
      *) id=555; c=failure; [ "$(pr_state)" = pass ] && c=success ;;
    esac
    echo '[{"databaseId":'"$id"',"status":"completed","conclusion":"'"$c"'","workflowName":"ci","headSha":"abc"}]' ;;
  "run view") cat "$dir/failed-log" ;;
  "run rerun")
    n=$(cat "$dir/reruns" 2>/dev/null || echo 0)
    echo $((n + 1)) > "$dir/reruns" ;;
esac
"####;

/// One fake claude for every role: the drafter, the body check, the
/// attribution turn, the repair step and the first implementation.
const FAKE_CLAUDE: &str = r####"#!/bin/sh
dir="$(dirname "$0")"
brief=$(cat)
case "$brief" in
"## Pull request draft"*) cat "$dir/draft-reply" ;;
"## Pull request body check"*) cat "$dir/check-reply" ;;
*"## PR check repair"*)
  printf '%s' "$brief" > "$dir/repair-brief.md"
  echo fixed > fix.txt
  echo '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  cat "$dir/report-reply" ;;
*sushi-attribution*)
  echo asked >> "$dir/attribution.log"
  cat "$dir/attribution-reply" ;;
*)
  echo pr.txt > pr.txt
  echo '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  cat "$dir/report-reply" ;;
esac
"####;

struct Setup {
    daemon: Daemon,
    tools: tempfile::TempDir,
    repo: tempfile::TempDir,
    _remote: tempfile::TempDir,
}

impl Setup {
    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.tools.path().join(name)).unwrap_or_default()
    }

    fn remote_sha(&self) -> String {
        let head = self.read("head");
        git_out(
            self.repo.path(),
            &["ls-remote", "origin", &format!("refs/heads/{head}")],
        )
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
    }

    fn task(&self, id: &str) -> serde_json::Value {
        self.daemon
            .request("task.get", serde_json::json!({"id": id}))
    }

    /// Assistant notes in the repo's chat that name `test`.
    fn notes_naming(&self, id: &str, test: &str) -> Vec<String> {
        let repo = self.task(id)["repo"].as_str().unwrap().to_string();
        let thread = self
            .daemon
            .request("chat.get", serde_json::json!({"repo": repo}));
        thread["messages"]
            .as_array()
            .map(|m| {
                m.iter()
                    .filter_map(|m| m["text"].as_str())
                    .filter(|t| t.contains(test))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn reply(text: &str) -> String {
    serde_json::json!({
        "type": "result",
        "total_cost_usd": 0.01,
        "usage": {"input_tokens": 1, "output_tokens": 1},
        "result": text,
    })
    .to_string()
}

fn setup(scenario: &str) -> Setup {
    let tools = tempfile::tempdir().unwrap();
    let test = if scenario == "own" {
        "routines::own_case"
    } else {
        "routines::flaky_case"
    };
    let write = |name: &str, body: String| {
        std::fs::write(tools.path().join(name), body + "\n").unwrap();
    };
    write("scenario", scenario.to_string());
    write(
        "failed-log",
        format!("unit\tRun tests\t2026-10-16T10:00:00.0000000Z test {test} ... FAILED\nunit\tRun tests\t2026-10-16T10:00:01.0000000Z test result: FAILED. 0 passed; 1 failed"),
    );
    write(
        "draft-reply",
        reply(r#"{"branch":"","title":"t","body":"b"}"#),
    );
    write("check-reply", reply(r#"{"ok":true,"corrections":[]}"#));
    let verdict = serde_json::json!({
        "cause": "introduced",
        "new_failures": [test],
        "evidence": "fails only with this change",
    });
    write(
        "attribution-reply",
        reply(&format!("```sushi-attribution\n{verdict}\n```")),
    );
    let report = serde_json::json!({
        "outcome": "complete",
        "summary": "done",
        "decisions": [],
        "question": "",
    });
    write(
        "report-reply",
        reply(&format!("```sushi-report\n{report}\n```")),
    );
    let harness = fake_harness_script(tools.path(), "fake-claude.sh", FAKE_CLAUDE);
    let gh = fake_harness_script(tools.path(), "fake-gh.sh", FAKE_GH);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
        ("ORCHD_GH_BIN", gh.to_str().unwrap()),
        ("ORCHD_PR_CI_POLL_MS", "100"),
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
    settings["prOnDoneRepos"] = serde_json::json!({ repo.path().to_str().unwrap(): true });
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    cheap_route_on_claude(&daemon);
    Setup {
        daemon,
        tools,
        repo,
        _remote: remote,
    }
}

/// Creates the task, waits until it is done and its pull request is open, and
/// returns the task as it was when it first became done.
fn opened(s: &Setup) -> (String, serde_json::Value) {
    let id = s.daemon.request(
        "task.create",
        serde_json::json!({
            "repo": s.repo.path().to_str().unwrap(),
            "title": "Add pr file",
            "goal": "write pr.txt",
            "criteria": ["pr.txt exists"],
            "verify": ["test -f pr.txt"],
            "start": true,
        }),
    )["id"]
        .as_str()
        .unwrap()
        .to_string();
    let done = poll_until(&s.daemon, &id, |st| {
        matches!(st, "done" | "failed" | "stopped")
    });
    assert_eq!(done["status"], "done", "{done}");
    wait_until("the pull request", || {
        s.task(&id)["prUrl"].as_str().map(str::to_string)
    });
    (id, done)
}

fn attempts(task: &serde_json::Value) -> usize {
    task["attempts"].as_array().map_or(0, Vec::len)
}

#[test]
fn a_check_that_passes_on_a_rerun_is_told_once_and_the_task_is_left_alone() {
    let s = setup("flaky");
    let (id, done) = opened(&s);
    let note = wait_until("the owner note", || {
        s.notes_naming(&id, "routines::flaky_case")
            .into_iter()
            .next()
    });
    assert!(note.to_lowercase().contains("rerun"), "{note}");
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(s.notes_naming(&id, "routines::flaky_case").len(), 1);
    assert_eq!(s.read("reruns").trim(), "1", "{}", s.read("gh.log"));
    assert!(
        s.read("gh.log").contains("run rerun 555"),
        "{}",
        s.read("gh.log")
    );
    let task = s.task(&id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(attempts(&task), attempts(&done), "{task}");
    assert_eq!(s.remote_sha(), s.read("first-sha"));
    assert!(!s.tools.path().join("attribution.log").exists());
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_test_that_also_fails_on_the_base_branch_is_told_once_and_the_task_is_left_alone() {
    let s = setup("base");
    let (id, done) = opened(&s);
    let note = wait_until("the owner note", || {
        s.notes_naming(&id, "routines::flaky_case")
            .into_iter()
            .next()
    });
    assert!(note.contains("main"), "{note}");
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(s.notes_naming(&id, "routines::flaky_case").len(), 1);
    assert!(s.read("reruns").trim().parse::<u32>().unwrap_or(0) <= 1);
    let task = s.task(&id);
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(attempts(&task), attempts(&done), "{task}");
    assert_eq!(s.remote_sha(), s.read("first-sha"));
    assert!(
        !s.tools.path().join("attribution.log").exists(),
        "the names match the base run: no agent turn is needed"
    );
    s.daemon.shutdown_and_wait();
}

#[test]
fn a_failure_of_the_tasks_own_change_is_repaired_and_pushed_to_the_same_pull_request() {
    let s = setup("own");
    let (id, done) = opened(&s);
    wait_until("the fix pushed to the same branch", || {
        let first = s.read("first-sha");
        (!first.is_empty() && s.remote_sha() != first).then_some(())
    });
    let task = poll_until(&s.daemon, &id, |st| st == "done");
    assert_eq!(task["prUrl"], URL, "{task}");
    assert_eq!(attempts(&task), attempts(&done) + 1, "{task}");
    let creates = s
        .read("gh.log")
        .lines()
        .filter(|line| line.starts_with("pr create"))
        .count();
    assert_eq!(creates, 1, "{}", s.read("gh.log"));
    assert!(
        s.read("repair-brief.md").contains("routines::own_case"),
        "the repair brief must carry the failing job log"
    );
    assert!(s.tools.path().join("attribution.log").exists());
    assert_eq!(s.read("reruns").trim(), "1", "{}", s.read("gh.log"));
    std::thread::sleep(Duration::from_secs(3));
    assert!(s.notes_naming(&id, "routines::own_case").is_empty());
    s.daemon.shutdown_and_wait();
}
