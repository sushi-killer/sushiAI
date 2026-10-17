use crate::clock::now_ms;
use crate::engine::decision::implement_attempt_count;
use crate::engine::graph::dependencies::DEPENDENCY_QUESTION;
use crate::engine::state::App;
use crate::task::graph::is_parent;
use crate::task::graph::own_waits_state;
use crate::task::graph::unmet_titles;
use crate::task::graph::waits_state;
use crate::task::graph::Waits;
use crate::task::Task;
use crate::task::TaskStatus;
use crate::task::Wait;
use std::sync::atomic::Ordering;
use std::sync::Arc;

impl App {
    pub(in crate::engine) fn landing_lock(&self, parent_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.landing_locks
            .lock()
            .unwrap()
            .entry(parent_id.to_string())
            .or_default()
            .clone()
    }
}

impl App {
    pub(in crate::engine) fn repo_tasks(&self, repo: &str) -> Vec<Task> {
        let mut tasks = self.store.list_tasks().unwrap_or_else(|e| {
            eprintln!("orch: listing graph tasks failed: {e}");
            Vec::new()
        });
        tasks.retain(|t| t.repo == repo);
        tasks
    }
}

impl App {
    /// Moves a repo's task graph along after something in it changed: a
    /// queued task (or a running parent) with no live loop starts once all
    /// it waits for is done, and waits for the owner when one of those ended
    /// `failed`/`stopped`. Tasks outside any graph are never touched.
    pub(in crate::engine) fn advance_graph(&self, repo: &str) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let mut all = self.repo_tasks(repo);
        self.wake_graph_parks(&mut all);
        for task in all.iter().filter(|t| !t.archived) {
            let parent = is_parent(task, &all);
            let asking = task.status == TaskStatus::Waiting
                && task
                    .question
                    .as_ref()
                    .is_some_and(|q| q.text.starts_with(DEPENDENCY_QUESTION));
            let idle = task.status == TaskStatus::Queued
                || (parent && task.status == TaskStatus::Running)
                || asking;
            if !idle
                || task.queue.backlog.is_some()
                || self.is_suspended(task)
                || self.controls.lock().unwrap().contains_key(&task.id)
            {
                continue;
            }
            // Waiting for a file: `advance_queue` starts it when it is free.
            if self.file_blocked(task) {
                continue;
            }
            if !self.graph_parent_allows_start(task, &all) {
                continue;
            }
            let in_graph = parent || task.parent.is_some() || !task.depends_on.is_empty();
            let state = waits_state(task, &all);
            // A parent whose children were only just drafted has to start
            // them itself once its own dependencies are done.
            let unstarted_children = parent
                && matches!(own_waits_state(task, &all), Waits::Ready | Waits::Nothing)
                && all.iter().any(|t| {
                    t.parent.as_deref() == Some(task.id.as_str())
                        && !t.archived
                        && t.status == TaskStatus::Drafting
                });
            match state {
                // Also picks up a task restarted while its previous loop was
                // still winding down (that spawn was a no-op), and a
                // dependent whose question is moot because everything it
                // waited for finished meanwhile.
                Waits::Ready | Waits::Nothing if in_graph => {
                    if asking {
                        if let Err(e) = self.update_task(&task.id, |task| {
                            task.clear_question();
                            task.resume_phase(TaskStatus::Queued, crate::clock::now_ms());
                        }) {
                            eprintln!("orch: updating task {} failed: {e}", task.id);
                        }
                    }
                    if task.parent.is_some() {
                        self.spawn_child_loop(&all, task);
                    } else {
                        self.spawn_task_loop(task.id.clone(), true)
                    }
                }
                // Everything that had ended was restarted and still runs:
                // the question is moot, the task just waits again.
                Waits::Pending if asking => {
                    if let Err(e) = self.update_task(&task.id, |task| {
                        task.clear_question();
                        task.resume_phase(TaskStatus::Queued, crate::clock::now_ms());
                    }) {
                        eprintln!("orch: updating task {} failed: {e}", task.id);
                    }
                    if unstarted_children {
                        self.spawn_task_loop(task.id.clone(), true)
                    }
                }
                Waits::Pending if unstarted_children => self.spawn_task_loop(task.id.clone(), true),
                Waits::Ended(ended) if !asking => {
                    self.ask_dependency_question(task, &all, &ended);
                }
                Waits::Pending if !parent && !asking => self.note_dependency_wait(task, &all),
                _ => {}
            }
        }
        self.advance_queue(repo);
        self.advance_autopilot();
    }
}

