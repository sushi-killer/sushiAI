use crate::api::validation::validate_task_id;
use crate::clock::now_ms;
use crate::engine::review::evidence::attempt_screenshots;
use crate::engine::review::evidence::evidence_scope;
use crate::engine::state::App;
use crate::git::paths::short_sha;
use crate::policy::delivery::BaseFrom;
use crate::prompts::clip::truncate_chars;
use crate::report::{self, Facts};
use crate::task::creation::NewTask;
use crate::task::LeadTouch;
use crate::task::Stage;
use crate::task::Task;
use crate::task::TaskStatus;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

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

/// A follow-up task's title: one line naming the task it follows up, never
/// the request (the note and the report excerpt live there).
fn follow_up_title(task: &Task) -> String {
    let title = task.title.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&format!("Follow-up: {title}"), 80)
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

/// What the task changed: the landing commit against its parent once landed,
/// else its branch against its base.
pub(super) fn task_range(task: &Task) -> (String, String) {
    match &task.landed_sha {
        Some(sha) => (format!("{sha}^"), sha.clone()),
        None => (task.base_sha.clone(), task.branch.clone()),
    }
}

/// `task.diff_stat` from git as it stands now; unchanged when git cannot
/// tell (the branch or the commit is gone).
pub(super) async fn refresh_diff_stat(task: &mut Task) {
    let (from, to) = task_range(task);
    let repo = PathBuf::from(&task.repo);
    let stat = tokio::task::spawn_blocking(move || {
        crate::git::changes::range_stat(&repo, &from, &to).ok()
    })
    .await
    .unwrap_or(None);
    if stat.is_some() {
        task.record_task_diff_stat(stat);
    }
}

/// Stamps a landing: `landed_at` now, and the diff stat of what landed.
pub(super) async fn record_landing(task: &mut Task) {
    task.stamp_landing(now_ms());

    refresh_diff_stat(task).await;
}

/// Per implement attempt of every task in `graph`: its exploration calls
/// (the timeline's `explore` bucket) and the time to its first edit.
fn attempt_facts(
    store: &crate::store::Store,
    graph: &[&Task],
    rules: &crate::policy::settings::WorkBuckets,
) -> Vec<report::AttemptFact> {
    let mut out = Vec::new();
    for task in graph {
        let segments = crate::timeline::task_timeline(store, task, rules, now_ms());
        for a in task.attempts.iter().filter(|a| a.stage == Stage::Implement) {
            let explore = segments
                .iter()
                .find(|s| s.stage == "implement" && s.attempt == a.n)
                .and_then(|s| s.buckets.as_ref())
                .map(|b| b.explore);
            out.push(report::AttemptFact {
                task_id: task.id.clone(),
                n: a.n,
                explore,
                first_edit_ms: a.first_edit_at.map(|at| at - a.started_at),
            });
        }
    }
    out
}

