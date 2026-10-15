use crate::clock::now_ms;
use crate::engine::answer_policy::policy_open;
use crate::engine::answer_policy::record_policy_answer;
use crate::engine::ladder;
use crate::engine::questions::answers::record_answer_for;
use crate::engine::state::App;
use crate::task::graph::is_parent;
use crate::task::graph::waits_for;
use crate::task::AnsweredBy;
use crate::task::AskedBy;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::Task;
use crate::task::TaskStatus;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Asked when something a task waits for ended `failed`/`stopped`; its
/// answer is handled by `answer_dependency_question`, not the attempt loop.
pub(in crate::engine) const DEPENDENCY_QUESTION: &str = "A task this one waits for ended";

pub(in crate::engine) fn status_word(status: TaskStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Undoes a split that could not create every part: removes each child
/// already created (its record, worktree and branch). None has a loop or
/// has been broadcast yet, so nothing else has seen them.
pub(in crate::engine) async fn discard_children(app: &Arc<App>, children: Vec<Task>) {
    for child in children {
        let _ = std::fs::remove_dir_all(app.store.task_dir(&child.id));
        let _ = tokio::task::spawn_blocking(move || {
            crate::delivery::git::recovery::discard_worktree(
                Path::new(&child.repo),
                Path::new(&child.worktree),
                &child.branch,
            )
        })
        .await;
    }
}

impl App {
    /// Parks `task` `waiting` on the question of what to do about the
    /// dependencies in `ended`.
    pub(in crate::engine) fn ask_dependency_question(
        &self,
        task: &Task,
        all: &[Task],
        ended: &[String],
    ) {
        let names: Vec<String> = all
            .iter()
            .filter(|t| ended.contains(&t.id))
            .map(|t| format!("\"{}\" ({})", t.title, status_word(t.status)))
            .collect();
        let mut task = task.clone();
        task.ask(Question::new(
            format!(
                "{DEPENDENCY_QUESTION}: {}. Retry it, drop it from what this task waits for, or stop?",
                names.join(", ")
            ),
            vec![
                "retry the dependency".into(),
                "drop the dependency".into(),
                "stop".into(),
            ],
            QuestionKind::DependencyEnded,
            AskedBy::Implement, crate::clock::now_ms()), now_ms());
        task.resume_phase(TaskStatus::Waiting, crate::clock::now_ms());
        task.touch(now_ms());
        if self.restart_environment_dependency(&task, all, ended) {
            return;
        }
        // The answer policy retries an ended dependency once before the owner
        // is asked.
        if let Some(question) = task.question.clone() {
            if !all
                .iter()
                .any(|t| ended.contains(&t.id) && t.queue.stopped_explicitly)
                && policy_open(self, &task, &question)
                && !task
                    .assumptions
                    .iter()
                    .any(|a| a.kind == Some(QuestionKind::DependencyEnded))
            {
                let answer = "retry the dependency";
                match self.answer_dependency_question(
                    task.clone(),
                    &crate::engine::questions::reply::Reply {
                        text: (answer).to_string(),
                        default_evidence: None,
                        like: None,
                    },
                    None,
                ) {
                    Ok(_) => return,
                    Err(e) => eprintln!(
                        "orch: answering dependency for task {} failed: {e}",
                        task.id
                    ),
                }
            }
        }
        if let Err(e) = self.update_task(&task.id, |fresh| {
            if let Some(question) = task.question.clone() {
                fresh.ask(question, now_ms());
            } else {
                fresh.clear_question();
            }
            fresh.resume_phase(TaskStatus::Waiting, crate::clock::now_ms());
        }) {
            eprintln!("orch: updating task {} failed: {e}", task.id);
        }
    }
}

impl App {
    /// A dependency that failed on the environment (a timeout, a flaky or
    /// outside-the-diff check, an install error) is restarted once more by
    /// itself, after a pause; dropping a dependency stays the owner's.
    /// `true` when this call handled `task` (restarted it or is waiting out
    /// the pause); `task` carries the dependency question.
    pub(in crate::engine) fn restart_environment_dependency(
        &self,
        task: &Task,
        all: &[Task],
        ended: &[String],
    ) -> bool {
        let deps: Vec<&Task> = all.iter().filter(|t| ended.contains(&t.id)).collect();
        if deps.is_empty()
            || task.loop_state.dependency_restarted
            || deps.iter().any(|d| {
                d.status != TaskStatus::Failed
                    || !ladder::agent_may_restart(d)
                    || d.queue.stopped_explicitly
                    || !ladder::failed_on_environment(d, &ladder::base_tails(self, &d.base_sha))
            })
        {
            return false;
        }
        let pause = ladder::dependency_restart_delay().as_millis() as i64;
        let left = pause - (now_ms() - deps.iter().map(|d| d.updated_at).max().unwrap_or(0));
        if left > 0 {
            // The graph comes back to this task when the pause is over.
            if tokio::runtime::Handle::try_current().is_ok() {
                let (app, repo) = (self.arc(), task.repo.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(left as u64 + 20)).await;
                    app.advance_graph(&repo);
                });
            }
            return true;
        }
        if let Err(e) = self.update_task(&task.id, |t| {
            t.mark_dependency_restarted();
            t.record_decision(ladder::DEPENDENCY_RESTART_LINE.to_string())
        }) {
            eprintln!("orch: updating task {} failed: {e}", task.id);
            return false;
        }
        match self.answer_dependency_question(
            task.clone(),
            &crate::engine::questions::reply::Reply {
                text: ("retry the dependency").to_string(),
                default_evidence: None,
                like: None,
            },
            Some(AnsweredBy::Policy),
        ) {
            Ok(_) => true,
            Err(e) => {
                eprintln!(
                    "orch: restarting dependency of task {} failed: {e}",
                    task.id
                );
                false
            }
        }
    }
}

