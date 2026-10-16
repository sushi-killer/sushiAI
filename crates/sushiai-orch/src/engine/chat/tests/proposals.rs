use crate::api::events::Event;
use crate::engine::chat::sessions::{push, ChatStore};
use crate::engine::chat::store::{load, save, store_path};
use crate::engine::chat::types::ChatMode;
use serde_json::json;

use super::{find_proposal, handle_create_proposal, ChatProposal, ProposalDep, ProposedTask};
use crate::engine::chat::test_support::{git_repo, proposal_session, test_app};

#[tokio::test]
async fn rows_created_together_are_parts_of_one_feature() {
    for backlog in [false, true] {
        let (app, _dir, repo) = test_app();
        git_repo(&repo);
        let message = proposal_session(&app, &repo);
        let made = handle_create_proposal(
            &app,
            json!({"repo": repo, "messageId": message, "indices": [1, 2], "backlog": backlog}),
        )
        .await
        .unwrap();
        let created = made["created"].as_array().unwrap();
        let feature = created
            .iter()
            .find(|c| c["feature"] == json!(true))
            .unwrap()["taskId"]
            .as_str()
            .unwrap()
            .to_string();
        let parent = app.store.load_task(&feature).unwrap().unwrap();
        assert_eq!(parent.title, "Numbers");
        assert_eq!(
            parent.queue.backlog.map(|b| b.bucket),
            backlog.then_some(crate::task::BacklogBucket::Next)
        );
        for row in made["proposal"]["tasks"].as_array().unwrap().iter().take(2) {
            let part = app
                .store
                .load_task(row["taskId"].as_str().unwrap())
                .unwrap()
                .unwrap();
            assert_eq!(part.parent.as_deref(), Some(feature.as_str()));
            assert_eq!(part.base_ref.as_deref(), Some(parent.branch.as_str()));
            assert!(part.queue.backlog.is_none());
            if backlog {
                assert_eq!(part.status, crate::task::TaskStatus::Queued);
            }
        }
        // A row created later joins the same feature.
        let made = handle_create_proposal(
            &app,
            json!({"repo": repo, "messageId": message, "indices": [3]}),
        )
        .await
        .unwrap();
        assert_eq!(made["created"].as_array().unwrap().len(), 1);
        let late = made["created"][0]["taskId"].as_str().unwrap();
        let late = app.store.load_task(late).unwrap().unwrap();
        assert_eq!(late.parent.as_deref(), Some(feature.as_str()));
        app.shutdown();
    }
}

#[tokio::test]
async fn a_feature_whose_first_part_fails_is_removed() {
    let (app, _dir, repo) = test_app();
    git_repo(&repo);
    let mut session = load(&app, &repo).current().clone();
    push(&mut session, "user", "plan it", ChatMode::Plan);
    push(&mut session, "assistant", "here", ChatMode::Plan);
    let message = session.messages.last_mut().unwrap();
    message.proposal = Some(ChatProposal {
        tasks: vec![
            ProposedTask {
                title: "One".into(),
                depends_on: vec![ProposalDep::Id("gone".into())],
                ..Default::default()
            },
            ProposedTask {
                title: "Two".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    let id = message.id.clone();
    save(&app, &repo, &session);

    let error = handle_create_proposal(
        &app,
        json!({"repo": repo, "messageId": id, "indices": [1, 2]}),
    )
    .await
    .unwrap_err();
    assert!(error.contains("unknown dependency"), "{error}");
    assert!(app.repo_tasks(&repo).is_empty());
    let (_, proposal) = find_proposal(&load(&app, &repo), &id).unwrap();
    assert!(proposal.feature_task_id.is_none());
    app.shutdown();
}

#[tokio::test]
async fn creating_from_a_proposal_never_leaves_the_chat_busy() {
    let (app, _dir, repo) = test_app();
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    ] {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap();
    }
    let message = proposal_session(&app, &repo);
    let mut events = app.subscribe();
    let calls = [
        json!({"indices": [1], "skip": []}),
        json!({"indices": [2], "backlog": true, "skip": []}),
        json!({"indices": [3]}),
    ];
    for call in calls {
        let mut params = call;
        params["repo"] = json!(repo);
        params["messageId"] = json!(message);
        handle_create_proposal(&app, params).await.unwrap();
        let stored = load(&app, &repo);
        assert!(stored.sessions.iter().all(|s| !s.busy));
        let on_disk: ChatStore =
            serde_json::from_str(&std::fs::read_to_string(store_path(&app, &repo)).unwrap())
                .unwrap();
        assert!(on_disk.sessions.iter().all(|s| !s.busy));
    }
    let mut seen = 0;
    while let Ok(event) = events.try_recv() {
        if let Event::Chat {
            thread, sessions, ..
        } = event
        {
            seen += 1;
            assert_eq!(thread["busy"], json!(false), "{thread}");
            assert!(sessions
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["busy"] == json!(false)));
        }
    }
    assert!(seen >= 3);
}

#[test]
fn proposals_order_dependencies_first() {
    let proposal = ChatProposal::parse(
        &json!([
            {"title": "A", "dependsOn": [2]},
            {"title": "B", "dependsOn": [3]},
            {"title": "C"},
        ]),
        None,
    )
    .unwrap();
    assert_eq!(proposal.order(&[0, 1, 2]), Some(vec![2, 1, 0]));
    assert_eq!(proposal.order(&[0, 1]), Some(vec![1, 0]));
}
