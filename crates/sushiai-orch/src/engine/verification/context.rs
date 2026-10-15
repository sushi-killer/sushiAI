use crate::clock::now_ms;
use crate::engine::state::App;
use crate::task::attempt::VerifyOutcome;
use crate::task::state::Task;

#[derive(Clone, Copy)]
pub(in crate::engine) struct CheckContext<'a> {
    pub(super) app: &'a App,
    task_id: Option<&'a str>,
    pub(super) label: Option<&'a str>,
    pub(super) gate: &'a str,
}

impl<'a> CheckContext<'a> {
    pub(in crate::engine) fn new(app: &'a App, task_id: Option<&'a str>) -> Self {
        Self {
            app,
            task_id,
            label: None,
            gate: "verify",
        }
    }

    pub(in crate::engine) fn with_gate(self, gate: &'a str) -> Self {
        Self { gate, ..self }
    }

    pub(in crate::engine) fn with_label(self, label: &'a str) -> Self {
        Self {
            label: Some(label),
            ..self
        }
    }

    pub(in crate::engine) fn decision(&self, mut line: String, lines: &mut Vec<String>) {
        let mut stored = self
            .task_id
            .and_then(|id| self.app.store.load_task(id).ok().flatten());
        if let Some(held) = stored
            .as_ref()
            .and_then(|task| task.held_out.as_ref())
            .filter(|held| !held.run.is_empty())
        {
            line = line.replace(
                &held.run,
                &crate::task::checks::commands::held_out_label(held.criterion),
            );
        }
        lines.push(line.clone());
        if let Some(id) = self.task_id {
            {
                let controls = self.app.controls.lock().unwrap();
                if controls.contains_key(id) {
                    self.app
                        .check_decisions
                        .lock()
                        .unwrap()
                        .entry(id.to_string())
                        .or_default()
                        .push(line.clone());
                }
            }
            if let Some(task) = stored.as_mut() {
                record_check_decisions(task, vec![line]);
                task.touch(now_ms());
                self.app.save_task_logged(task);
                self.app.broadcast_task(task);
            }
        }
    }
}

/// Adds each check decision line once.
pub(in crate::engine) fn record_check_decisions(task: &mut Task, lines: Vec<String>) {
    for line in lines {
        if !task.decisions.contains(&line) {
            task.record_decision(line);
        }
    }
}

pub(in crate::engine) fn record_check_outcomes(
    app: &App,
    task: &mut Task,
    results: &[VerifyOutcome],
) {
    let mut lines = app
        .check_decisions
        .lock()
        .unwrap()
        .remove(&task.id)
        .unwrap_or_default();
    lines.extend(results.iter().flat_map(|r| r.decisions.iter().cloned()));
    record_check_decisions(task, lines);
}
