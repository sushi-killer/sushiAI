use crate::task::test_support::sample_task;
use crate::task::{AttemptStatus, TaskStatus};

#[test]
fn a_new_delivery_restarts_the_dirty_checkout_deadline() {
    let mut task = sample_task("new-delivery", TaskStatus::Running, AttemptStatus::Passed);
    task.begin_dirty_wait(10);

    assert!(task.begin_delivery(100));
    assert_eq!(task.status, TaskStatus::Landing);
    assert_eq!(task.updated_at, 100);
    assert_eq!(task.queue.dirty_since, None);
    assert_eq!(task.begin_dirty_wait(110), 110);
}

#[test]
fn a_delivery_retry_keeps_the_original_dirty_checkout_deadline() {
    let mut task = sample_task("delivery-retry", TaskStatus::Landing, AttemptStatus::Passed);
    task.begin_dirty_wait(10);

    assert!(task.begin_delivery(100));
    assert_eq!(task.queue.dirty_since, Some(10));
    assert_eq!(task.begin_dirty_wait(110), 10);
    assert!(task.begin_delivery(120));
    assert_eq!(task.queue.dirty_since, Some(10));
}
