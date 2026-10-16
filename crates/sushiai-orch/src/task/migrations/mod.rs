//! One-time decoding of task state persisted before typed loop fields.

mod base_state;
mod loop_state;

use crate::task::Task;

pub(crate) fn apply(task: &mut Task) {
    let task = &mut task.data;
    loop_state::migrate_loop_state(task);
    base_state::migrate_base_and_permission_state(task);
    loop_state::migrate_final_check_failures(task);
}
