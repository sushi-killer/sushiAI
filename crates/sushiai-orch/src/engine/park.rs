use super::*;

pub(super) struct Cancellation {
    park: Option<ParkRecord>,
    explicit: bool,
    line: String,
    last_answer: Option<AnsweredQuestion>,
    pub(super) restart: bool,
    pub(super) restart_children: bool,
}

impl Cancellation {
    pub(super) fn apply(&self, task: &mut Task) {
        task.queue.park = if self.restart {
            None
        } else {
            self.park.clone()
        };
        task.queue.stopped_explicitly = self.explicit && !self.restart;
        task.status = if self.restart || task.queue.park.is_some() {
            TaskStatus::Queued
        } else {
            TaskStatus::Stopped
        };
        if !task.decisions.contains(&self.line) {
            task.decisions.push(self.line.clone());
        }
        if let Some(p) = &self.park {
            let note = format!("Park: {} (by {})", p.reason, p.by.label());
            if !task.decisions.contains(&note) {
                task.decisions.push(note);
            }
        }
        if let Some(answer) = &self.last_answer {
            if !task.question_history.contains(answer) {
                task.question_history.push(answer.clone());
            }
        }
        task.queue.restart_children = self.restart_children;
        task.question = None;
    }
}

impl AnswerSource {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::Lead => "Lead",
            Self::Chat => "orchestrator chat",
            Self::Mcp => "MCP caller",
            Self::Overseer => "overseer",
            Self::Policy => "policy",
        }
    }

    pub(super) fn control_line(self, action: &str, reason: Option<&str>) -> String {
        let origin = match self {
            Self::Owner => format!("Owner: {action} (UI)"),
            Self::Lead => format!("Lead: {action} (RPC)"),
            source => format!("Orchestrator: {action} (requested by {})", source.label()),
        };
        match reason.map(str::trim).filter(|r| !r.is_empty()) {
            Some(reason) => format!("{origin}: {reason}"),
            None => origin,
        }
    }
}

impl App {
    pub(super) fn is_suspended(&self, task: &Task) -> bool {
        task.queue.park.is_some()
            || task.queue.stopped_explicitly
            || self
                .cancellations
                .lock()
                .unwrap()
                .get(&task.id)
                .is_some_and(|c| !c.restart)
    }

    /// Saves the interruption before cancelling; queued waiters need not wait
    /// for the harness process or its loop to exit.
    pub(super) fn interrupt_task(
        &self,
        task: &mut Task,
        park: Option<ParkRecord>,
        explicit: bool,
        line: String,
    ) -> Result<(), String> {
        let mut cancellations = self.cancellations.lock().unwrap();
        let live = self.controls.lock().unwrap().contains_key(&task.id);
        task.question = None;
        task.queue.park = park.clone();
        task.queue.stopped_explicitly = explicit;
        task.status = if park.is_some() {
            TaskStatus::Queued
        } else {
            TaskStatus::Stopped
        };
        if !task.decisions.contains(&line) {
            task.decisions.push(line.clone());
        }
        if let Some(p) = &park {
            let note = format!("Park: {} (by {})", p.reason, p.by.label());
            if !task.decisions.contains(&note) {
                task.decisions.push(note);
            }
        }
        task.updated_at = now_ms();
        self.store.save_task(task).map_err(|e| e.to_string())?;
        if live {
            cancellations.insert(
                task.id.clone(),
                Cancellation {
                    park,
                    explicit,
                    line,
                    last_answer: task.question_history.last().cloned(),
                    restart: false,
                    restart_children: false,
                },
            );
        }
        drop(cancellations);
        self.broadcast_task(task);
        if let Some(control) = self.controls.lock().unwrap().get(&task.id) {
            control.cancel.cancel();
        }
        self.advance_queue(&task.repo);
        Ok(())
    }

    pub(super) fn apply_cancellation(&self, task: &mut Task) {
        if let Some(end) = self.cancellations.lock().unwrap().get(&task.id) {
            end.apply(task);
        } else if task.queue.park.is_some() {
            task.status = TaskStatus::Queued;
        } else {
            task.status = self.cancelled_status(&task.status);
        }
        if task.status == TaskStatus::Stopped || task.queue.park.is_some() {
            task.question = None;
        }
    }

    pub(super) async fn handle_task_park(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            reason: String,
            #[serde(default)]
            until: Option<String>,
            #[serde(default)]
            source: AnswerSource,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let reason = p.reason.trim();
        if reason.is_empty() {
            return Err("a park reason is required".into());
        }
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.archived || task.status == TaskStatus::Done || task.queue.stopped_explicitly {
            return Err("a done, archived or explicitly stopped task cannot be parked".into());
        }
        if let Some(until) = &p.until {
            validate_task_id(&self.store, until)?;
            let target = self
                .store
                .load_task(until)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "until task not found".to_string())?;
            if target.repo != task.repo {
                return Err("until must be a task in the same repo".into());
            }
            let all = self.repo_tasks(&task.repo);
            let mut edges = wait_edges(&all);
            for t in &all {
                if let Some(until) = t.queue.park.as_ref().and_then(|p| p.until.as_ref()) {
                    edges.entry(t.id.clone()).or_default().push(until.clone());
                }
            }
            edges
                .entry(task.id.clone())
                .or_default()
                .push(until.clone());
            if has_cycle(&edges) {
                return Err("park until would create a dependency cycle".into());
            }
        }
        let park = ParkRecord {
            reason: reason.to_string(),
            until: p.until,
            by: p.source,
        };
        self.interrupt_task(
            &mut task,
            Some(park),
            false,
            p.source.control_line("park", Some(reason)),
        )?;
        self.advance_graph(&task.repo);
        serde_json::to_value(task).map_err(|e| e.to_string())
    }
}
