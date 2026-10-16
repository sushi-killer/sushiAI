use super::*;
use crate::engine::persistence::contract::tests::{assert_owner_applies, queue_owner};
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::TaskStatus;

#[test]
fn a_refresh_cannot_invalidate_an_accepted_owner_amendment() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().verify = vec!["false".into()];
    app.store.save_task(&task).unwrap();
    let input = ContractInput::capture(&task);
    let ruling = resolve_refresh(
        &app,
        &task,
        Ok(r#"{"verify":["true"],"paths":["new"]}"#.into()),
        None,
    );
    queue_owner(&app, &task);
    assert_eq!(
        settle_refresh(&app, &task, &input, ruling, &CancelToken::new()),
        Err(WaitEnd::Changed)
    );
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.verify, ["false"]);
    assert!(!saved.brief_check.refreshed);
    assert!(saved.paths.is_empty());
    assert_owner_applies(&app, &task);
}

#[test]
fn an_admitted_refresh_applies_commands_and_its_completion_flag_together() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Queued);
    app.store.save_task(&task).unwrap();
    let input = ContractInput::capture(&task);
    let ruling = resolve_refresh(
        &app,
        &task,
        Ok(r#"{"verify":["false"],"paths":["new"]}"#.into()),
        None,
    );
    assert_eq!(
        settle_refresh(&app, &task, &input, ruling, &CancelToken::new()),
        Ok(())
    );
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.verify, ["false"]);
    assert_eq!(saved.paths, ["new"]);
    assert!(saved.brief_check.refreshed);
}
