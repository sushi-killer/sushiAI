//! Chat modes and proposed-task lists, through a real daemon whose chat
//! harness is a fake that answers by prompt text and records its argv.

mod common;
use common::*;
use serde_json::json;
use std::time::{Duration, Instant};

const FAKE: &str = r##"#!/bin/sh
p=$(cat)
printf '%s\n' "$@" > "$ARGV_OUT"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-chat"}'
case "$p" in
*zzplan*)
  r='Plan below.\n```sushi-draft\n{\"tasks\":[{\"title\":\"First\",\"goal\":\"g1\",\"criteria\":[\"c1\"],\"tier\":\"hard\"},{\"title\":\"Second\",\"goal\":\"g2\",\"criteria\":[\"c2\"],\"dependsOn\":[1],\"tier\":\"standard\"},{\"title\":\"Third\",\"dependsOn\":[\"2\"]}]}\n```' ;;
*zzbad*)
  r='Oops.\n```sushi-draft\n{\"tasks\":[{\"title\":\"A\",\"dependsOn\":[5]}]}\n```' ;;
*)
  r='Plain reply.' ;;
esac
printf '%s\n' '{"type":"result","session_id":"sess-chat","result":"'"$r"'"}'
"##;

struct Env {
    daemon: Daemon,
    _scripts: tempfile::TempDir,
    argv: std::path::PathBuf,
    repo: tempfile::TempDir,
}

fn spawn() -> Env {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude-modes.sh", FAKE);
    let argv = scripts.path().join("argv");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("ARGV_OUT", argv.to_str().unwrap()),
    ]);
    Env {
        daemon,
        _scripts: scripts,
        argv,
        repo: init_git_repo(),
    }
}

impl Env {
    fn path(&self) -> String {
        self.repo.path().to_str().unwrap().to_string()
    }

