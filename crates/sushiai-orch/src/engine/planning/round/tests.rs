use super::*;
use crate::engine::run_control::TaskSignals;
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::amendment::Amendment;
use crate::task::checks::authority::{CheckAuthority, Origin};
use crate::task::TaskStatus;

#[cfg(unix)]
#[tokio::test]
async fn failed_pending_consumption_stops_before_a_planner_attempt_is_prepared() {
    use std::os::unix::fs::PermissionsExt;
    struct Restore(std::path::PathBuf, std::fs::Permissions);
    impl Drop for Restore {
        fn drop(&mut self) {
            std::fs::set_permissions(&self.0, self.1.clone()).unwrap();
        }
    }
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Drafting);
    app.store.save_task(&task).unwrap();
    task.queue_pending_amendment(
        Amendment {
            criteria: Some(vec!["Owner's next contract".into()]),
            ..Default::default()
        },
        Origin::Owner,
        &CheckAuthority::default(),
        now_ms(),
    )
    .unwrap();
    app.store.save_task_with_pending(&task, None).unwrap();
    let receipt = task.pending_amendment.clone();
    let signals = TaskSignals::new();
    *signals.pending_amend.lock().unwrap() = receipt.clone();
    let path = app.store.task_dir(&task.id);
    let restore = Restore(
        path.clone(),
        std::fs::metadata(&path).unwrap().permissions(),
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o555)).unwrap();
    let route = app.settings.read().unwrap().routes[0].id.clone();
    let result = prepare_round(
        &app,
        &task.id,
        &(route, "fixture".into()),
        &signals,
        &mut None,
    )
    .await;
    drop(restore);
    assert!(
        matches!(result, Err(WaitEnd::Stopped)),
        "a failed consume cannot prepare a model turn"
    );
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.pending_amendment, receipt);
    assert_eq!(saved.criteria, task.criteria);
    assert!(saved.attempts.is_empty());
    assert!(!app.store.task_dir(&task.id).join("runs").exists());
}
