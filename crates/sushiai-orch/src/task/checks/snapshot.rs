use crate::task::{Check, PlannedTest, SourcedCheck, TaskData};

/// Content identity for asynchronous check work. Baselines are observations,
/// so completing another baseline does not itself invalidate this identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckSnapshot {
    task_id: String,
    repo: String,
    base: String,
    goal: String,
    request: Option<String>,
    paths: Vec<String>,
    dependencies: Vec<String>,
    files: Vec<PlannedTest>,
    criteria: Vec<String>,
    visual: Vec<String>,
    verify: Vec<String>,
    final_verify: Vec<String>,
    screenshot: Option<String>,
    targeted: Option<String>,
    checklist: Vec<(String, String)>,
    non_gating: Vec<String>,
    checks: Vec<(usize, String)>,
    held_out: Option<(usize, String)>,
    sources: Vec<SourcedCheck>,
}

impl CheckSnapshot {
    pub(crate) fn capture(task: &TaskData) -> Self {
        let key = |check: &Check| (check.criterion, check.run.clone());
        Self {
            task_id: task.id.clone(),
            repo: task.repo.clone(),
            base: task.base_sha.clone(),
            goal: task.goal.clone(),
            request: task.request.clone(),
            paths: task.paths.clone(),
            dependencies: task.depends_on.clone(),
            files: task.planner_tests.files.clone(),
            criteria: task.criteria.clone(),
            visual: task.visual_criteria.clone(),
            verify: task.verify.clone(),
            final_verify: task.final_verify.clone(),
            screenshot: task.brief_check.screenshot.clone(),
            targeted: task.brief_check.check.clone(),
            checklist: task
                .checklist
                .iter()
                .map(|item| (item.text.clone(), item.check.clone()))
                .collect(),
            non_gating: task.brief_check.non_gating.clone(),
            checks: task.checks.iter().map(key).collect(),
            held_out: task.held_out.as_ref().map(key),
            sources: task.planner_tests.sources.clone(),
        }
    }

    pub(crate) fn same_contract_after_carry(&self, task: &TaskData) -> bool {
        let mut carried = Self::capture(task);
        carried.base.clone_from(&self.base);
        *self == carried
    }

    pub(crate) fn matches(&self, task: &TaskData) -> bool {
        *self == Self::capture(task)
    }
}

/// A positional request resolved against the task as the caller saw it.
/// A later replay cannot interpret the same index as the next criterion.
#[derive(Debug, Clone)]
pub(crate) struct CriterionEdit {
    pub(super) snapshot: CheckSnapshot,
    pub(super) index: usize,
}

impl CriterionEdit {
    pub(crate) fn capture(task: &TaskData, index: usize) -> Option<Self> {
        (index < task.criteria.len()).then(|| Self {
            snapshot: CheckSnapshot::capture(task),
            index,
        })
    }
}
