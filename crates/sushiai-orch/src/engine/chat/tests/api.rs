use crate::engine::chat::sessions::{parse_store, push, ChatStore};
use crate::engine::chat::store::{begin_turn, finish_turn, load, store_path, BUSY};
use crate::engine::chat::types::ChatMode;
use crate::harness::cancel::CancelToken;
use serde_json::json;

use super::{handle_clear, handle_get, handle_list, handle_new, handle_switch};
use crate::engine::chat::test_support::{converse, test_app};

#[tokio::test]
async fn a_new_repo_starts_with_one_empty_current_session() {
    let (app, _dir, repo) = test_app();
    let list = handle_list(&app, json!({"repo": repo})).await.unwrap();
    let thread = handle_get(&app, json!({"repo": repo})).await.unwrap();
    assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(list["current"], thread["id"]);
    assert_eq!(thread["messages"], json!([]));
    assert!(thread.get("sessionId").is_none());
    assert_eq!(thread["repo"], json!(repo));
}

#[tokio::test]
async fn a_new_session_is_empty_current_and_titled_by_its_first_owner_message() {
    let (app, _dir, repo) = test_app();
    converse(&app, &repo, "Fix the login page", "harness-1");
    let first = load(&app, &repo).current.clone();

    let created = handle_new(&app, json!({"repo": repo})).await.unwrap();
    assert_ne!(created["id"], json!(first));
    assert_eq!(created["messages"], json!([]));
    assert!(created.get("title").is_none());
    assert!(created.get("sessionId").is_none());

    let long = "x".repeat(80);
    converse(&app, &repo, &long, "harness-2");
    let list = handle_list(&app, json!({"repo": repo})).await.unwrap();
    assert_eq!(list["current"], created["id"]);
    let titles: Vec<_> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["title"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(titles[0], "Fix the login page");
    assert_eq!(titles[1], format!("{}\u{2026}", "x".repeat(59)));
    assert!(list["sessions"][0].get("messages").is_none());
}

#[tokio::test]
async fn switching_brings_back_that_sessions_own_messages_and_harness_session() {
    let (app, _dir, repo) = test_app();
    converse(&app, &repo, "first chat", "harness-1");
    let first = load(&app, &repo).current.clone();
    handle_new(&app, json!({"repo": repo})).await.unwrap();
    converse(&app, &repo, "second chat", "harness-2");

    let back = handle_switch(&app, json!({"repo": repo, "id": first}))
        .await
        .unwrap();
    assert_eq!(back["id"], json!(first));
    assert_eq!(back["messages"][0]["text"], "first chat");
    assert_eq!(back["sessionId"], "harness-1");
    assert_eq!(load(&app, &repo).current, first);

    let unknown = handle_switch(&app, json!({"repo": repo, "id": "nope"})).await;
    assert!(unknown.is_err());
    assert_eq!(load(&app, &repo).current, first);
}

#[tokio::test]
async fn clearing_empties_the_current_session_and_forgets_its_harness_session() {
    let (app, _dir, repo) = test_app();
    converse(&app, &repo, "keep me", "harness-1");
    let kept = load(&app, &repo).current.clone();
    handle_new(&app, json!({"repo": repo})).await.unwrap();
    converse(&app, &repo, "clear me", "harness-2");

    let cleared = handle_clear(&app, json!({"repo": repo})).await.unwrap();
    assert_eq!(cleared["messages"], json!([]));
    assert!(cleared.get("sessionId").is_none());
    assert!(cleared.get("title").is_none());
    let chats = load(&app, &repo);
    assert_eq!(chats.summaries().len(), 2);
    let other = chats.sessions.iter().find(|s| s.id == kept).unwrap();
    assert_eq!(other.messages.len(), 2);
    assert_eq!(other.session_id.as_deref(), Some("harness-1"));
}

#[tokio::test]
async fn new_switch_and_clear_are_refused_while_a_turn_is_running() {
    let (app, _dir, repo) = test_app();
    converse(&app, &repo, "first chat", "harness-1");
    let first = load(&app, &repo).current.clone();
    handle_new(&app, json!({"repo": repo})).await.unwrap();
    converse(&app, &repo, "second chat", "harness-2");
    let second = load(&app, &repo).current.clone();

    app.chat_turns
        .lock()
        .unwrap()
        .insert(repo.clone(), CancelToken::new());
    let list = handle_list(&app, json!({"repo": repo})).await.unwrap();
    let busy: Vec<_> = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["busy"].as_bool().unwrap())
        .collect();
    assert_eq!(busy, vec![false, true]);
    for result in [
        handle_new(&app, json!({"repo": repo})).await,
        handle_switch(&app, json!({"repo": repo, "id": first})).await,
        handle_clear(&app, json!({"repo": repo})).await,
    ] {
        assert_eq!(result.unwrap_err(), BUSY);
    }
    let chats = load(&app, &repo);
    assert_eq!(chats.summaries().len(), 2);
    assert_eq!(chats.current, second);
    assert_eq!(chats.current().messages.len(), 2);

    // The turn's final save frees the slot in the same step, so the reply
    // lands in the session it belongs to and the same requests go through.
    let mut reply = chats.current().clone();
    push(&mut reply, "assistant", "done", ChatMode::Chat);
    finish_turn(&app, &repo, &reply);
    assert!(!app.chat_turns.lock().unwrap().contains_key(&repo));
    let chats = load(&app, &repo);
    assert_eq!(chats.current().messages.len(), 3);
    assert!(!chats.current().busy);
    handle_switch(&app, json!({"repo": repo, "id": first}))
        .await
        .unwrap();
    let cleared = handle_clear(&app, json!({"repo": repo})).await.unwrap();
    assert_eq!(cleared["id"], json!(first));
    assert_eq!(cleared["messages"], json!([]));
}

