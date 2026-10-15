use super::*;
use crate::task::amendment::Amendment;
use crate::task::checks::authority::{CheckAuthority, Origin};
use crate::task::test_support::sample_task;
use crate::task::{AttemptStatus, TaskStatus};

fn task() -> Task {
    sample_task("pending", TaskStatus::Waiting, AttemptStatus::Running)
}

#[test]
fn serialized_pending_edits_keep_snapshot_authority_and_an_explicit_held_out_clear() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = task();
    task.fixture_data().held_out = Some(crate::task::Check {
        criterion: 0,
        run: "owner-check".into(),
        baseline: Some(crate::task::Baseline::Fail),
    });
    store.save_task(&task).unwrap();
    task.queue_pending_amendment(
        Amendment {
            criteria: Some(vec!["Owner changed it".into()]),
            held_out: Some(None),
            ..Default::default()
        },
        Origin::Owner,
        &CheckAuthority::default(),
        2,
    )
    .unwrap();
    let receipt = task.pending_amendment.clone().unwrap();
    store.save_task_with_pending(&task, None).unwrap();
    let reopened = Store::new(dir.path()).unwrap();
    let mut loaded = reopened.load_task(&task.id).unwrap().unwrap();
    assert_eq!(loaded.pending_amendment.as_ref(), Some(&receipt));
    assert_eq!(
        loaded.pending_amendment.as_ref().unwrap().held_out,
        Some(None)
    );
    assert!(matches!(
        loaded.settle_pending_amendment(&CheckAuthority::default(), 3),
        crate::task::amendment::storage::Settlement::Applied
    ));
    reopened
        .save_task_with_pending(&loaded, Some(&receipt))
        .unwrap();
    let saved = reopened.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["Owner changed it"]);
    assert!(saved.pending_amendment.is_none());
    assert!(saved.held_out.is_none());
    store.save_task(&task).unwrap();
    assert!(store
        .load_task(&task.id)
        .unwrap()
        .unwrap()
        .pending_amendment
        .is_none());
}

#[test]
fn a_racing_stale_save_cannot_erase_an_acknowledged_pending_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let old = task();
    store.save_task(&old).unwrap();
    let mut queued = old.clone();
    queued
        .queue_pending_amendment(
            Amendment {
                verify: Some(vec!["new-command".into()]),
                ..Default::default()
            },
            Origin::Owner,
            &CheckAuthority::default(),
            2,
        )
        .unwrap();
    let receipt = queued.pending_amendment.clone();
    let stale_store = store.clone();
    let (sent, received) = std::sync::mpsc::channel();
    let racing = std::thread::spawn(move || {
        stale_store.save_task(&old).unwrap();
        received.recv().unwrap();
        for _ in 0..64 {
            stale_store.save_task(&old).unwrap();
        }
    });
    store.save_task_with_pending(&queued, None).unwrap();
    sent.send(()).unwrap();
    racing.join().unwrap();
    assert_eq!(
        store
            .load_task(&queued.id)
            .unwrap()
            .unwrap()
            .pending_amendment,
        receipt
    );
}

#[test]
fn consuming_an_older_receipt_cannot_discard_a_later_merged_edit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = task();
    store.save_task(&task).unwrap();
    task.queue_pending_amendment(
        Amendment {
            criteria: Some(vec!["first edit".into()]),
            ..Default::default()
        },
        Origin::Owner,
        &CheckAuthority::default(),
        2,
    )
    .unwrap();
    store.save_task_with_pending(&task, None).unwrap();
    let earlier = task.pending_amendment.clone().unwrap();
    let mut stale = task.clone();
    stale.settle_pending_amendment(&CheckAuthority::default(), 3);
    task.queue_pending_amendment(
        Amendment {
            verify: Some(vec!["second-command".into()]),
            ..Default::default()
        },
        Origin::Owner,
        &CheckAuthority::default(),
        3,
    )
    .unwrap();
    store.save_task_with_pending(&task, Some(&earlier)).unwrap();
    assert!(store
        .save_task_with_pending(&stale, Some(&earlier))
        .is_err());
    let saved = store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.pending_amendment, task.pending_amendment);
    assert_eq!(saved.criteria, ["works"]);
}
