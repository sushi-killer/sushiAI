//! An overseer wake runs on a fresh harness session, leaves the owner's session
//! alone and keeps a small state file for the repo.

mod common;
use common::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

/// Numbers its calls and logs each call's argv and prompt. An implement run
/// replies with a report, every other run with `Reply <n>`.
const FAKE: &str = r###"#!/bin/sh
p=$(cat)
n=$(cat "$COUNT_FILE" 2>/dev/null || echo 0)
n=$((n+1))
echo $n > "$COUNT_FILE"
printf '%s\n' "$@" > "$LOG_DIR/argv-$n"
printf '%s' "$p" > "$LOG_DIR/prompt-$n"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-'$n'"}'
case "$p" in
*"## Report format"*)
  echo changed > CHANGED_MARKER.txt
  r='```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```' ;;
*)
  r='Reply '$n ;;
esac
printf '%s\n' '{"type":"result","session_id":"sess-'$n'","total_cost_usd":0.01,"result":"'"$r"'"}'
"###;

struct Env {
    daemon: Daemon,
    scripts: tempfile::TempDir,
    repo: tempfile::TempDir,
}

impl Env {
    fn path(&self) -> String {
        self.repo.path().to_str().unwrap().to_string()
    }

    fn until(&self, what: &str, done: impl Fn(&Env) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(start.elapsed() < Duration::from_secs(30), "{what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn state_file(&self) -> Option<std::path::PathBuf> {
        let dir = self.daemon.data_dir().join("overseer");
        std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "md"))
    }

    fn state(&self) -> String {
        self.state_file()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default()
    }

    fn decisions(&self) -> usize {
        let state = self.state();
        state
            .split("## Decisions\n")
            .nth(1)
            .map_or(0, |d| d.lines().filter(|l| l.starts_with("- ")).count())
    }

    fn idle(&self) -> bool {
        let thread = self
            .daemon
            .request("chat.get", json!({"repo": self.path()}));
        thread["busy"] == json!(false)
    }

    fn argv(&self, n: usize) -> Vec<String> {
        std::fs::read_to_string(self.scripts.path().join(format!("argv-{n}")))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn prompt(&self, n: usize) -> String {
        std::fs::read_to_string(self.scripts.path().join(format!("prompt-{n}"))).unwrap()
    }

    fn calls(&self) -> usize {
        std::fs::read_to_string(self.scripts.path().join("count"))
            .ok()
            .and_then(|c| c.trim().parse().ok())
            .unwrap_or(0)
    }

    /// The calls whose prompt opens with the state header, in order.
    fn wakes(&self) -> Vec<usize> {
        (1..=self.calls())
            .filter(|n| self.prompt(*n).starts_with("Overseer state for this repo"))
            .collect()
    }

    fn task(&self, title: &str, goal: &str) -> String {
        let task = self.daemon.request(
            "task.create",
            json!({
                "repo": self.path(),
                "title": title,
                "goal": goal,
                "criteria": [format!("{title} criterion")],
                "verify": ["true"],
                "start": true,
            }),
        );
        task["id"].as_str().unwrap().to_string()
    }
}

fn resume_of(argv: &[String]) -> Option<&String> {
    let at = argv.iter().position(|a| a == "--resume")?;
    argv.get(at + 1)
}

#[test]
fn wakes_start_fresh_sessions_and_keep_a_state_file() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude-wake.sh", FAKE);
    let count = scripts.path().join("count");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("COUNT_FILE", count.to_str().unwrap()),
        ("LOG_DIR", scripts.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["autoAnswer"] = json!(true);
    settings["chatTools"] = json!([]);
    daemon.request("settings.set", json!({"settings": settings}));
    let env = Env {
        daemon,
        scripts,
        repo: init_git_repo(),
    };

    // The owner talks once: call 1 holds session sess-1.
    env.daemon
        .request("chat.send", json!({"repo": env.path(), "text": "hello"}));
    env.until("owner turn", |e| e.calls() == 1 && e.idle());

    let first = env.task("First task", "FIRST-GOAL-77");
    let settled = settle(&env.daemon, &first);
    assert_eq!(settled["status"], "done", "{settled}");
    env.until("first wake", |e| e.decisions() == 1 && e.idle());

    let file = env.state_file().expect("state file");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let state = env.state();
    assert!(state.contains("First task"), "{state}");
    assert!(state.contains("done"), "{state}");
    assert!(state.contains("FIRST-GOAL-77"), "{state}");
    assert!(state.contains("First task criterion"), "{state}");
    let wakes = env.wakes();
    assert_eq!(wakes.len(), 1, "{wakes:?}");
    assert!(env.prompt(wakes[0]).contains("FIRST-GOAL-77"));
    assert_eq!(resume_of(&env.argv(wakes[0])), None);

    let second = env.task("Second task", "SECOND-GOAL-88");
    let settled = settle(&env.daemon, &second);
    assert_eq!(settled["status"], "done", "{settled}");
    env.until("second wake", |e| e.decisions() == 2 && e.idle());
    let wakes = env.wakes();
    assert_eq!(wakes.len(), 2, "{wakes:?}");
    assert!(env.prompt(wakes[1]).contains("SECOND-GOAL-88"));
    assert_eq!(resume_of(&env.argv(wakes[1])), None);

    // A question from a task wakes the overseer too, also on a fresh session.
    env.daemon.request(
        "message.send",
        json!({"from": first, "to": "orchestrator", "text": "Which branch?"}),
    );
    env.until("question wake", |e| e.decisions() == 3 && e.idle());
    let wakes = env.wakes();
    assert_eq!(wakes.len(), 3, "{wakes:?}");
    assert_eq!(resume_of(&env.argv(wakes[2])), None);
    assert!(env.prompt(wakes[2]).contains("Which branch?"));
    assert!(env.state().contains(" questions -: Reply "));

    // The owner's next turn resumes the session from before the wakes.
    let before = env.calls();
    env.daemon.request(
        "chat.send",
        json!({"repo": env.path(), "text": "and again"}),
    );
    env.until("owner turn", |e| e.calls() == before + 1 && e.idle());
    assert_eq!(
        resume_of(&env.argv(before + 1)).map(String::as_str),
        Some("sess-1")
    );
    // An owner turn adds no decision.
    assert_eq!(env.decisions(), 3);

    for id in [&first, &second] {
        let task = env.daemon.request("task.get", json!({"id": id}));
        let _ = std::fs::remove_dir_all(task["worktree"].as_str().unwrap());
    }
}