#[tokio::test]
async fn a_legacy_single_thread_file_becomes_a_one_session_store() {
    let (app, _dir, repo) = test_app();
    let legacy = json!({
        "repo": repo,
        "messages": [
            {"id": "a", "role": "user", "text": "Old question", "ts": 1},
            {"id": "b", "role": "assistant", "text": "Old answer", "ts": 2},
            {"id": "c", "role": "user", "text": "Follow-up", "ts": 3},
        ],
        "busy": true,
        "sessionId": "legacy-harness",
        "routeId": "claude-opus",
        "taskMcp": {"servers": {}},
    });
    let path = store_path(&app, &repo);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, legacy.to_string()).unwrap();

    let thread = handle_get(&app, json!({"repo": repo})).await.unwrap();
    let list = handle_list(&app, json!({"repo": repo})).await.unwrap();
    assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(list["current"], thread["id"]);
    assert_eq!(list["sessions"][0]["title"], "Old question");
    let texts: Vec<_> = thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, vec!["Old question", "Old answer", "Follow-up"]);
    assert_eq!(thread["sessionId"], "legacy-harness");
    assert_eq!(thread["routeId"], "claude-opus");
    assert_eq!(thread["taskMcp"], json!({"servers": {}}));
    assert_eq!(thread["busy"], false);

    // The migrated shape is on disk, so the ids hold across reads.
    let on_disk: ChatStore =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(json!(on_disk.current), thread["id"]);
    assert_eq!(on_disk.sessions[0].messages.len(), 3);
}

#[tokio::test]
async fn a_turn_in_a_new_session_does_not_resume_the_previous_harness_session() {
    let (app, _dir, repo) = test_app();
    converse(&app, &repo, "first chat", "harness-1");
    let created = handle_new(&app, json!({"repo": repo})).await.unwrap();
    let (session, route, _cancel) = begin_turn(&app, &repo).unwrap();
    assert_eq!(json!(session.id), created["id"]);
    assert_eq!(session.session_id, None);
    assert_eq!(session.route_id.as_deref(), Some(route.id.as_str()));
    assert_eq!(begin_turn(&app, &repo).err(), Some(BUSY.to_string()));
}

#[test]
fn a_store_pointing_at_a_missing_session_falls_back_to_the_last_one() {
    let text = json!({"repo": "/r", "current": "gone", "sessions": [{"id": "a"}, {"id": "b"}]})
        .to_string();
    let (chats, changed) = parse_store("/r", Some(&text));
    assert!(changed);
    assert_eq!(chats.current, "b");
    let (fresh, changed) = parse_store("/r", Some(r#"{"repo":"/r","current":"x","sessions":[]}"#));
    assert!(changed);
    assert_eq!(fresh.sessions.len(), 1);
    assert_eq!(fresh.current, fresh.sessions[0].id);
}

#[test]
fn a_store_from_before_modes_loads_its_brainstorm_sessions_as_chat_sessions() {
    let old = json!({
        "repo": "/r",
        "current": "a",
        "brainstormCurrent": "b",
        "sessions": [
            {"id": "a", "kind": "chat", "messages": []},
            {"id": "b", "kind": "brainstorm", "draft": {"title": "T"}, "messages": [
                {"id": "m", "role": "user", "text": "idea", "ts": 5},
                {"id": "n", "role": "assistant", "text": "which?", "ts": 6,
                 "draft": {"title": "T"}, "questions": [{"text": "Q?", "options": ["x"]}]},
            ]},
        ],
    })
    .to_string();
    let (chats, _) = parse_store("/r", Some(&old));
    assert_eq!(chats.current, "a");
    assert_eq!(chats.sessions.len(), 2);
    let brainstorm = &chats.sessions[1];
    assert_eq!(brainstorm.messages.len(), 2);
    assert_eq!(brainstorm.messages[1].questions.len(), 1);
    assert_eq!(brainstorm.mode, ChatMode::Chat);
    let thread = chats.thread_json();
    assert!(thread.get("kind").is_none());
}
