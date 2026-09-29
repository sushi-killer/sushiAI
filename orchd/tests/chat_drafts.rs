//! Structured task drafts and questions in the orchestrator chat, through a
//! real daemon whose chat harness is a fake that answers by prompt text.

mod common;
use common::*;
use serde_json::json;
use std::time::{Duration, Instant};

const FAKE: &str = r##"#!/bin/sh
p=$(cat)
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-chat"}'
case "$p" in
*zzdraft*)
  r='Here you go.\n```sushi-draft\n{\"title\":\"Ship it\",\"goal\":\"g\",\"criteria\":[{\"text\":\"c\"}],\"dependsOn\":[\"t1\"],\"tier\":\"weird\",\"questions\":[{\"text\":\"Which?\",\"options\":[\"A\",\"B\"]},{\"text\":\"\"}]}\n```\nBye.' ;;
*zzbad*)
  r='Oops.\n```sushi-draft\n{not json\n```' ;;
*zzask*)
  r='Pick one.\n```sushi-draft\n{\"questions\":[{\"text\":\"A or B?\",\"options\":[\"A\",\"B\"]}]}\n```' ;;
*)
  r='Plain reply.' ;;
esac
printf '%s\n' '{"type":"result","session_id":"sess-chat","result":"'"$r"'"}'
"##;

fn spawn() -> (Daemon, tempfile::TempDir, tempfile::TempDir, String) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude-chat.sh", FAKE);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let repo = tempfile::tempdir().unwrap();
    let path = repo.path().to_str().unwrap().to_string();
    (daemon, scripts, repo, path)
}

fn say(daemon: &Daemon, repo: &str, text: &str) -> serde_json::Value {
    daemon.request("chat.send", json!({"repo": repo, "text": text}));
    let start = Instant::now();
    loop {
        let thread = daemon.request("chat.get", json!({"repo": repo}));
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

fn last(thread: &serde_json::Value) -> &serde_json::Value {
    thread["messages"].as_array().unwrap().last().unwrap()
}

#[test]
fn a_draft_block_is_stripped_and_lands_on_the_message_and_the_thread() {
    let (daemon, _s, _r, repo) = spawn();
    let thread = say(&daemon, &repo, "zzdraft please");
    let reply = last(&thread);
    assert_eq!(reply["role"], "assistant");
    assert_eq!(reply["text"], "Here you go.\n\nBye.");
    assert_eq!(reply["draft"]["title"], "Ship it");
    assert_eq!(reply["draft"]["goal"], "g");
    assert_eq!(reply["draft"]["criteria"], json!(["c"]));
    assert_eq!(reply["draft"]["dependsOn"], json!(["t1"]));
    assert!(reply["draft"].get("tier").is_none() || reply["draft"]["tier"].is_null());
    assert_eq!(
        reply["questions"],
        json!([{"text": "Which?", "options": ["A", "B"]}])
    );
    assert_eq!(thread["draft"], reply["draft"]);
    // The owner's own message carries neither.
    let first = &thread["messages"][0];
    assert!(first.get("draft").is_none() && first.get("questions").is_none());
}

#[test]
fn a_malformed_block_and_a_questions_only_block_leave_the_thread_draft_alone() {
    let (daemon, _s, _r, repo) = spawn();
    let drafted = say(&daemon, &repo, "zzdraft");
    let thread = say(&daemon, &repo, "zzbad");
    assert_eq!(
        last(&thread)["text"],
        "Oops.\n```sushi-draft\n{not json\n```"
    );
    assert!(last(&thread).get("draft").is_none());
    assert_eq!(thread["draft"], drafted["draft"]);

    let thread = say(&daemon, &repo, "zzask");
    let reply = last(&thread);
    assert_eq!(reply["text"], "Pick one.");
    assert!(reply.get("draft").is_none());
    assert_eq!(reply["questions"][0]["text"], "A or B?");
    assert_eq!(thread["draft"], drafted["draft"]);

    let plain = say(&daemon, &repo, "hello");
    assert_eq!(last(&plain)["text"], "Plain reply.");
    assert!(last(&plain).get("questions").is_none());
}

#[test]
fn clear_draft_keeps_messages_and_clear_removes_the_draft() {
    let (daemon, _s, _r, repo) = spawn();
    say(&daemon, &repo, "zzdraft");
    let thread = daemon.request("chat.clearDraft", json!({"repo": repo}));
    assert!(thread.get("draft").is_none());
    assert_eq!(thread["messages"].as_array().unwrap().len(), 2);
    let again = daemon.request("chat.get", json!({"repo": repo}));
    assert!(again.get("draft").is_none());
    assert_eq!(last(&again)["draft"]["title"], "Ship it");

    let thread = say(&daemon, &repo, "zzdraft");
    assert!(thread["draft"]["title"].is_string());
    let cleared = daemon.request("chat.clear", json!({"repo": repo}));
    assert!(cleared.get("draft").is_none());
    assert_eq!(cleared["messages"], json!([]));
}

#[test]
fn a_chats_file_from_before_drafts_still_loads() {
    let (daemon, _s, _r, repo) = spawn();
    // The first read writes the repo's store; its file name is a hash of the
    // repo, so it is the only *.json under chats/.
    daemon.request("chat.get", json!({"repo": repo}));
    let dir = daemon.data_dir().join("chats");
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    assert_eq!(files.len(), 1);
    let store = json!({
        "repo": repo,
        "current": "s1",
        "sessions": [{
            "id": "s1",
            "title": "Old",
            "messages": [
                {"id": "a", "role": "user", "text": "Old", "ts": 1},
                {"id": "b", "role": "assistant", "text": "Answer", "ts": 2}
            ]
        }]
    });
    std::fs::write(&files[0], store.to_string()).unwrap();

    let thread = daemon.request("chat.get", json!({"repo": repo}));
    assert_eq!(thread["id"], "s1");
    assert_eq!(thread["messages"].as_array().unwrap().len(), 2);
    assert!(thread.get("draft").is_none());
    assert!(last(&thread).get("draft").is_none());
    assert!(last(&thread).get("questions").is_none());
    let list = daemon.request("chat.list", json!({"repo": repo}));
    assert_eq!(list["sessions"][0]["title"], "Old");
}
