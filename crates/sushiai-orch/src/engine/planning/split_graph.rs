use crate::engine::plan_gate;
use crate::engine::planning::draft::PlanSubtask;
use crate::engine::planning::graph::subtask_request;
use crate::engine::planning::subtasks::SubtaskPlan;
use crate::policy::delivery::BaseFrom;
use crate::prompts::clip::truncate_chars;
use crate::task::creation::NewTask;
use crate::task::{valid_check, valid_checks, Check, ChecklistItem, Task};
use crate::verify::options::split_verify_commands;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
/// What a standalone part brings to its child: its own draft.
#[derive(Default)]
struct Draft<'a> {
    goal: String,
    criteria: Vec<String>,
    verify: Vec<String>,
    final_verify: Vec<String>,
    checks: Vec<Check>,
    held_out: Option<Check>,
    complete: bool,
    part: Option<&'a crate::engine::planning::draft::PlanSubtask>,
}

/// One child of a split: the planner's parts that stand alone, and the one
/// merged group (if any) with its ordered checklist.
struct Unit {
    /// Indexes into the parts, in group order.
    members: Vec<usize>,
    merged: bool,
}

pub(in crate::engine::planning) struct PreparedChild<'a> {
    pub(in crate::engine::planning) task: NewTask,
    pub(in crate::engine::planning) part: Option<&'a PlanSubtask>,
    pub(in crate::engine::planning) complete: bool,
}

pub(in crate::engine::planning) struct SplitGraph<'a> {
    plan: &'a SubtaskPlan,
    units: Vec<Unit>,
    ids: Vec<String>,
    unit_of_key: HashMap<&'a str, usize>,
}

impl<'a> SplitGraph<'a> {
    pub(in crate::engine::planning) fn new(plan: &'a SubtaskPlan) -> Self {
        let mut units = Vec::new();
        for i in 0..plan.parts.len() {
            if plan.merge.merged.contains(&i) {
                if plan.merge.merged.iter().min() == Some(&i) {
                    units.push(Unit {
                        members: plan.merge.merged.clone(),
                        merged: true,
                    });
                }
            } else {
                units.push(Unit {
                    members: vec![i],
                    merged: false,
                });
            }
        }
        let ids = units
            .iter()
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect();
        let unit_of_key = units
            .iter()
            .enumerate()
            .flat_map(|(i, unit)| {
                unit.members
                    .iter()
                    .map(move |&member| (plan.parts[member].key.trim(), i))
            })
            .collect();
        Self {
            plan,
            units,
            ids,
            unit_of_key,
        }
    }
    pub(in crate::engine::planning) fn len(&self) -> usize {
        self.units.len()
    }
    fn title(&self, index: usize) -> String {
        let unit = &self.units[index];
        let part = &self.plan.parts[unit.members[0]];
        let first = if part.title.trim().is_empty() {
            truncate_chars(part.request.trim(), 60)
        } else {
            part.title.trim().to_string()
        };
        if unit.members.len() > 1 {
            format!("{first} (+{} more)", unit.members.len() - 1)
        } else {
            first
        }
    }
    fn dependencies(&self, index: usize) -> Vec<String> {
        let mut dependencies = Vec::new();
        for &member in &self.units[index].members {
            for key in &self.plan.parts[member].depends_on {
                let id = self.ids[self.unit_of_key[key.trim()]].clone();
                if id != self.ids[index] && !dependencies.contains(&id) {
                    dependencies.push(id);
                }
            }
        }
        dependencies
    }
    pub(in crate::engine::planning) fn display_title(&self, index: usize) -> String {
        let title = self.title(index);
        let dependencies = self.dependencies(index);
        if dependencies.is_empty() {
            title
        } else {
            let after = dependencies
                .iter()
                .filter_map(|id| self.ids.iter().position(|unit| unit == id))
                .map(|at| self.title(at))
                .collect::<Vec<_>>();
            format!("{title} (after {})", after.join(", "))
        }
    }
    pub(in crate::engine::planning) fn request(
        &self,
        parent: &Task,
        i: usize,
        review_on: bool,
        now: i64,
    ) -> PreparedChild<'a> {
        let unit = &self.units[i];
        let parts = self.plan.parts.as_slice();
        let title = self.title(i);
        let depends_on = self.dependencies(i);
        let (drafted, request, checklist, paths) =
            unit_draft(parts, unit, review_on, Path::new(&parent.worktree));
        let relay_of = self.plan.links[unit.members[0]].and_then(|at| {
            let id = &self.ids[self.unit_of_key[parts[at].key.trim()]];
            (id != &self.ids[i]).then(|| id.clone())
        });
        let task = NewTask {
            backlog: None,
            id: self.ids[i].clone(),
            repo_root: PathBuf::from(&parent.repo.clone()),
            project_id: parent.project_id.clone(),
            title: title.clone(),
            goal: drafted.goal,
            criteria: drafted.criteria,
            verify: drafted.verify,
            final_verify: drafted.final_verify,
            checks: drafted.checks,
            held_out: drafted.held_out,
            request: Some(subtask_request(parent, &request)),
            branch: None,
            base: parent.branch.clone(),
            base_from: BaseFrom::Plain,
            variant: parent.variant(),
            depends_on: depends_on.clone(),
            parent: Some(parent.id.clone()),
            paths,
            checklist,
            relay_of,
            // Eval reports count the parent, which carries the children's
            // cost.
            eval_set: None,
            eval_name: None,
            eval_check_cmd: None,
            source: Some("planner".to_string()),
            // Keeps the planner's order when children are listed by age.
            created_at: now + i as i64,
        };
        PreparedChild {
            task,
            part: drafted.part,
            complete: drafted.complete,
        }
    }
}

