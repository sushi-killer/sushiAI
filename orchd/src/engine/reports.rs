use super::*;
use crate::report::{self, Facts};

const DAY_MS: i64 = 86_400_000;
const EXCERPT_MAX: usize = 3000;

/// Held across the check-and-create of a follow-up so two identical
/// concurrent marks cannot create two tasks.
static FOLLOW_UP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The `## What changed`, `## Criteria` and `## Follow-ups` sections of a
/// report, trimmed to a bounded length.
fn report_excerpt(report: &str) -> String {
    let mut out = String::new();
    let mut keep = false;
    for line in report.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            let h = h.trim();
            keep = h == "What changed" || h == "Criteria" || h == "Follow-ups";
        }
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    truncate_chars(out.trim(), EXCERPT_MAX)
}

/// The request of a follow-up task: the owner's note first, then the
/// original's title, goal and criteria, then an excerpt of its report.
fn follow_up_request(task: &Task, note: &str) -> String {
    let mut text = format!("{note}\n\nFollow-up to task \"{}\".\n", task.title);
    if !task.goal.trim().is_empty() {
        text.push_str(&format!("\nOriginal goal: {}\n", task.goal.trim()));
    }
    if !task.criteria.is_empty() {
        text.push_str("\nOriginal criteria:\n");
        for c in &task.criteria {
            text.push_str(&format!("- {c}\n"));
        }
    }
    if let Some(report) = &task.report {
        let excerpt = report_excerpt(report);
        if !excerpt.is_empty() {
            text.push_str(&format!("\nExcerpt of its report:\n\n{excerpt}\n"));
        }
    }
    text
}

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
        for p in attempt_screenshots(wt, task.created_at, &evidence_scope(task)) {
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
        let owner_note = p.note.unwrap_or_default().trim().to_string();
        task.lead_touch = p.touched.map(|touched| LeadTouch {
            touched,
            note: owner_note.clone(),
            at: now_ms(),
            by: "owner".to_string(),
        });
        if task.report.is_some() {
            self.write_report(&mut task).await;
        }
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        // Only the owner's own mark counts: clearing it lets the report
        // re-apply an automatic one, which never creates a follow-up.
        if p.touched == Some(true) && !owner_note.is_empty() {
            task = self
                .create_follow_up(&task.id, &owner_note)
                .await
                .unwrap_or(task);
        }
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }

    /// Creates the follow-up task for an owner's mark with `note` and links
    /// both tasks; a note already behind a listed follow-up creates nothing.
    /// A failure is recorded as a decision on the original. Returns the
    /// original as saved.
    async fn create_follow_up(&self, id: &str, note: &str) -> Result<Task, String> {
        let _guard = FOLLOW_UP_LOCK.lock().await;
        let mut task = self
            .store
            .load_task(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let prefix = format!("{note}\n\n");
        let exists = task.follow_ups.iter().any(|f| {
            self.store
                .load_task(f)
                .ok()
                .flatten()
                .and_then(|t| t.request)
                .is_some_and(|r| r.starts_with(&prefix))
        });
        if exists {
            return Ok(task);
        }
        match self.spawn_follow_up(&task, note).await {
            Ok(new) => {
                let mut new = new;
                new.decisions.push(format!(
                    "Follow-up of task {}: the owner marked it as needing a fix: {note}",
                    task.id
                ));
                new.follow_up_of = Some(task.id.clone());
                self.store.save_task(&new).map_err(|e| e.to_string())?;
                self.broadcast_task(&new);
                task.follow_ups.push(new.id.clone());
                task.decisions.push(format!(
                    "Created follow-up task {} from the owner's note: {note}",
                    new.id
                ));
                self.spawn_task_loop(new.id.clone(), true);
            }
            Err(e) => task.decisions.push(format!(
                "No follow-up task was created for the owner's note: {e}"
            )),
        }
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        Ok(task)
    }

    async fn spawn_follow_up(&self, task: &Task, note: &str) -> Result<Task, String> {
        let settings = self.settings.read().unwrap().clone();
        let variant = task.variant.clone().unwrap_or_default();
        let planner = variant.plan_route_id(&settings);
        if planner.is_empty() {
            return Err("the planner is disabled".to_string());
        }
        if !settings.routes.iter().any(|r| r.id == planner) {
            return Err(format!("planner route \"{planner}\" is not configured"));
        }
        let request = follow_up_request(task, note);
        let repo = task.repo.clone();
        let (sha, base_ref) = (task.landed_sha.clone(), task.base_ref.clone());
        let branch = task.branch.clone();
        let base = tokio::task::spawn_blocking(move || match (sha, base_ref) {
            (Some(sha), Some(base))
                if git::is_ancestor(Path::new(&repo), &sha, &base) == Some(true) =>
            {
                base
            }
            _ => branch,
        })
        .await
        .map_err(|e| e.to_string())?;
        self.create_task_record(NewTask {
            id: uuid::Uuid::new_v4().to_string(),
            repo_root: PathBuf::from(&task.repo),
            title: truncate_chars(&request, 60),
            goal: String::new(),
            criteria: vec![],
            verify: vec![],
            final_verify: vec![],
            checks: vec![],
            held_out: None,
            request: Some(request),
            branch: None,
            base,
            variant,
            depends_on: vec![],
            parent: None,
            paths: vec![],
            relay_of: None,
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            source: Some("handoff".to_string()),
            created_at: now_ms(),
        })
        .await
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
