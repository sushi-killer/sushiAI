use super::*;
use crate::report::{self, Facts};

const DAY_MS: i64 = 86_400_000;

/// Git facts about the work of a finished task: its landing commit, else the
/// commits its branch holds over the base it started from.
fn gather_facts(task: &Task) -> Facts {
    let repo = Path::new(&task.repo);
    let (from, to) = match &task.landed_sha {
        Some(sha) => (format!("{sha}^"), sha.clone()),
        None => (task.base_sha.clone(), task.branch.clone()),
    };
    let mut facts = Facts {
        commits: git::log_subjects(repo, &from, &to).unwrap_or_default(),
        files: git::numstat(repo, &from, &to).unwrap_or_default(),
        images: Vec::new(),
    };
    let wt = Path::new(&task.worktree);
    if wt.join("artifacts").is_dir() {
        for p in attempt_screenshots(wt, task.created_at) {
            facts.images.push(p.display().to_string());
        }
    }
    for (path, ..) in &facts.files {
        if report::is_image(path) {
            facts.images.push(repo.join(path).display().to_string());
        }
    }
    facts
}

/// Why a landed task counts as touched, when it does: its landing is no
/// longer on the base branch, or a commit not made by orchd changed one of
/// its files there within a day of the landing.
fn auto_touch_note(task: &Task) -> Option<String> {
    let sha = task.landed_sha.as_deref()?;
    let base = task.base_ref.as_deref()?;
    let repo = Path::new(&task.repo);
    if !git::is_ancestor(repo, sha, base)? {
        return Some(format!(
            "the landing {} is no longer on {base}: it was rewritten or reverted",
            short_sha(sha)
        ));
    }
    let landed_at = git::commit_time_ms(repo, sha)?;
    let files: std::collections::HashSet<String> = git::numstat(repo, &format!("{sha}^"), sha)
        .ok()?
        .into_iter()
        .map(|f| f.0)
        .collect();
    let later = git::commits_after(repo, sha, base).ok()?;
    let fix = later.iter().find(|c| {
        !c.by_orchd && c.ts_ms <= landed_at + DAY_MS && c.files.iter().any(|f| files.contains(f))
    })?;
    Some(format!(
        "commit {} on {base}, not made by orchd, changed the task's files within 24h of landing",
        short_sha(&fix.sha)
    ))
}

impl App {
    /// Marks a landed task touched (by auto) when git says so. Never
    /// overrides the owner's mark or an existing touched mark. `true` when
    /// the mark changed.
    fn apply_auto_touch(task: &mut Task) -> bool {
        if task.status != TaskStatus::Done
            || task
                .lead_touch
                .as_ref()
                .is_some_and(|t| t.touched || t.by == "owner")
        {
            return false;
        }
        let Some(note) = auto_touch_note(task) else {
            return false;
        };
        task.lead_touch = Some(LeadTouch {
            touched: true,
            note,
            at: now_ms(),
            by: "auto".to_string(),
        });
        true
    }

    /// Writes the report of a done top-level task (`report.md` and
    /// `task.report`), first checking whether the work was touched. The
    /// caller saves and broadcasts `task`.
    pub(super) async fn write_report(&self, task: &mut Task) {
        if task.parent.is_some() || task.status != TaskStatus::Done {
            return;
        }
        let all = self.repo_tasks(&task.repo);
        let data = self.data_dir.clone();
        let snapshot = task.clone();
        let (text, touched) = tokio::task::spawn_blocking(move || {
            let mut t = snapshot;
            Self::apply_auto_touch(&mut t);
            let graph = report::graph_of(&t, &all);
            let text = report::build(
                &t,
                &graph,
                &crate::costs::read_all(&data),
                &gather_facts(&t),
            );
            (text, t.lead_touch)
        })
        .await
        .unwrap_or_default();
        if text.is_empty() {
            return;
        }
        task.lead_touch = touched;
        let dir = self.store.task_dir(&task.id);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("report.md"), &text);
        task.report = Some(text);
        task.report_at.get_or_insert_with(now_ms);
    }

    /// `task.report {id}` -> `{id, report}`: the report of a done top-level
    /// task, rebuilt now so the lead-touch check and mark are current.
    pub(super) async fn handle_task_report(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
        validate_task_id(&self.store, id)?;
        let mut task = self
            .store
            .load_task(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.parent.is_some() {
            return Err("only a top-level task has a report".to_string());
        }
        if task.status != TaskStatus::Done {
            return Err("the task is not done".to_string());
        }
        let before = task.lead_touch.clone();
        self.write_report(&mut task).await;
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        if task.lead_touch != before {
            self.broadcast_task(&task);
        }
        Ok(json!({"id": task.id, "report": task.report}))
    }

    /// `task.leadTouch {id, touched?, note?}`: the owner's mark on a done
    /// task; without `touched` the mark is cleared (unknown again).
    pub(super) async fn handle_task_lead_touch(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            #[serde(default)]
            touched: Option<bool>,
            #[serde(default)]
            note: Option<String>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.status != TaskStatus::Done {
            return Err("the task is not done".to_string());
        }
        task.lead_touch = p.touched.map(|touched| LeadTouch {
            touched,
            note: p.note.unwrap_or_default().trim().to_string(),
            at: now_ms(),
            by: "owner".to_string(),
        });
        if task.report.is_some() {
            self.write_report(&mut task).await;
        }
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    /// On daemon start: every landed done task not yet marked touched gets
    /// its landing checked against its base branch, in the background.
    pub(super) fn spawn_lead_touch_check(&self) {
        let app = self.arc();
        tokio::spawn(async move {
            let tasks = tokio::task::spawn_blocking({
                let app = app.clone();
                move || app.store.list_tasks().unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            for mut task in tasks {
                if app.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                if task.landed_sha.is_none() || task.status != TaskStatus::Done {
                    continue;
                }
                let checked = tokio::task::spawn_blocking(move || {
                    let changed = App::apply_auto_touch(&mut task);
                    (changed, task)
                })
                .await;
                let Ok((true, mut task)) = checked else {
                    continue;
                };
                if task.report.is_some() {
                    app.write_report(&mut task).await;
                }
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
            }
        });
    }
}