impl App {
    /// A task with no loop that waits for other tasks shows which ones.
    pub(in crate::engine) fn note_dependency_wait(&self, task: &Task, all: &[Task]) {
        let wait = Wait::Dependency {
            titles: unmet_titles(task, all),
        };
        if task.queue.wait.as_ref() != Some(&wait) {
            if let Err(e) = self.update_task(&task.id, |t| t.set_wait(Some(wait))) {
                eprintln!("orch: updating task {} failed: {e}", task.id);
            }
        }
    }
}

impl App {
    /// Starts `child`'s loop unless its parent already has `childParallel`
    /// subtasks running. `false`: not started; the graph comes back to it
    /// when a sibling ends.
    pub(in crate::engine) fn spawn_child_loop(&self, all: &[Task], child: &Task) -> bool {
        let cap = self.settings.read().unwrap().child_parallel.max(1) as usize;
        let _admit = self.child_admission.lock().unwrap();
        let running = {
            let controls = self.controls.lock().unwrap();
            all.iter()
                .filter(|t| {
                    t.parent == child.parent && t.id != child.id && controls.contains_key(&t.id)
                })
                .count()
        };
        if running >= cap {
            let wait = Wait::SubtaskSlot {
                running: running as u32,
                limit: cap as u32,
            };
            if child.queue.wait.as_ref() != Some(&wait) {
                if let Err(e) = self.update_task(&child.id, |t| t.set_wait(Some(wait))) {
                    eprintln!("orch: updating task {} failed: {e}", child.id);
                }
            }
            return false;
        }
        self.spawn_task_loop(child.id.clone(), true);
        true
    }
}

impl App {
    fn wake_graph_parks(&self, all: &mut [Task]) {
        for task in all {
            let ready = task
                .queue
                .park
                .as_ref()
                .and_then(|p| p.until.as_ref())
                .and_then(|id| self.store.load_task(id).ok().flatten())
                .is_some_and(|target| target.status == TaskStatus::Done);
            if ready && !task.archived && !self.controls.lock().unwrap().contains_key(&task.id) {
                // Cleared on the current copy, under the task lock, while it is still parked.
                let until = task.queue.park.as_ref().and_then(|p| p.until.clone());
                let saved = self.update_task_if(&task.id, |fresh| {
                    let same = !fresh.archived
                        && fresh.queue.park.as_ref().and_then(|p| p.until.clone()) == until;
                    if same {
                        fresh.clear_park();
                        fresh.record_decision("Park: the until task is done, resuming".into());
                        fresh.touch(now_ms());
                    }
                    same
                });
                if let Some(saved) = saved {
                    *task = saved;
                    self.spawn_task_loop(task.id.clone(), true);
                }
            }
        }
    }
}

impl App {
    fn graph_parent_allows_start(&self, task: &Task, all: &[Task]) -> bool {
        // A subtask never starts while its parent still waits for the
        // parent's own dependencies or sits in the backlog.
        if let Some(p) = task
            .parent
            .as_deref()
            .and_then(|pid| all.iter().find(|t| t.id == pid))
        {
            if !matches!(own_waits_state(p, all), Waits::Ready | Waits::Nothing)
                || p.queue.backlog.is_some()
                || self.is_suspended(p)
                || matches!(p.status, TaskStatus::Stopped | TaskStatus::Failed)
                || (p.status == TaskStatus::Waiting && implement_attempt_count(task) == 0)
            {
                return false;
            }
        }
        true
    }
}
