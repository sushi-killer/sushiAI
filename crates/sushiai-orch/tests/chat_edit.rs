//! `chat.edit`: replace an own message, drop what followed and run again on a
//! fresh harness session that gets the kept history.

mod common;
use common::*;
use serde_json::json;
use std::time::{Duration, Instant};

/// Answers by prompt, numbers its harness sessions and logs each call's argv
/// and prompt.
const FAKE: &str = r##"#!/bin/sh
p=$(cat)
n=$(cat "$COUNT_FILE" 2>/dev/null || echo 0)
n=$((n+1))
echo $n > "$COUNT_FILE"
printf '%s\n' "$@" > "$LOG_DIR/argv-$n"
printf '%s' "$p" > "$LOG_DIR/prompt-$n"
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-'$n'"}'
case "$p" in
*zzslow*) sleep 3 ;;
esac
case "$p" in
*zzplan*)
  r='Plan.\n```sushi-draft\n{\"tasks\":[{\"title\":\"First\",\"goal\":\"g1\"}]}\n```' ;;
*)
  r='Reply '$n ;;
esac
printf '%s\n' '{"type":"result","session_id":"sess-'$n'","result":"'"$r"'"}'
"##;

struct Env {
    daemon: Daemon,
    scripts: tempfile::TempDir,
    repo: tempfile::TempDir,
}

fn spawn() -> Env {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude-edit.sh", FAKE);
    let count = scripts.path().join("count");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("COUNT_FILE", count.to_str().unwrap()),
        ("LOG_DIR", scripts.path().to_str().unwrap()),
    ]);
    // The connected tools are on by default and probing an http one runs the
    // fake harness too; these tests count the chat turns' own calls.
    let mut settings = daemon.request("settings.get", json!({}));
    settings["chatTools"] = json!([]);
    daemon.request("settings.set", json!({"settings": settings}));
    Env {
        daemon,
        scripts,
        repo: init_git_repo(),
    }
}

impl Env {
    fn path(&self) -> String {
        self.repo.path().to_str().unwrap().to_string()
    }

    fn thread(&self) -> serde_json::Value {
        self.daemon
            .request("chat.get", json!({"repo": self.path()}))
    }

    fn settle(&self, messages: usize) -> serde_json::Value {
        let start = Instant::now();
        loop {
            let thread = self.thread();
            if thread["busy"] == json!(false)
                && thread["messages"].as_array().unwrap().len() == messages
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

    fn say(&self, text: &str, messages: usize) -> serde_json::Value {
        self.daemon
            .request("chat.send", json!({"repo": self.path(), "text": text}));
        self.settle(messages)
    }

    fn edit(&self, id: &str, text: &str) -> Result<serde_json::Value, String> {
        let params = json!({"repo": self.path(), "messageId": id, "text": text});
        Ok(self.daemon.request("chat.edit", params))
    }

    fn prompt(&self, n: usize) -> String {
        std::fs::read_to_string(self.scripts.path().join(format!("prompt-{n}"))).unwrap()
    }

    fn argv(&self, n: usize) -> Vec<String> {
        std::fs::read_to_string(self.scripts.path().join(format!("argv-{n}")))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

#[test]
fn an_edit_replaces_the_message_drops_the_rest_and_starts_a_fresh_session_with_the_kept_history() {
    let env = spawn();
    env.say("first question", 2);
    let thread = env.say("second question", 4);
    let second = thread["messages"][2]["id"].as_str().unwrap().to_string();
    env.say("third question", 6);

    env.edit(&second, "second, corrected").unwrap();
    let thread = env.settle(4);
    let messages = thread["messages"].as_array().unwrap();
    assert_eq!(messages[2]["id"], json!(second));
    assert_eq!(messages[2]["text"], "second, corrected");
    assert_eq!(messages[3]["text"], "Reply 4");
    assert!(!messages
        .iter()
        .any(|m| m["text"].as_str().unwrap().contains("third")));

    // The edited turn did not resume the old session and was given the history.
    assert!(!env.argv(4).contains(&"--resume".to_string()));
    let prompt = env.prompt(4);
    assert!(prompt.contains("Owner: first question"), "{prompt}");
    assert!(prompt.contains("Orchestrator: Reply 1"), "{prompt}");
    assert!(prompt.ends_with("second, corrected"), "{prompt}");
    assert!(!prompt.contains("third question"), "{prompt}");
    assert!(!prompt.contains("Reply 2"), "{prompt}");

    // The next turn resumes the new session, not the old one.
    env.say("fourth", 6);
    let argv = env.argv(5);
    let at = argv.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(argv[at + 1], "sess-4");
}

#[test]
fn editing_the_first_message_sends_no_history_and_can_change_the_mode() {
    let env = spawn();
    let thread = env.say("hello", 2);
    let first = thread["messages"][0]["id"].as_str().unwrap().to_string();
    env.daemon.request(
        "chat.edit",
        json!({"repo": env.path(), "messageId": first, "text": "zzplan it", "mode": "plan"}),
    );
    let thread = env.settle(2);
    assert_eq!(thread["messages"][0]["mode"], "plan");
    assert_eq!(thread["title"], "zzplan it");
    assert_eq!(env.prompt(2), "zzplan it");
    assert_eq!(
        thread["messages"][1]["proposal"]["tasks"][0]["title"],
        "First"
    );

    // Editing again drops that proposal with its reply.
    env.edit(&first, "plain now").unwrap();
    let thread = env.settle(2);
    assert_eq!(thread["messages"][0]["mode"], "plan");
    assert!(thread["messages"][1].get("proposal").is_none());
}

#[test]
fn an_edit_is_refused_while_answering_and_for_messages_that_are_not_the_owners() {
    let env = spawn();
    let thread = env.say("hello", 2);
    let first = thread["messages"][0]["id"].as_str().unwrap().to_string();
    let reply = thread["messages"][1]["id"].as_str().unwrap().to_string();
    let error = env.daemon.request_error(
        "chat.edit",
        json!({"repo": env.path(), "messageId": reply, "text": "x"}),
    );
    assert!(error.contains("not one of the owner's"), "{error}");
    let error = env.daemon.request_error(
        "chat.edit",
        json!({"repo": env.path(), "messageId": "nope", "text": "x"}),
    );
    assert!(error.contains("not one of the owner's"), "{error}");
    // The refused edits did not leave the turn slot held.
    env.say("zzslow", 4);

    env.daemon.request(
        "chat.send",
        json!({"repo": env.path(), "text": "zzslow again"}),
    );
    let error = env.daemon.request_error(
        "chat.edit",
        json!({"repo": env.path(), "messageId": first, "text": "x"}),
    );
    assert!(error.contains("still answering"), "{error}");
    env.settle(6);
}
