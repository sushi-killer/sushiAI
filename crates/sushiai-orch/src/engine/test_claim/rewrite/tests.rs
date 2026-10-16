use super::*;
use crate::engine::persistence::contract::tests::{assert_owner_applies, queue_owner};
use crate::engine::test_support::{task_with_status, test_app};
use crate::harness::cancel::CancelToken;
use crate::task::TaskStatus;

fn proposal(task: &Task) -> TestRewrite {
    TestRewrite {
        input: ContractInput::capture(task),
        files: vec![PlannedTest {
            path: "protected.rs".into(),
            content: "new test".into(),
        }],
        decision: "planner rewrite".into(),
    }
}

#[test]
fn accepted_owner_edit_prevents_the_real_protected_file_write() {
    let (app, dir) = test_app();
    let task = task_with_status(TaskStatus::Running);
    let path = dir.path().join("protected.rs");
    std::fs::write(&path, "original test").unwrap();
    app.store.save_task(&task).unwrap();
    let proposal = proposal(&task);
    queue_owner(&app, &task);
    assert!(matches!(
        proposal.publish(&app, &task, &CancelToken::new(), dir.path()),
        Err(RewriteError::Deferred(WaitEnd::Changed))
    ));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "original test");
    assert!(app
        .store
        .load_task(&task.id)
        .unwrap()
        .unwrap()
        .planner_tests
        .files
        .is_empty());
    assert_owner_applies(&app, &task);
}

#[test]
fn admitted_rewrite_publishes_the_real_file_and_its_protection_receipt() {
    let (app, dir) = test_app();
    let task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let result = proposal(&task).publish(&app, &task, &CancelToken::new(), dir.path());
    let receipt = match result {
        Ok(receipt) => receipt,
        Err(_) => panic!("rewrite refused"),
    };
    assert_eq!(
        std::fs::read_to_string(dir.path().join("protected.rs")).unwrap(),
        "new test"
    );
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.planner_tests.files, receipt.files);
    assert_eq!(saved.planner_tests.protected, Some(receipt.hashes));
    assert!(saved.decisions.contains(&"planner rewrite".into()));
}

#[test]
fn a_stop_before_admission_keeps_the_real_protected_file_unchanged() {
    let (app, dir) = test_app();
    let task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    std::fs::write(dir.path().join("protected.rs"), "original test").unwrap();
    let proposal = proposal(&task);
    app.update_task(&task.id, |fresh| {
        fresh.fixture_data().status = TaskStatus::Stopped
    })
    .unwrap();
    assert!(matches!(
        proposal.publish(&app, &task, &CancelToken::new(), dir.path()),
        Err(RewriteError::Deferred(WaitEnd::Stopped))
    ));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("protected.rs")).unwrap(),
        "original test"
    );
}
