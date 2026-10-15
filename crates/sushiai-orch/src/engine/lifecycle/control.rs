use crate::clock::now_ms;
use crate::engine::answer_policy::check_authority;
use crate::engine::state::App;

/// Removing a loop and publishing its last accepted edit share the RPC lock.
/// Baselines remain unset until the next check round when the loop is waiting.
pub(in crate::engine) fn retire(app: &App, id: &str) {
    let lock = app.task_lock(id);
    let _guard = lock.lock().unwrap();
    let mut controls = app.controls.lock().unwrap();
    if let Some(control) = controls.get(id) {
        let mut pending = control.signals.pending_amend.lock().unwrap();
        if let Some(edit) = pending.as_ref() {
            let result = (|| {
                let mut task = app
                    .store
                    .load_task(id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "task not found".to_string())?;
                let authority = check_authority(&app.settings.read().unwrap(), &task);
                match edit.preview(&task, &authority, now_ms()) {
                    Ok(amended) => task = amended,
                    Err(why) => task.record_decision(format!("Amendment not applied: {why}")),
                }
                app.store.save_task(&task).map_err(|e| e.to_string())?;
                if edit.verify.is_some() {
                    app.verify_cache.lock().unwrap().remove(id);
                }
                app.broadcast_task(&task);
                Ok::<(), String>(())
            })();
            if let Err(error) = result {
                eprintln!("orch: settling pending amendment for {id} failed: {error}");
            }
            pending.take();
        }
    }
    controls.remove(id);
}