fn gather_facts(
    task: &Task,
    store: &crate::store::Store,
    graph: &[&Task],
    rules: &crate::policy::settings::WorkBuckets,
) -> Facts {
    let repo = Path::new(&task.repo);
    let (from, to) = task_range(task);
    let mut facts = Facts {
        commits: crate::git::history::log_subjects(repo, &from, &to).unwrap_or_default(),
        files: crate::git::changes::numstat(repo, &from, &to).unwrap_or_default(),
        images: Vec::new(),
        attempts: attempt_facts(store, graph, rules),
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

/// Refreshes the patch-aware count of commits on a completed top-level task
/// that are absent from its base branch.
fn refresh_unlanded(task: &mut Task) {
    if task.parent.is_some() || task.status != TaskStatus::Done || task.landed_sha.is_some() {
        task.record_unlanded(None);
        return;
    }
    let Some(base) = task.base_ref.as_deref() else {
        task.record_unlanded(None);
        return;
    };
    task.record_unlanded(
        crate::git::history::cherry_count(Path::new(&task.repo), base, &task.branch)
            .ok()
            .filter(|commits| *commits > 0)
            .map(|commits| crate::task::Unlanded {
                commits,
                base: base.to_string(),
            }),
    );
}

/// Why a landed task counts as touched, when it does: its landing is no
/// longer on the base branch, or a commit not made by orchd changed one of
/// its files there within a day of the landing.
fn auto_touch_note(task: &Task) -> Option<String> {
    let sha = task.landed_sha.as_deref()?;
    let base = task.base_ref.as_deref()?;
    let repo = Path::new(&task.repo);
    if !crate::git::history::is_ancestor(repo, sha, base)? {
        return Some(format!(
            "the landing {} is no longer on {base}: it was rewritten or reverted",
            short_sha(sha)
        ));
    }
    let landed_at = crate::git::history::commit_time_ms(repo, sha)?;
    let files: std::collections::HashSet<String> =
        crate::git::changes::numstat(repo, &format!("{sha}^"), sha)
            .ok()?
            .into_iter()
            .map(|f| f.0)
            .collect();
    let later = crate::git::history::commits_after(repo, sha, base).ok()?;
    let fix = later.iter().find(|c| {
        !c.by_orchd && c.ts_ms <= landed_at + DAY_MS && c.files.iter().any(|f| files.contains(f))
    })?;
    Some(format!(
        "commit {} on {base}, not made by orchd, changed the task's files within 24h of landing",
        short_sha(&fix.sha)
    ))
}

/// Puts the mark and what `write_report` set on `from` (a copy loaded with
/// `decisions_before` decisions) onto `t`, the task as stored now.
fn keep_report(t: &mut Task, from: &Task, decisions_before: usize) {
    t.adopt_report(from);
    for line in from.decisions.iter().skip(decisions_before) {
        if !t.decisions.contains(line) {
            t.record_decision(line.clone());
        }
    }
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
        task.record_lead_touch(Some(LeadTouch {
            touched: true,
            note,
            at: now_ms(),
            by: "auto".to_string(),
        }));
        true
    }

    /// Writes the report of a done top-level task (`report.md` and
    /// `task.report`), first checking whether the work was touched. The
    /// caller saves and broadcasts `task`.
    pub(super) async fn write_report(&self, task: &mut Task) {
        if task.parent.is_some() || task.status != TaskStatus::Done {
            return;
        }
        self.collect_deliverables(task).await;
        let all = self.repo_tasks(&task.repo);
        let data = self.data_dir.clone();
        let rules = self
            .store
            .load_settings()
            .map(|s| s.work_buckets)
            .unwrap_or_default();
        let snapshot = task.clone();
        let (text, touched, unlanded) = tokio::task::spawn_blocking(move || {
            let mut t = snapshot;
            Self::apply_auto_touch(&mut t);
            refresh_unlanded(&mut t);
            let graph = report::graph_of(&t, &all);
            let facts = crate::store::Store::new(&data)
                .map(|store| gather_facts(&t, &store, &graph, &rules))
                .unwrap_or_default();
            let text = report::build(&t, &graph, &crate::store::costs::read_all(&data), &facts);
            (text, t.lead_touch.clone(), t.unlanded.clone())
        })
        .await
        .unwrap_or_default();
        if text.is_empty() {
            return;
        }

        let dir = self.store.task_dir(&task.id);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("report.md"), &text);
        task.record_report(
            crate::task::delivery::operations::ReportObservation {
                text,
                lead_touch: touched,
                unlanded,
            },
            now_ms(),
        );
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
        let before = (task.lead_touch.clone(), task.unlanded.clone());
        self.write_report(&mut task).await;
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        if (task.lead_touch.clone(), task.unlanded.clone()) != before {
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
        task.record_lead_touch(p.touched.map(|touched| LeadTouch {
            touched,
            note: owner_note.clone(),
            at: now_ms(),
            by: "owner".to_string(),
        }));
        let decisions = task.decisions.len();
        if task.report.is_some() {
            self.write_report(&mut task).await;
        }
        // Put onto the task as stored now, under its lock: a finishing loop
        // writes its metrics in the same moments, and a save between its
        // load and its write would lose the mark.
        let mut task = self.update_task(&p.id, |t| keep_report(t, &task, decisions))?;
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
        let task = self
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
                .and_then(|t| t.request.clone())
                .is_some_and(|r| r.starts_with(&prefix))
        });
        if exists {
            return Ok(task);
        }
        let (follow_up, line) = match self.spawn_follow_up(&task, note).await {
            Ok(new) => {
                let mut new = new;
                new.record_decision(format!(
                    "Follow-up of task {}: the owner marked it as needing a fix: {note}",
                    task.id
                ));
                new.link_follow_up(task.id.clone());
                self.store.save_task(&new).map_err(|e| e.to_string())?;
                self.broadcast_task(&new);
                self.spawn_task_loop(new.id.clone(), true);
                let line = format!(
                    "Created follow-up task {} from the owner's note: {note}",
                    new.id
                );
                (Some(new.id.clone()), line)
            }
            Err(e) => (
                None,
                format!("No follow-up task was created for the owner's note: {e}"),
            ),
        };
        // The copy loaded above is stale after the slow creation: only the
        // link and the decision go onto the task as stored now.
        self.update_task(id, |t| {
            t.record_follow_ups(follow_up);
            t.record_decision(line);
        })
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
                if crate::git::history::is_ancestor(Path::new(&repo), &sha, &base)
                    == Some(true) =>
            {
                base
            }
            _ => branch,
        })
        .await
        .map_err(|e| e.to_string())?;
        self.create_task_record(NewTask {
            backlog: None,
            id: uuid::Uuid::new_v4().to_string(),
            repo_root: PathBuf::from(&task.repo),
            project_id: task.project_id.clone(),
            title: follow_up_title(task),
            decisions: vec![],
            goal: String::new(),
            criteria: vec![],
            verify: vec![],
            final_verify: vec![],
            checks: vec![],
            held_out: None,
            request: Some(request),
            branch: None,
            base,
            base_from: BaseFrom::Plain,
            variant,
            depends_on: vec![],
            parent: None,
            paths: vec![],
            checklist: vec![],
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
                move || {
                    app.store.list_tasks().unwrap_or_else(|e| {
                        eprintln!("orch: listing tasks for lead touch check failed: {e}");
                        Vec::new()
                    })
                }
            })
            .await
            .unwrap_or_else(|e| {
                eprintln!("orch: starting lead touch check failed: {e}");
                Vec::new()
            });
            for mut task in tasks {
                if app.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                if task.status != TaskStatus::Done || task.parent.is_some() {
                    continue;
                }
                let checked = tokio::task::spawn_blocking(move || {
                    let before = (task.lead_touch.clone(), task.unlanded.clone());
                    if task.landed_sha.is_some() {
                        App::apply_auto_touch(&mut task);
                    }
                    refresh_unlanded(&mut task);
                    let changed = before != (task.lead_touch.clone(), task.unlanded.clone());
                    (changed, task)
                })
                .await;
                let Ok((true, mut task)) = checked else {
                    continue;
                };
                if task.report.is_some() {
                    app.write_report(&mut task).await;
                }
                task.touch(now_ms());
                app.save_task_logged(&task);
                app.broadcast_task(&task);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{task_with_status, test_app};

    #[tokio::test]
    async fn the_rereviews_of_a_landing_start_over_with_the_next_one() {
        let mut task = task_with_status(TaskStatus::Landing);
        task.fixture_data().loop_state.rereviews = 3;
        record_landing(&mut task).await;
        assert_eq!(task.loop_state.rereviews, 0);
    }

    #[test]
    fn a_follow_up_title_is_one_line_naming_the_original() {
        let mut task = task_with_status(TaskStatus::Done);
        task.fixture_data().title = "Columns are in the wrong order\nfor the Totals row".into();
        let title = follow_up_title(&task);
        assert_eq!(
            title,
            "Follow-up: Columns are in the wrong order for the Totals row"
        );
        assert!(!title.contains('\n'));
        task.fixture_data().title = "x".repeat(200);
        assert!(follow_up_title(&task).chars().count() <= 81);
    }

    /// Metrics are computed from a copy loaded before the overseer log is
    /// read; the log is a FIFO here, so the owner's mark lands in that gap.
    #[tokio::test]
    async fn an_owner_mark_made_while_metrics_are_recorded_is_kept() {
        let (app, home) = test_app();
        let task = task_with_status(TaskStatus::Done);
        let id = task.id.clone();
        app.store.save_task(&task).unwrap();
        let log =
            std::ffi::CString::new(app.data_dir.join("overseer.jsonl").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(log.as_ptr(), 0o600) }, 0);
        let recorder = {
            let (app, id) = (app.clone(), id.clone());
            std::thread::spawn(move || app.record_task_metrics(&id))
        };
        std::thread::sleep(std::time::Duration::from_millis(500));
        app.handle_task_lead_touch(json!({"id": id, "touched": true, "note": "fix it"}))
            .await
            .unwrap();
        // Closing a writer ends the blocked read of the log.
        drop(
            std::fs::OpenOptions::new()
                .write(true)
                .open(app.data_dir.join("overseer.jsonl"))
                .unwrap(),
        );
        recorder.join().unwrap();
        let stored = app.store.load_task(&id).unwrap().unwrap();
        let mark = stored
            .lead_touch
            .clone()
            .expect("the owner's mark survives");
        assert!(mark.touched && mark.by == "owner");
        assert!(stored.metrics.is_some());
        drop(home);
    }

    /// A write under the task lock (the finishing loop's metrics) loaded its
    /// copy before the owner's mark: the mark waits for it and is put onto
    /// what it saved.
    #[tokio::test]
    async fn an_owner_mark_waits_for_a_locked_write_and_is_kept() {
        let (app, home) = test_app();
        let task = task_with_status(TaskStatus::Done);
        let id = task.id.clone();
        app.store.save_task(&task).unwrap();
        let (loaded_tx, loaded_rx) = std::sync::mpsc::channel();
        let writer = {
            let (app, id) = (app.clone(), id.clone());
            std::thread::spawn(move || {
                let lock = app.task_lock(&id);
                let _guard = lock.lock().unwrap();
                let mut stale = app.store.load_task(&id).unwrap().unwrap();
                loaded_tx.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(300));
                stale
                    .fixture_data()
                    .decisions
                    .push("written under the lock".into());
                app.store.save_task(&stale).unwrap();
            })
        };
        loaded_rx.recv().unwrap();
        app.handle_task_lead_touch(json!({"id": id, "touched": true}))
            .await
            .unwrap();
        writer.join().unwrap();
        let stored = app.store.load_task(&id).unwrap().unwrap();
        let mark = stored
            .lead_touch
            .clone()
            .expect("the owner's mark survives");
        assert!(mark.touched && mark.by == "owner");
        assert!(stored
            .decisions
            .contains(&"written under the lock".to_string()));
        drop(home);
    }
}
