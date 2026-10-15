use crate::engine::persistence::persist_attempt_observation;
use crate::engine::state::App;
use std::sync::Arc;

/// Stamps `first_edit_at` now when the streamed `line` is the first edit of
/// an implement attempt; `true` once it has.
pub(crate) async fn note_first_edit(
    app: &Arc<App>,
    task_id: &str,
    attempt_n: u32,
    line: &str,
) -> bool {
    let is_edit = serde_json::from_str(line)
        .ok()
        .is_some_and(|v| crate::ab::is_edit_event(&v));
    if is_edit {
        persist_attempt_observation(
            app,
            task_id,
            attempt_n,
            crate::task::attempts::RunObservation::FirstEdit,
        )
        .await;
    }
    is_edit
}
