//! A done top-level task whose work did not land gets a pull request drafted
//! by a read-only helper and opened through `task.pr`, by default and without
//! the overseer.

mod common;

use common::harness::{cheap_route_on_claude, fake_harness_script};
use common::polling::{holds_for, poll_until, wait_state, wait_task, ABSENCE_WINDOW};
use common::process::Daemon;
use common::repository::{git_out, init_git_repo};

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

/// The body check's reply, as the fake harness prints it.
fn truth_reply(verdict: serde_json::Value) -> String {
    serde_json::json!({"type": "result", "result": verdict.to_string()}).to_string()
}

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
"## Pull request body check"*)
echo check >> "$(dirname "$0")/checks.log"
cat "$(dirname "$0")/truth-reply"; echo
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
        Some(wait_task(
            &self.daemon,
            id,
            "the pull request url",
            |task| task["prUrl"].as_str().map(str::to_string),
        ))
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
    setup_full(&script(reply, hold), pr, land, FAKE_GH, &[])
}

fn setup_full(
    harness_body: &str,
    pr: Pr,
    land: bool,
    gh_body: &str,
    env: &[(&str, &str)],
) -> Setup {
    let tools = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(tools.path(), "fake-claude.sh", harness_body);
    let gh = fake_harness_script(tools.path(), "fake-gh.sh", gh_body);
    let ok = truth_reply(serde_json::json!({"ok": true, "corrections": []}));
    std::fs::write(tools.path().join("truth-reply"), ok).unwrap();
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

/// Absence check: gives a forbidden pull request, draft or second notice time to happen.
fn stays(what: &str, still: impl FnMut() -> bool) {
    holds_for(what, ABSENCE_WINDOW, still);
}

/// Waits until the helper script wrote `name` into the tools directory.
fn wait_for(s: &Setup, name: &str) {
    let path = s.tools.path().join(name);
    wait_state(
        &format!("{} to appear", path.display()),
        || {
            if path.exists() {
                Ok(())
            } else {
                Err(format!(
                    "{name} missing; tools: {:?}",
                    dir_names(s.tools.path())
                ))
            }
        },
        || common::polling::daemon_log_tail(&s.daemon),
    );
}

fn dir_names(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn wait_decision(s: &Setup, id: &str, text: &str) -> serde_json::Value {
    wait_task(&s.daemon, id, &format!("the decision {text:?}"), |task| {
        task["decisions"]
            .to_string()
            .contains(text)
            .then(|| task.clone())
    })
}

/// Restarts the daemon after `plan` was written into the held task's file,
/// the way an earlier run left it, and lets the fresh script draft `reply`.
fn restart_with_plan(s: &mut Setup, id: &str, plan: serde_json::Value, reply: &str) {
    let path = s.daemon.data_dir().join("tasks").join(id).join("task.json");
    let mut task: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    task["prPlan"] = plan;
    std::fs::write(&path, task.to_string()).unwrap();
    let fresh = fake_harness_script(s.tools.path(), "fake-claude-2.sh", &script(reply, false));
    let gh = s.tools.path().join("fake-gh.sh");
    s.daemon.restart(&[
        ("ORCHD_CLAUDE_BIN", fresh.to_str().unwrap()),
        ("ORCHD_GH_BIN", gh.to_str().unwrap()),
    ]);
}

fn held_task(s: &Setup) -> (String, String) {
    let id = s.create(serde_json::json!({}));
    done(s, &id);
    wait_for(s, "draft-started");
    let task = s.daemon.request("task.get", serde_json::json!({"id": id}));
    let branch = task["branch"].as_str().unwrap().to_string();
    (id, branch)
}

fn drafts(s: &Setup) -> usize {
    std::fs::read_to_string(s.tools.path().join("drafts.log"))
        .map(|log| log.lines().count())
        .unwrap_or(0)
}

const DRAFT: &str = "{\"branch\":\"feature/drafted-name\",\"title\":\"feat: drafted title\",\"body\":\"Drafted body\"}";

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
    wait_state(
        "the owner notice",
        || match note_texts(s, id) {
            notes if !notes.is_empty() => Ok(()),
            _ => Err("no pull request note in the repo chat".to_string()),
        },
        || common::polling::daemon_log_tail(&s.daemon),
    );
}

/// Commits a worktree setup to main and origin, moves origin's main on, and
/// returns the task request whose check needs what the setup creates.
fn setup_and_moved_base(s: &Setup, setup: &str) -> serde_json::Value {
    std::fs::create_dir_all(s.repo.path().join(".sushiai")).unwrap();
    std::fs::write(
        s.repo.path().join(".sushiai/setup.json"),
        serde_json::json!({ "setup": setup }).to_string(),
    )
    .unwrap();
    git_out(s.repo.path(), &["add", "."]);
    git_out(s.repo.path(), &["commit", "-q", "-m", "add setup"]);
    git_out(s.repo.path(), &["push", "-q", "origin", "main"]);
    let other = tempfile::tempdir().unwrap();
    git_out(
        other.path(),
        &["clone", "-q", s._remote.path().to_str().unwrap(), "."],
    );
    git_out(
        other.path(),
        &["config", "user.email", "other@example.invalid"],
    );
    git_out(other.path(), &["config", "user.name", "other"]);
    std::fs::write(other.path().join("newer.txt"), "newer\n").unwrap();
    git_out(other.path(), &["add", "."]);
    git_out(other.path(), &["commit", "-q", "-m", "newer main"]);
    git_out(other.path(), &["push", "-q", "origin", "main"]);
    serde_json::json!({"verify": ["test -f pr.txt && test -f .env-ready"]})
}

#[path = "pr_on_done/carry.rs"]
mod carry_tests;
#[path = "pr_on_done/failure.rs"]
mod failure_tests;
#[path = "pr_on_done/publication.rs"]
mod publication_tests;
#[path = "pr_on_done/restart.rs"]
mod restart_tests;