impl App {
    /// Stops every child of `parent_id` that has not finished: a live loop
    /// is cancelled (it marks itself stopped), an idle one is marked here.
    pub(in crate::engine) fn stop_children(&self, parent_id: &str, repo: &str) {
        for mut child in self.repo_tasks(repo) {
            if child.parent.as_deref() != Some(parent_id)
                || matches!(
                    child.status,
                    TaskStatus::Done | TaskStatus::Failed | TaskStatus::Stopped
                )
            {
                continue;
            }
            self.close_stopped_task(&mut child);
            let _ = self.interrupt_task(
                &mut child,
                None,
                true,
                "Orchestrator: stopped with its parent".into(),
            );
            self.stop_children(&child.id, repo);
        }
    }
}

impl App {
    /// "retry" restarts every task this one waits for that ended
    /// `failed`/`stopped`; "drop" stops waiting for them (a dependency leaves
    /// `dependsOn`, a child is detached from this parent). Either way this
    /// task goes back to waiting for the rest.
    pub(in crate::engine) fn answer_dependency_question(
        &self,
        task: Task,
        reply: &crate::engine::questions::reply::Reply,
        caller: Option<AnsweredBy>,
    ) -> Result<serde_json::Value, String> {
        let answer = reply.text.as_str();
        let default_evidence = reply.default_evidence.as_deref();
        let like = reply.like.as_deref();
        let all = self.repo_tasks(&task.repo);
        let was_parent = is_parent(&task, &all);
        let ended: Vec<Task> = waits_for(&task, &all)
            .into_iter()
            .filter(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Stopped))
            .cloned()
            .collect();
        let by_owner = caller == Some(AnsweredBy::Owner);
        let choice = answer.trim().to_ascii_lowercase();
        if !choice.starts_with("retry") && !choice.starts_with("drop") {
            return Err("answer retry the dependency, drop the dependency, or stop".to_string());
        }
        if choice.starts_with("retry") {
            if !by_owner && ended.iter().any(|t| t.queue.stopped_explicitly) {
                return Err("an explicitly stopped dependency needs the owner's restart".into());
            }
            if !by_owner {
                let ids: Vec<String> = ended.iter().map(|t| t.id.clone()).collect();
                self.agent_restart_gate(&ids)?;
            }
            self.restart_ended_dependencies(&ended, &all);
        } else if choice.starts_with("drop") {
            for child in ended
                .iter()
                .filter(|t| t.parent.as_deref() == Some(task.id.as_str()))
            {
                self.update_task(&child.id, |child| {
                    child.detach_parent();
                    child.record_decision(format!(
                        "{}: dropped from \"{}\"",
                        caller.filter(|_| default_evidence.is_none()).map_or(
                            "Policy",
                            |by| match by {
                                AnsweredBy::Owner => "Owner",
                                AnsweredBy::Lead => "Lead",
                                AnsweredBy::Overseer | AnsweredBy::Orchestrator => "Orchestrator",
                                _ => "Policy",
                            }
                        ),
                        task.title
                    ));
                })?;
            }
        }
        let fresh = self.repo_tasks(&task.repo);
        let task = self.update_task(&task.id, |current| {
            if choice.starts_with("drop") {
                current.forget_dependencies(&ended.iter().map(|t| t.id.clone()).collect::<Vec<_>>());
            }
            let question = current.question.clone();
            if let Some(caller) = caller {
                record_answer_for(current, question.as_ref(), &crate::engine::questions::reply::Reply { text: (answer.trim()).to_string(), default_evidence: (default_evidence).map(str::to_string), like: (like).map(str::to_string) }, caller);
            } else if let Some(question) = task.question.as_ref() {
                record_policy_answer(
                    current, question, answer, "policy",
                    "a dependency that ended is retried once", 0,
                );
            }
            current.clear_question();
            if was_parent && !is_parent(current, &fresh) {
                current.record_decision(
                    "Orchestrator: no subtasks are left, so this task was stopped instead of implementing the whole request itself".to_string(),
                );
                current.resume_phase(TaskStatus::Stopped, crate::clock::now_ms());
                self.close_stopped_task(current);
            } else {
                current.resume_phase(if is_parent(current, &fresh) {
                    TaskStatus::Running
                } else {
                    TaskStatus::Queued
                }, crate::clock::now_ms());
            }
        })?;
        if task.status == TaskStatus::Stopped {
            return serde_json::to_value(&task).map_err(|e| e.to_string());
        }
        // Its loop waits for whatever is still pending, or goes ahead.
        self.spawn_task_loop(task.id.clone(), true);
        let latest = self
            .store
            .load_task(&task.id)
            .map_err(|e| e.to_string())?
            .unwrap_or(task);
        serde_json::to_value(&latest).map_err(|e| e.to_string())
    }
}

impl App {
    fn restart_ended_dependencies(&self, ended: &[Task], all: &[Task]) {
        for dep in ended.iter().filter(|t| !t.archived) {
            let mut cancellations = self.cancellations.lock().unwrap();
            if let Some(end) = cancellations.get_mut(&dep.id) {
                end.restart = true;
                end.restart_children = is_parent(dep, all);
            }
            // Queued before its loop runs, so nothing reads it as
            // ended again while it retries.
            // From the stored task: the restart gate just wrote its record there.
            let restart_children = is_parent(dep, all);
            let Ok(dep) = self.update_task(&dep.id, |d| {
                d.queue_child_restart(restart_children, now_ms());
            }) else {
                continue;
            };
            drop(cancellations);
            self.spawn_task_loop(dep.id.clone(), true);
        }
    }
}