fn unit_draft<'a>(
    parts: &'a [PlanSubtask],
    unit: &Unit,
    review_on: bool,
    parent_worktree: &Path,
) -> (Draft<'a>, String, Vec<ChecklistItem>, Vec<String>) {
    let non_empty = |list: &[String]| -> Vec<String> {
        list.iter()
            .filter(|c| !c.trim().is_empty())
            .cloned()
            .collect()
    };
    // A merged group is drafted by its own planner; a standalone part
    // carries the draft it came with.
    let mut drafted = Draft::default();
    let (request, checklist, paths) = if unit.merged {
        let text = unit
            .members
            .iter()
            .map(|&m| parts[m].request.trim())
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut paths: Vec<String> = Vec::new();
        for &m in &unit.members {
            for p in &parts[m].paths {
                if !paths.contains(p) {
                    paths.push(p.clone());
                }
            }
        }
        (text, plan_gate::checklist_of(parts, &unit.members), paths)
    } else {
        let part = &parts[unit.members[0]];
        let mut drafted_verify = non_empty(&part.verify);
        // A complete draft leads with the part's targeted check, so the
        // subtask runs that check (the full CI entry is the parent's).
        let check = part.check.trim();
        if part.is_complete()
            && !check.is_empty()
            && !drafted_verify.iter().any(|c| c.trim() == check)
        {
            drafted_verify.insert(0, check.to_string());
        }
        let (verify, judged) = if review_on {
            split_verify_commands(parent_worktree, &drafted_verify)
        } else {
            (drafted_verify, Vec::new())
        };
        let mut criteria = non_empty(&part.criteria);
        // Indices count the part's own criteria, before the review entries.
        let own_criteria = criteria.len();
        drafted.checks = valid_checks(part.checks.clone(), own_criteria);
        drafted.held_out = valid_check(part.held_out.clone(), own_criteria);
        criteria.extend(
            judged
                .into_iter()
                .map(|v| format!("Checked by review (not a shell command): {v}")),
        );
        drafted.goal = part.goal.trim().to_string();
        drafted.complete = part.is_complete() && !verify.is_empty();
        drafted.criteria = criteria;
        drafted.verify = verify;
        drafted.final_verify = non_empty(&part.final_verify);
        drafted.part = Some(part);
        (part.request.trim().to_string(), vec![], part.paths.clone())
    };
    (drafted, request, checklist, paths)
}
