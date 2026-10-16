use crate::clock::now_ms;
use crate::engine::graph::dependencies::discard_children;
use crate::engine::plan_gate;
use crate::engine::planning::graph::relay_links;
use crate::engine::planning::graph::serialise_overlapping;
use crate::engine::planning::split_graph::{PreparedChild, SplitGraph};
use crate::engine::protected::paths::keep_planned_tests;
use crate::engine::state::App;
use crate::task::Task;
use crate::task::TaskStatus;
use crate::verify::repository::record_plan_check_sources;
use std::path::PathBuf;
use std::sync::Arc;

pub(in crate::engine) struct SubtaskPlan {
    pub(in crate::engine) parts: Vec<crate::engine::planning::draft::PlanSubtask>,
    pub(in crate::engine) links: Vec<Option<usize>>,
    decisions: Vec<String>,
    pub(in crate::engine) merge: plan_gate::grouping::MergePlan,
}

/// All dependency edges are present before grouping can contract the graph.
pub(in crate::engine) fn prepare_subtasks(
    parts: &[crate::engine::planning::draft::PlanSubtask],
) -> SubtaskPlan {
    let (mut parts, mut decisions) = serialise_overlapping(parts);
    let (links, relayed) = relay_links(&mut parts);
    decisions.extend(relayed);
    let merge = plan_gate::grouping::merge_parts(&parts);
    SubtaskPlan {
        parts,
        links,
        decisions,
        merge,
    }
}

/// The whole plan is one merged group: the task keeps its draft and gains
/// the ordered checklist. `Err("")` tells the caller to carry on as one task.
pub(in crate::engine) fn merge_into_checklist(
    app: &Arc<App>,
    task_id: &str,
    plan: &SubtaskPlan,
) -> Result<(), String> {
    let mut task = app
        .store
        .load_task(task_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    task.record_checklist(plan_gate::grouping::checklist_of(
        &plan.parts,
        &plan.merge.merged,
    ));
    task.record_decisions(plan.decisions.iter().cloned());
    task.record_decision(format!(
        "Plan gate: merged {} subtasks into one task with a checklist",
        plan.merge.merged.len()
    ));
    task.touch(now_ms());
    app.store.save_task(&task).map_err(|e| e.to_string())?;
    app.broadcast_task(&task);
    Ok(())
}

pub(in crate::engine) async fn split_into_subtasks(
    app: &Arc<App>,
    parent_id: &str,
    plan: &SubtaskPlan,
    start: bool,
) -> Result<(), String> {
    let mut parent = app
        .store
        .load_task(parent_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    let graph = SplitGraph::new(plan);
    let parent_mcp = std::fs::read_to_string(app.store.task_dir(parent_id).join("mcp.json")).ok();
    let now = now_ms();
    let review_on = !app.settings.read().unwrap().review.is_empty();
    let mut titles = Vec::new();
    let mut children = Vec::new();
    for index in 0..graph.len() {
        let child = match create_child(app, graph.request(&parent, index, review_on, now)).await {
            Ok(child) => child,
            Err(error) => {
                discard_children(app, children).await;
                return Err(error);
            }
        };
        if let Err(error) = app.store.save_task(&child) {
            children.push(child);
            discard_children(app, children).await;
            return Err(error.to_string());
        }
        if let Some(mcp) = &parent_mcp {
            let _ = std::fs::write(app.store.task_dir(&child.id).join("mcp.json"), mcp);
        }
        titles.push(graph.display_title(index));
        children.push(child);
    }
    if let Err(error) = finish_split(app, &mut parent, plan, &titles, start) {
        discard_children(app, children).await;
        return Err(error);
    }
    for child in &children {
        app.broadcast_task(child);
    }
    app.broadcast_task(&parent);
    Ok(())
}

async fn create_child(app: &Arc<App>, request: PreparedChild<'_>) -> Result<Task, String> {
    let PreparedChild {
        task: new,
        part,
        complete,
    } = request;
    let mut child = app.create_task_record(new).await?;
    if let Some(part) = part {
        let tests = {
            let (tests, dropped) = keep_planned_tests(&part.tests);
            child.record_decisions(dropped);
            Some(tests)
        };
        child.record_child_plan(crate::task::planning::operations::ChildPlan {
            tests,
            tier: part.tier,
            visual_criteria: part.visual_criteria.clone(),
            screenshot: part
                .screenshot
                .as_deref()
                .map(str::trim)
                .filter(|command| !command.is_empty())
                .map(str::to_string),
            check: part.check.trim().to_string(),
            complete,
        });
        if complete {
            let repo = PathBuf::from(&child.repo);
            record_plan_check_sources(&mut child, &repo);
        }
    }

    Ok(child)
}

fn finish_split(
    app: &Arc<App>,
    parent: &mut Task,
    plan: &SubtaskPlan,
    titles: &[String],
    start: bool,
) -> Result<(), String> {
    parent.record_decisions(plan.decisions.iter().cloned());
    if plan.merge.merged.len() > 1 {
        parent.record_decision(format!(
            "Plan gate: merged {} subtasks into one task with a checklist",
            plan.merge.merged.len()
        ));
    }
    parent.record_decision(format!(
        "Planner: split into {} subtasks: {}",
        titles.len(),
        titles.join("; ")
    ));
    parent.resume_phase(
        if start {
            TaskStatus::Running
        } else {
            TaskStatus::Stopped
        },
        now_ms(),
    );
    if parent.status == TaskStatus::Stopped {
        app.close_stopped_task(parent);
    }
    parent.touch(now_ms());
    app.store
        .save_task(parent)
        .map_err(|error| error.to_string())
}
