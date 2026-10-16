use crate::engine::lifecycle::execution::ExecutedAttempt;
use crate::engine::state::App;
use crate::task::{attempts, Task};
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

pub(super) fn finish(
    app: &App,
    task: &mut Task,
    executed: &mut ExecutedAttempt,
) -> BTreeSet<String> {
    let (blocks, handled, lines) = if let Some((token, context)) = executed.registered.take() {
        app.forget_hook(&token, &executed.run.run_dir);
        context.cancel.cancel();
        let applied = crate::engine::permissions::staging::finish_staging(&context);
        let handled = context.handled.lock().unwrap().clone();
        (
            context.blocks.load(Ordering::SeqCst),
            handled,
            crate::engine::permissions::staging::staging_lines(&applied),
        )
    } else {
        (0, BTreeSet::new(), Vec::new())
    };
    let _ = std::fs::remove_file(&executed.key_path);
    let _ = std::fs::remove_file(executed.run.run_dir.join("mcp.json"));
    if let Ok(Some(reloaded)) = app.store.load_task(&task.id) {
        *task = reloaded;
    }
    attempts::record_gate_blocks(task, executed.run.idx, blocks);
    task.record_decisions(lines);
    handled
}
