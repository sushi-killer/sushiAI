use super::*;
use crate::engine::questions::parking::WaitEnd;
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::TaskStatus;

#[tokio::test]
async fn a_completed_preflight_charges_once_when_cancel_is_already_ready() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let cancel = CancelToken::new();
    let (_tx, rx) = tokio::sync::watch::channel(Some(Arc::new(BasePreflight {
        settled: true,
        cost: 0.4,
        rulings: Vec::new(),
        decisions: vec!["completed before cancellation".into()],
    })));
    app.base_preflights.lock().unwrap().insert(
        task.id.clone(),
        Arc::new(PreflightJob {
            result: rx,
            applied: std::sync::atomic::AtomicBool::new(false),
            snapshot: crate::task::checks::snapshot::CheckSnapshot::capture(&task),
            cancel: cancel.clone(),
        }),
    );
    cancel.cancel();
    assert_eq!(
        settle_base_preflight(&app, &mut task, &cancel).await,
        Err(WaitEnd::Stopped)
    );
    assert_eq!(
        settle_base_preflight(&app, &mut task, &cancel).await,
        Ok(())
    );
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.cost_usd, 0.4);
    assert_eq!(task.cost_usd, 0.4);
    assert!(!saved.brief_check.feasibility_done);
    assert_eq!(
        saved
            .decisions
            .iter()
            .filter(|line| line.as_str() == "completed before cancellation")
            .count(),
        1
    );
}
