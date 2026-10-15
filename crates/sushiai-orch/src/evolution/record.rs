use super::detection::{AttemptEvents, Detection};
use super::evidence::{already_detected, append_line, evolution_dir, load_events};
use crate::clock::now_ms;
use crate::engine::{messages, state::App};
use crate::harness::cancel::CancelToken;
use crate::store::Store;
use crate::task::{Stage, Task, TaskStatus};
use std::path::Path;

/// The commands a failed task could not get past: non-zero verify outcomes
/// and the command a parent's final check tripped on. Each appears once.
fn failing_commands(task: &Task) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |c: &str| {
        if !c.is_empty() && !out.iter().any(|o| o == c) {
            out.push(c.to_string());
        }
    };
    for v in task.attempts.iter().flat_map(|a| &a.verify) {
        if matches!(v.code, Some(c) if c != 0) {
            add(&v.command);
        }
    }
    for command in &task.loop_state.final_check_failures {
        add(command);
    }
    out
}

impl App {
    /// Called once from `finish_task_loop`; the work runs in the background
    /// and never holds up the loop.
    pub(in crate::engine) fn record_evolution_signals(&self, task_id: &str) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let app = self.arc();
        let task_id = task_id.to_string();
        runtime.spawn(async move { app.detect_evolution_signals(&task_id).await });
    }

    pub(in crate::engine) async fn detect_evolution_signals(&self, task_id: &str) {
        // Held across check and append: two calls for one task record it once.
        let _guard = self.evolution_lock.lock().await;
        let Ok(Some(task)) = self.store.load_task(task_id) else {
            return;
        };
        if !matches!(task.status, TaskStatus::Done | TaskStatus::Failed) || task.eval_set.is_some()
        {
            return;
        }
        let dir = evolution_dir(&self.data_dir);
        if already_detected(&dir, &task.id) {
            return;
        }
        let settings = self.settings.read().unwrap().clone();
        let mut base_failures = Vec::new();
        if task.status == TaskStatus::Failed {
            let run_dir = self.store.task_dir(&task.id).join("runs").join("evolution");
            let _ = std::fs::create_dir_all(&run_dir);
            let cancel = CancelToken::new();
            for cmd in failing_commands(&task) {
                let check = crate::engine::verification::runner::run_eval_check(
                    crate::engine::verification::runner::CheckRunner::from_context(
                        crate::engine::verification::context::CheckContext::new(
                            self,
                            Some(task_id),
                        ),
                        &run_dir.join("eval-check"),
                        self.task_sandbox(&task.repo, &run_dir.join("eval-check"), &run_dir)
                            .as_ref(),
                        &task.base_sha,
                        &cancel,
                    ),
                    Path::new(&task.repo),
                    &cmd,
                )
                .await;
                if matches!(check.code, Some(c) if c != 0)
                    && !crate::verify::load::timed_out_under_load(&check)
                {
                    base_failures.push(cmd);
                }
            }
        }
        let store_dir = self.data_dir.clone();
        let repo_tasks = self.repo_tasks(&task.repo);
        let detected = {
            let (task, settings) = (task.clone(), settings.clone());
            tokio::task::spawn_blocking(move || {
                let store = Store::new(&store_dir).ok()?;
                let mut events = AttemptEvents::new();
                for t in repo_tasks.iter().filter(|t| t.eval_set.is_none()) {
                    for a in t.attempts.iter().filter(|a| a.stage == Stage::Implement) {
                        if let Some(evs) = load_events(&store, &t.id, a.n) {
                            events.insert((t.id.clone(), a.n), evs);
                        }
                    }
                }
                let messages = messages::load(&store_dir);
                Some(Detection::for_task(&task, &events, &settings).finish(
                    &repo_tasks,
                    &messages,
                    &base_failures,
                ))
            })
            .await
        };
        let Ok(Some(signals)) = detected else {
            return;
        };
        for s in &signals {
            let Ok(mut value) = serde_json::to_value(s) else {
                continue;
            };
            // Every stored signal carries its time, so "latest" and age mean something.
            value["ts"] = serde_json::json!(now_ms());
            let Ok(line) = serde_json::to_string(&value) else {
                continue;
            };
            if append_line(&dir.join("signals.jsonl"), &line).is_err() {
                return;
            }
        }
        let _ = append_line(
            &dir.join("detected.jsonl"),
            &serde_json::Value::String(task.id.clone()).to_string(),
        );
    }
}

#[cfg(test)]
mod tests;
