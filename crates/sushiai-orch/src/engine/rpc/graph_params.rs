use crate::api::validation::validate_task_id;
use crate::engine::decision::implement_attempt_count;
use crate::engine::state::App;
use crate::task::graph::has_cycle;
use crate::task::graph::is_parent;
use crate::task::graph::wait_edges;
use crate::task::graph::waits_state;
use crate::task::graph::Waits;
use crate::task::Task;
use crate::task::TaskStatus;

impl App {
    /// `task.create`/`task.amend`'s graph: every id a task of this repo,
    /// the parent a top-level task that has not implemented anything itself
    /// and is not done, and no dependency cycle once the new task is added
    /// (a parent waits for its children, so a child depending on its own
    /// parent is one). Returns the deduplicated dependency records and parent.
    pub(in crate::engine) fn check_graph_params(
        &self,
        repo: &str,
        depends_on: &[String],
        parent: Option<&str>,
        existing: Option<&Task>,
    ) -> Result<(Vec<Task>, Option<Task>), String> {
        let mut deps: Vec<Task> = Vec::new();
        for id in depends_on {
            let dependency = self.load_graph_task(repo, id, "dependency")?;
            if existing.is_some_and(|t| dependency.parent.as_deref() == Some(t.id.as_str())) {
                return Err("a task cannot depend on its own child".to_string());
            }
            if !deps.iter().any(|t| t.id == *id) {
                deps.push(dependency);
            }
        }
        let parent = self.graph_parent(repo, parent)?;
        let new_id = "(new task)".to_string();
        let mut all = self.repo_tasks(repo);
        for task in &mut all {
            if let Some(dependencies) = task
                .pending_amendment
                .as_ref()
                .and_then(|pending| pending.depends_on.clone())
            {
                task.record_dependencies(dependencies);
            }
        }
        let mut edges = wait_edges(&all);
        let mut waits: Vec<String> = deps.iter().map(|t| t.id.clone()).collect();
        if let Some(task) = existing {
            waits.extend(
                all.iter()
                    .filter(|t| t.parent.as_deref() == Some(task.id.as_str()))
                    .map(|t| t.id.clone()),
            );
            edges.insert(task.id.clone(), waits);
        } else {
            edges.insert(new_id.clone(), waits);
            if let Some(parent) = &parent {
                edges.entry(parent.id.clone()).or_default().push(new_id);
            }
        }
        if has_cycle(&edges) {
            return Err("dependsOn would make a dependency cycle".to_string());
        }
        Ok((deps, parent))
    }
}

impl App {
    fn load_graph_task(&self, repo: &str, id: &str, what: &str) -> Result<Task, String> {
        validate_task_id(&self.store, id).map_err(|_| format!("unknown {what} {id}"))?;
        let task = self
            .store
            .load_task(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("unknown {what} {id}"))?;
        if task.repo != repo {
            return Err(format!("{what} {id} belongs to another repository"));
        }
        Ok(task)
    }
    fn graph_parent(&self, repo: &str, parent: Option<&str>) -> Result<Option<Task>, String> {
        Ok(match parent.filter(|p| !p.trim().is_empty()) {
            None => None,
            Some(id) => {
                let parent = self.load_graph_task(repo, id, "parent")?;
                if parent.parent.is_some() {
                    return Err("a subtask cannot have subtasks of its own".to_string());
                }
                if parent.status == TaskStatus::Done {
                    return Err("the parent task is already done".to_string());
                }
                if implement_attempt_count(&parent) > 0 {
                    return Err(
                        "the parent task has already run an implement attempt of its own"
                            .to_string(),
                    );
                }
                // A live loop may already be queued for a slot to implement
                // the whole request; only a drafting one re-checks after
                // planning.
                if parent.status == TaskStatus::Landing {
                    return Err("the parent task is already landing".to_string());
                }
                // A parent with parts already runs no implement attempt of its
                // own, so a live loop may take one more part. A loop still
                // short of its dependencies is about to park without
                // implementing, so it may take its first part too.
                let all = self.repo_tasks(repo);
                let held_by_dependencies = !parent.depends_on.is_empty()
                    && !matches!(waits_state(&parent, &all), Waits::Ready | Waits::Nothing);
                if parent.status != TaskStatus::Drafting
                    && self.controls.lock().unwrap().contains_key(&parent.id)
                    && !is_parent(&parent, &all)
                    && !held_by_dependencies
                {
                    return Err(
                        "the parent task is already running; create it with start: false, add its subtasks, then start it"
                            .to_string(),
                    );
                }
                Some(parent)
            }
        })
    }
}