    fn say(&self, text: &str, mode: Option<&str>) -> serde_json::Value {
        let mut params = json!({"repo": self.path(), "text": text});
        if let Some(mode) = mode {
            params["mode"] = json!(mode);
        }
        self.daemon.request("chat.send", params);
        let start = Instant::now();
        loop {
            let thread = self
                .daemon
                .request("chat.get", json!({"repo": self.path()}));
            if thread["busy"] == json!(false)
                && thread["messages"]
                    .as_array()
                    .unwrap()
                    .len()
                    .is_multiple_of(2)
            {
                return thread;
            }
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "turn never ended"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn argv(&self) -> Vec<String> {
        std::fs::read_to_string(&self.argv)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn tasks(&self) -> Vec<serde_json::Value> {
        self.daemon
            .request("task.list", json!({}))
            .as_array()
            .unwrap()
            .clone()
    }
}

fn last(thread: &serde_json::Value) -> &serde_json::Value {
    thread["messages"].as_array().unwrap().last().unwrap()
}

fn flag_value(argv: &[String], flag: &str) -> String {
    argv[argv.iter().position(|a| a == flag).unwrap() + 1].clone()
}

#[test]
fn a_mode_is_per_message_and_picks_the_prompt_and_tools_of_each_turn() {
    let env = spawn();
    let thread = env.say("zzplan please", Some("plan"));
    assert_eq!(thread["mode"], "plan");
    assert_eq!(thread["messages"][0]["mode"], "plan");
    assert_eq!(last(&thread)["mode"], "plan");
    let argv = env.argv();
    assert!(flag_value(&argv, "--append-system-prompt").contains("plan mode"));
    let tools = flag_value(&argv, "--allowedTools");
    assert!(tools.contains("task_list") && !tools.contains("task_create"));
    assert!(!tools.contains("orchestrator_reply"));

    // The next message runs in brainstorm, resuming the same harness session.
    let thread = env.say("an idea", Some("brainstorm"));
    assert_eq!(thread["mode"], "brainstorm");
    let argv = env.argv();
    assert!(flag_value(&argv, "--append-system-prompt").contains("brainstorm mode"));
    assert_eq!(flag_value(&argv, "--resume"), "sess-chat");
    assert!(!flag_value(&argv, "--allowedTools").contains("task_create"));

    // Without a mode the turn is the plain chat, and the thread remembers it.
    let thread = env.say("hello", None);
    assert_eq!(thread["mode"], serde_json::Value::Null);
    assert!(last(&thread).get("mode").is_none());
    let argv = env.argv();
    assert!(flag_value(&argv, "--append-system-prompt").contains("task orchestrator"));
    assert!(flag_value(&argv, "--allowedTools").contains("task_create"));

    let error = env.daemon.request_error(
        "chat.send",
        json!({"repo": env.path(), "text": "x", "mode": "nope"}),
    );
    assert!(error.contains("mode must be"), "{error}");
}

#[test]
fn a_task_list_becomes_a_proposal_and_a_malformed_one_is_dropped() {
    let env = spawn();
    let thread = env.say("zzplan", Some("plan"));
    let reply = last(&thread);
    assert_eq!(reply["text"], "Plan below.");
    let rows = reply["proposal"]["tasks"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["title"], "First");
    assert_eq!(rows[0]["criteria"], json!(["c1"]));
    assert_eq!(rows[0]["tier"], "hard");
    assert_eq!(rows[1]["dependsOn"], json!([1]));
    // A quoted number reads as an index too.
    assert_eq!(rows[2]["dependsOn"], json!([2]));

    let thread = env.say("zzbad", Some("plan"));
    let reply = last(&thread);
    assert!(reply.get("proposal").is_none());
    assert!(reply["text"].as_str().unwrap().starts_with("Oops."));
}

#[test]
fn creating_a_proposal_maps_index_dependencies_and_never_creates_a_row_twice() {
    let env = spawn();
    let thread = env.say("zzplan", Some("plan"));
    let message = last(&thread)["id"].as_str().unwrap().to_string();

    // A row whose dependency is neither created nor chosen is refused up front.
    let error = env.daemon.request_error(
        "chat.createProposal",
        json!({"repo": env.path(), "messageId": message, "indices": [2], "backlog": true}),
    );
    assert!(error.contains("depends on"), "{error}");
    assert!(env.tasks().is_empty());

    let done = env.daemon.request(
        "chat.createProposal",
        json!({"repo": env.path(), "messageId": message, "indices": [2, 1], "backlog": true}),
    );
    // Rows created together are the parts of one parent: the parent waits in
    // the next backlog, its parts branch from its branch.
    let created = done["created"].as_array().unwrap();
    assert_eq!(created.len(), 3);
    assert_eq!(created[0]["feature"], true);
    assert_eq!(created[1]["index"], 1);
    assert_eq!(created[2]["index"], 2);
    let feature = created[0]["taskId"].as_str().unwrap();
    let first = created[1]["taskId"].as_str().unwrap();
    let second = created[2]["taskId"].as_str().unwrap();
    let tasks = env.tasks();
    assert_eq!(tasks.len(), 3);
    let by_id = |id: &str| tasks.iter().find(|t| t["id"] == id).unwrap();
    assert_eq!(by_id(feature)["backlog"]["bucket"], "next");
    assert!(by_id(feature).get("parent").is_none());
    assert_eq!(by_id(second)["dependsOn"], json!([first]));
    assert_eq!(by_id(first)["title"], "First");
    assert_eq!(by_id(first)["criteria"], json!(["c1"]));
    assert_eq!(by_id(first)["source"], "chat");
    for part in [first, second] {
        assert_eq!(by_id(part)["parent"], feature);
        assert_eq!(by_id(part)["baseRef"], by_id(feature)["branch"]);
        assert!(by_id(part).get("backlog").is_none());
    }

    // Row 3 was left out: recorded as skipped. The state survives a reload.
    let thread = env.daemon.request("chat.get", json!({"repo": env.path()}));
    let rows = last(&thread)["proposal"]["tasks"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(rows[0]["taskId"], first);
    assert_eq!(rows[1]["taskId"], second);
    assert_eq!(rows[2]["skipped"], true);
    assert!(rows[2].get("taskId").is_none());

    // The same rows again create nothing.
    let again = env.daemon.request(
        "chat.createProposal",
        json!({"repo": env.path(), "messageId": message, "indices": [1, 2], "backlog": true}),
    );
    assert_eq!(again["created"], json!([]));
    assert_eq!(env.tasks().len(), 3);

    // The skipped row can still be created later; its dependency is the task
    // created earlier.
    let late = env.daemon.request(
        "chat.createProposal",
        json!({"repo": env.path(), "messageId": message, "indices": [3], "skip": [], "backlog": true}),
    );
    let third = late["created"][0]["taskId"].as_str().unwrap().to_string();
    assert_eq!(late["proposal"]["tasks"][2]["taskId"], third);
    assert!(late["proposal"]["tasks"][2].get("skipped").is_none());
    let task = env.daemon.request("task.get", json!({"id": third}));
    assert_eq!(task["dependsOn"], json!([second]));
    // A goal-less row falls back to its title; a row created later joins
    // the same feature.
    assert_eq!(task["goal"], "Third");
    assert_eq!(task["parent"], feature);
    assert_eq!(late["proposal"]["featureTaskId"], feature);
}

#[test]
fn a_message_without_a_proposal_or_an_unknown_one_is_refused() {
    let env = spawn();
    let thread = env.say("hello", None);
    let plain = last(&thread)["id"].as_str().unwrap().to_string();
    for id in [plain.as_str(), "nope"] {
        let error = env.daemon.request_error(
            "chat.createProposal",
            json!({"repo": env.path(), "messageId": id, "indices": [1]}),
        );
        assert!(error.contains("no such chat message") || error.contains("proposes no tasks"));
    }
}

#[test]
fn a_prompts_yaml_in_the_data_dir_overrides_a_mode_prompt_on_the_next_turn() {
    let env = spawn();
    env.say("zzplan please", Some("plan"));
    assert!(flag_value(&env.argv(), "--append-system-prompt").contains("plan mode"));

    std::fs::write(
        env.daemon.data_dir.path().join("prompts.yaml"),
        "# owner override\nplan: |\n  You plan like a pirate.\n",
    )
    .unwrap();
    env.say("zzplan again", Some("plan"));
    assert_eq!(
        flag_value(&env.argv(), "--append-system-prompt"),
        "You plan like a pirate."
    );
    // Keys it leaves out keep their defaults.
    env.say("hello", None);
    assert!(flag_value(&env.argv(), "--append-system-prompt").contains("task orchestrator"));
}
