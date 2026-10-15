//! A pure observation pass over one task, its traces, and earlier repository work.

mod attempts;
mod calls;
mod graph;
mod provenance;
#[cfg(test)]
mod tests;

use crate::policy::settings::Settings;
use crate::task::evolution::signals::{Signal, SignalKind};
use crate::task::messages::Message;
use crate::task::{AnsweredBy, Task};
use serde_json::Value;
use std::collections::HashMap;

/// One event per original line; malformed lines remain Null to preserve references.
pub type AttemptEvents = HashMap<(String, u32), Vec<Value>>;

pub struct Detection<'a> {
    task: &'a Task,
    settings: &'a Settings,
    events: &'a AttemptEvents,
    out: Vec<Signal>,
}

impl<'a> Detection<'a> {
    pub fn for_task(task: &'a Task, events: &'a AttemptEvents, settings: &'a Settings) -> Self {
        Self {
            task,
            settings,
            events,
            out: Vec::new(),
        }
    }

    pub fn finish(
        mut self,
        repo_tasks: &[Task],
        messages: &[Message],
        base_failures: &[String],
    ) -> Vec<Signal> {
        let earlier: Vec<&Task> = repo_tasks
            .iter()
            .filter(|t| {
                t.id != self.task.id
                    && t.repo == self.task.repo
                    && t.eval_set.is_none()
                    && t.created_at < self.task.created_at
            })
            .collect();
        self.per_attempt(&earlier);
        self.discovery(&earlier);
        self.repeated_review();
        self.owner_questions();
        self.graph(repo_tasks, messages);
        for cmd in base_failures {
            self.source(0, None).record(
                SignalKind::Preexisting,
                format!("`{cmd}` fails on the task's base commit too"),
                0,
            );
        }
        self.out
    }

    fn owner_questions(&mut self) {
        for q in self
            .task
            .question_history
            .iter()
            .filter(|q| q.answered_by == AnsweredBy::Owner)
        {
            self.source(0, None)
                .record(SignalKind::OwnerQuestion, q.answer.clone(), 0);
        }
    }
}
