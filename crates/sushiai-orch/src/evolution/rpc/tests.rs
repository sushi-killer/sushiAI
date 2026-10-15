use super::*;
use crate::engine::evolution::test_support::*;
use crate::engine::test_support::test_app;

#[tokio::test]
async fn evolution_list_filters_by_repo_and_reject_and_adopt_change_status() {
    let (app, _dir) = test_app();
    let mut a = proposal(ProposalStatus::Proposed, "loop:a");
    a.created_at = 1;
    let mut b = proposal(ProposalStatus::Proposed, "loop:b");
    b.created_at = 2;
    b.repo = "/other".into();
    app.store.save_proposal(&a).unwrap();
    app.store.save_proposal(&b).unwrap();
    let all = app.dispatch("evolution.list", json!({})).await.unwrap();
    assert_eq!(all[0]["id"], b.id.as_str());
    assert_eq!(all[1]["id"], a.id.as_str());
    let one = app
        .dispatch("evolution.list", json!({"repo": "/r"}))
        .await
        .unwrap();
    assert_eq!(one.as_array().unwrap().len(), 1);
    let mut events = app.subscribe();
    let rejected = app
        .dispatch("evolution.reject", json!({"id": a.id, "reason": "no"}))
        .await
        .unwrap();
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["reason"], "no");
    assert!(matches!(events.try_recv(), Ok(Event::Proposal { .. })));
    assert!(app
        .dispatch("evolution.adopt", json!({"id": a.id}))
        .await
        .is_err());
    let adopted = app
        .dispatch("evolution.adopt", json!({"id": b.id}))
        .await
        .unwrap();
    assert_eq!(adopted["status"], "adopted");
    assert!(adopted["adoptedAt"].as_i64().unwrap() > 0);
    let err = app
        .dispatch("evolution.adopt", json!({"id": "../x"}))
        .await
        .unwrap_err();
    assert_eq!(err, "invalid proposal id");
}

#[tokio::test]
async fn evolution_approve_refuses_a_harness_track_proposal() {
    let (app, _dir) = test_app();
    let mut p = proposal(ProposalStatus::Proposed, "loop:a");
    p.track = Track::Harness;
    app.store.save_proposal(&p).unwrap();
    let err = app
        .dispatch("evolution.approve", json!({"id": p.id}))
        .await
        .unwrap_err();
    assert!(err.contains("repo-track"), "{err}");
}
