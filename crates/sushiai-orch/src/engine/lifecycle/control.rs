use crate::clock::now_ms;
use crate::engine::answer_policy::authority::check_authority;
use crate::engine::state::App;

/// Removing a loop and publishing its last accepted edit share the RPC lock.
/// Baselines remain unset until the next check round when the loop is waiting.
pub(in crate::engine) fn retire(app: &App, id: &str) {
    let lock = app.task_lock(id);
    let _guard = lock.lock().unwrap();
    let mut controls = app.controls.lock().unwrap();
    let result = (|| {
        let Some(mut task) = app.store.load_task(id).map_err(|e| e.to_string())? else {
            return Ok(());
        };
        let Some(receipt) = task.pending_amendment.clone() else {
            return Ok(());
        };
        let authority = check_authority(&app.settings.read().unwrap(), &task);
        task.settle_pending_amendment(&authority, now_ms());
        app.store
            .save_task_with_pending(&task, Some(&receipt))
            .map_err(|e| e.to_string())?;
        if let Some(control) = controls.get(id) {
            *control.signals.pending_amend.lock().unwrap() = None;
        }
        if receipt.verify.is_some() {
            app.verify_cache.lock().unwrap().remove(id);
        }
        app.broadcast_task(&task);
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        eprintln!("orch: settling pending amendment for {id} failed: {error}");
    }
    controls.remove(id);
}
