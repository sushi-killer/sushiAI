use crate::clock::now_ms;
use crate::engine::graph::parent::{run_parent, ParentEnd};
use crate::engine::leases::pass_lease_gate;
use crate::engine::planning::records::needs_planning;
use crate::engine::recovery::resumable_attempt;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::task::attempts::completed_implement_count;
use crate::task::graph::{is_parent, own_waits_state, unmet_titles, waits_state, Waits};
use crate::task::{Task, TaskStatus, Wait};
use std::sync::Arc;

pub(in crate::engine) enum Admission {
    Ready { carry_conflict: Option<String> },
    Waiting,
    Parent(ParentEnd),
}

pub(in crate::engine) async fn graph(
    app: &Arc<App>,
    task: &mut Task,
    signals: &TaskSignals,
    parent_failure: Option<&str>,
) -> Admission {
    if needs_planning(task) && task.parent.is_none() {
        return Admission::Ready {
            carry_conflict: None,
        };
    }
    let all = app.repo_tasks(&task.repo);
    let parent = is_parent(task, &all);
    let carry_conflict =
        (parent_failure.is_none() && !task.queue.carry_conflicts.is_empty() && parent).then(|| {
            format!(
                "The carry onto the newest base left conflicts in {}.",
                task.queue.carry_conflicts.join(", ")
            )
        });
    let failure = parent_failure.or(carry_conflict.as_deref());
    if parent
        && !task.pr_check_repair_due()
        && failure.is_none()
        && (completed_implement_count(task) == 0
            || !matches!(waits_state(task, &all), Waits::Ready | Waits::Nothing))
    {
        return Admission::Parent(run_parent(app, &task.id, signals).await);
    }
    if parent_blocks(app, task, &all)
        || (!task.depends_on.is_empty()
            && !matches!(waits_state(task, &all), Waits::Ready | Waits::Nothing))
    {
        queue_dependency(app, task, &all);
        return Admission::Waiting;
    }
    if task.status == TaskStatus::Drafting && task.planned_by_parent && task.parent.is_some() {
        let expected = crate::task::checks::snapshot::CheckSnapshot::capture(task);
        match app.update_task_checked(&task.id, |fresh| {
            if signals.cancel.is_cancelled()
                || fresh.archived
                || fresh.queue.park.is_some()
                || fresh.status != TaskStatus::Drafting
                || !fresh.planned_by_parent
                || fresh.parent.is_none()
                || !expected.matches(fresh)
            {
                return Err("the parent-drafted child changed before admission".into());
            }
            fresh.resume_phase(TaskStatus::Queued, now_ms());
            Ok(())
        }) {
            Ok(admitted) => *task = admitted,
            Err(_) => return Admission::Waiting,
        }
    }
    if !needs_planning(task) {
        let force = task.status == TaskStatus::Landing
            || completed_implement_count(task) > 0
            || resumable_attempt(app, task).is_some()
            || failure.is_some();
        if !pass_lease_gate(app, task, force).await {
            return Admission::Waiting;
        }
    }
    Admission::Ready { carry_conflict }
}

fn parent_blocks(app: &App, task: &Task, all: &[Task]) -> bool {
    task.parent
        .as_deref()
        .and_then(|id| all.iter().find(|parent| parent.id == id))
        .is_some_and(|parent| {
            !matches!(own_waits_state(parent, all), Waits::Ready | Waits::Nothing)
                || parent.queue.backlog.is_some()
                || app.is_suspended(parent)
                || matches!(parent.status, TaskStatus::Stopped | TaskStatus::Failed)
                || (parent.status == TaskStatus::Waiting && completed_implement_count(task) == 0)
        })
}

fn queue_dependency(app: &App, task: &mut Task, all: &[Task]) {
    let wait = Wait::Dependency {
        titles: unmet_titles(task, all),
    };
    if task.status != TaskStatus::Queued || task.queue.wait.as_ref() != Some(&wait) {
        task.resume_phase(TaskStatus::Queued, task.updated_at);
        task.set_wait(Some(wait));
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
}
