use crate::clock::now_ms;
use crate::harness::process::kill_stale_process_group;
use crate::harness::run_files::{clear_interrupted_credentials, RunFate};
use crate::store::Store;
use crate::task::audit::{Audit, AuditStatus};
use crate::task::{AttemptStatus, Task};
use std::{fs, io};

pub(in crate::engine) fn tasks(
    store: &Store,
    mut fate: impl FnMut(&Task, usize) -> RunFate,
    mut settle: impl FnMut(&mut Task, usize),
) -> io::Result<Vec<Task>> {
    let mut recovered = Vec::new();
    for mut task in store.list_tasks()? {
        let mut interrupted = false;
        for idx in 0..task.attempts.len() {
            if task.attempts[idx].status != AttemptStatus::Running {
                continue;
            }
            if matches!(fate(&task, idx), RunFate::Resume) {
                continue;
            }
            let attempt = &task.attempts[idx];
            let run_dir = store.run_dir(&task.id, attempt.n);
            if let Some(pgid) = attempt.pgid {
                kill_stale_process_group(pgid, &run_dir);
            }
            clear_interrupted_credentials(&run_dir);
            crate::task::attempts::end(
                &mut task,
                idx,
                crate::task::attempts::AttemptEnd::Interrupted,
                now_ms(),
            );
            settle(&mut task, idx);
            interrupted = true;
        }
        if interrupted {
            crate::task::transitions::recover_interruption(&mut task, now_ms());
            if let Err(e) = store.save_task(&task) {
                eprintln!("orch: saving task {} failed: {e}", task.id);
                continue;
            }
            recovered.push(task);
        }
    }
    Ok(recovered)
}

pub(super) fn audits(store: &Store) -> io::Result<Vec<Audit>> {
    let dir = store.data_dir().join("audits");
    let mut recovered = Vec::new();
    if !dir.exists() {
        return Ok(recovered);
    }
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let Ok(Some(mut audit)) =
            crate::store::files::read_json::<Audit>(&entry.path().join("audit.json"))
        else {
            continue;
        };
        if audit.status == AuditStatus::Running {
            let _ = fs::remove_file(entry.path().join("key"));
            audit.status = AuditStatus::Stopped;
            audit.error = Some("Interrupted by a daemon restart.".to_string());
            audit.ended_at = Some(now_ms());
            store.save_audit(&audit)?;
            recovered.push(audit);
        }
    }
    Ok(recovered)
}
