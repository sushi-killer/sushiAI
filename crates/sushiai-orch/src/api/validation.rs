use crate::store::Store;
/// P0: every task id that arrives as a request param must be a valid UUID
/// (the only shape `task.create` ever generates) *and* resolve to a path
/// that actually stays under `<data>/tasks` -- defense in depth, since a
/// non-UUID id can never produce a path outside that directory anyway, but
/// this makes the invariant an assertion rather than an assumption.
pub(crate) fn validate_task_id(store: &Store, id: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(id).map_err(|_| "invalid task id".to_string())?;
    let dir = store.task_dir(id);
    let tasks_root = store.data_dir().join("tasks");
    if !dir.starts_with(&tasks_root) {
        return Err("invalid task id".to_string());
    }
    Ok(())
}
