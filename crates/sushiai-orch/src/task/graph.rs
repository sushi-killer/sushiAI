use crate::task::{Task, TaskStatus};
use std::collections::HashMap;

impl Task {
    pub(crate) fn record_restart_children(&mut self, restart: bool) {
        self.data.queue.restart_children = restart;
    }
    pub(crate) fn discard_relay(&mut self) {
        self.data.queue.relay_of = None;
    }
    pub(crate) fn record_relay_base(&mut self, base: Option<String>) {
        self.data.queue.relay_base = base;
    }
    pub(crate) fn detach_parent(&mut self) {
        self.data.parent = None;
    }
    pub(crate) fn forget_dependencies(&mut self, ended: &[String]) {
        self.data.depends_on.retain(|id| !ended.contains(id));
    }
    pub(crate) fn record_dependencies(&mut self, dependencies: Vec<String>) {
        self.data.depends_on = dependencies;
    }
    pub(crate) fn mark_dependency_restarted(&mut self) {
        self.data.loop_state.dependency_restarted = true;
    }
    pub(crate) fn reconcile_children_cost(&mut self, total: f64) {
        self.data.cost_usd = total;
    }
    pub(crate) fn record_final_check_failures(
        &mut self,
        commands: impl IntoIterator<Item = String>,
    ) {
        self.data.loop_state.final_check_failures.extend(commands);
    }
    pub(crate) fn queue_child_restart(&mut self, restart_children: bool, now: i64) {
        self.data.queue.restart_children = restart_children;
        self.data.queue.stopped_explicitly = false;
        self.data.queue.park = None;
        self.data.question = None;
        self.data.status = TaskStatus::Queued;
        self.data.updated_at = now;
    }
}

/// Whether the wait-for graph has a cycle; `edges[a]` lists what `a` waits
/// for.
pub(crate) fn has_cycle(edges: &HashMap<String, Vec<String>>) -> bool {
    fn visit<'a>(
        node: &'a str,
        edges: &'a HashMap<String, Vec<String>>,
        state: &mut HashMap<&'a str, bool>,
    ) -> bool {
        match state.get(node) {
            Some(true) => return true,
            Some(false) => return false,
            None => {}
        }
        state.insert(node, true);
        for next in edges.get(node).into_iter().flatten() {
            if visit(next, edges, state) {
                return true;
            }
        }
        state.insert(node, false);
        false
    }
    // `true` = on the current path, `false` = finished without a cycle.
    let mut state: HashMap<&str, bool> = HashMap::new();
    edges.keys().any(|n| visit(n, edges, &mut state))
}

/// A task waits for its dependencies; a parent also waits for each child.
pub(crate) fn wait_edges(tasks: &[Task]) -> HashMap<String, Vec<String>> {
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for t in tasks {
        edges
            .entry(t.id.clone())
            .or_default()
            .extend(t.depends_on.iter().cloned());
        if let Some(parent) = &t.parent {
            edges.entry(parent.clone()).or_default().push(t.id.clone());
        }
    }
    edges
}

/// What `task` waits for among `all` (its repo's tasks): the dependencies
/// that still exist, and for a parent its children that are not archived.
pub(crate) fn waits_for<'a>(task: &Task, all: &'a [Task]) -> Vec<&'a Task> {
    all.iter()
        .filter(|t| {
            task.depends_on.contains(&t.id)
                || (t.parent.as_deref() == Some(task.id.as_str()) && !t.archived)
        })
        .collect()
}

pub(crate) fn is_parent(task: &Task, all: &[Task]) -> bool {
    all.iter()
        .any(|t| t.parent.as_deref() == Some(task.id.as_str()))
}

#[derive(Debug, PartialEq)]
pub(crate) enum Waits {
    /// Waits for nothing.
    Nothing,
    Ready,
    Pending,
    /// These ended `failed` or `stopped` and will not finish on their own.
    Ended(Vec<String>),
}

pub(crate) fn waits_state(task: &Task, all: &[Task]) -> Waits {
    state_of(waits_for(task, all))
}

/// Like `waits_state` but only for the task's own `dependsOn`, not for its
/// children: what a parent must wait for before its children may start.
pub(crate) fn own_waits_state(task: &Task, all: &[Task]) -> Waits {
    state_of(
        all.iter()
            .filter(|t| task.depends_on.contains(&t.id))
            .collect(),
    )
}

pub(crate) fn state_of(waits: Vec<&Task>) -> Waits {
    if waits.is_empty() {
        return Waits::Nothing;
    }
    let ended: Vec<String> = waits
        .iter()
        .filter(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Stopped))
        .map(|t| t.id.clone())
        .collect();
    if !ended.is_empty() {
        Waits::Ended(ended)
    } else if waits.iter().all(|t| t.status == TaskStatus::Done) {
        Waits::Ready
    } else {
        Waits::Pending
    }
}

/// Titles of what a queued task waits for, for its `dependency` wait: what
/// it depends on that is not done, else what its parent waits for.
pub(crate) fn unmet_titles(task: &Task, all: &[Task]) -> Vec<String> {
    let unmet = |of: &Task| -> Vec<String> {
        waits_for(of, all)
            .iter()
            .filter(|t| t.status != TaskStatus::Done)
            .map(|t| t.title.clone())
            .collect()
    };
    let mut titles = unmet(task);
    if titles.is_empty() {
        if let Some(parent) = task
            .parent
            .as_deref()
            .and_then(|pid| all.iter().find(|t| t.id == pid))
        {
            titles = unmet(parent);
            if titles.is_empty() {
                titles.push(parent.title.clone());
            }
        }
    }
    titles
}

/// A parent's cost: its own runs plus every child's total.
pub(crate) fn parent_cost(parent: &Task, all: &[Task]) -> f64 {
    let own: f64 = parent
        .attempts
        .iter()
        .map(|a| a.cost_usd.unwrap_or(0.0) + a.review_cost_usd.unwrap_or(0.0))
        .sum();
    own + all
        .iter()
        .filter(|t| t.parent.as_deref() == Some(parent.id.as_str()))
        .map(|t| t.cost_usd)
        .sum::<f64>()
}

pub(crate) mod landing;
