use super::CheckContext;
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::test_support::{task_with_status, test_app};
use crate::procs::testing::wait_until;
use crate::task::attempt::Stage;
use crate::task::TaskStatus;

/// The background base preflight writes a check decision while the loop admits
/// the first attempt. The decision must not write back the copy it read before
/// the attempt was saved: the attempt would vanish, its result could not be
/// settled, and the loop would end with the task left queued.
#[tokio::test]
async fn a_check_decision_never_erases_an_attempt_saved_while_it_waits_for_the_task_lock() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Queued);
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            signals: TaskSignals::new(),
            handle: tokio::spawn(async {}),
        },
    );
    // The attempt start holds the task lock.
    let lock = app.task_lock(&id);
    let guard = lock.lock().unwrap();
    let writer = {
        let (app, id) = (app.clone(), id.clone());
        std::thread::spawn(move || {
            CheckContext::new(&app, Some(&id)).decision(
                "Orchestrator: true already passed on tree 853694aa; not run again".into(),
                &mut Vec::new(),
            );
        })
    };
    // The decision has read the task and is waiting for the lock.
    wait_until("the check decision to reach the task lock", || {
        app.check_decisions.lock().unwrap().contains_key(&id)
    });
    let mut started = app.store.load_task(&id).unwrap().unwrap();
    let route = app.settings.read().unwrap().routes[0].clone();
    crate::task::attempts::start(&mut started, Stage::Implement, &route, "fixture", 1);
    app.store.save_task(&started).unwrap();
    drop(guard);
    writer.join().unwrap();
    let saved = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(
        saved.attempts.len(),
        1,
        "the check decision erased the attempt"
    );
    assert!(saved
        .decisions
        .iter()
        .any(|line| line.contains("not run again")));
    let control = app.controls.lock().unwrap().remove(&id).unwrap();
    control.handle.await.unwrap();
}
