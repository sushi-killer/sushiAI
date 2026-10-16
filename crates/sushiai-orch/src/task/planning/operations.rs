use crate::policy::routing::Tier;
use crate::policy::variant::Variant;
use crate::task::{ChecklistItem, MissingTool, PlanFinding, PlannedTest, Task, TaskMetrics};

pub(crate) struct PlannedBrief {
    pub(crate) title: String,
    pub(crate) goal: String,
    pub(crate) tier: Option<Tier>,
    pub(crate) paths: Vec<String>,
    pub(crate) screenshot: Option<String>,
}

pub(crate) struct ChildPlan {
    pub(crate) tests: Option<Vec<PlannedTest>>,
    pub(crate) tier: Option<Tier>,
    pub(crate) visual_criteria: Vec<String>,
    pub(crate) screenshot: Option<String>,
    pub(crate) check: String,
    pub(crate) complete: bool,
}

impl Task {
    pub(crate) fn apply_variant(&mut self, variant: Variant) {
        self.data.variant = Some(variant);
    }
    pub(crate) fn record_planned_brief(&mut self, brief: PlannedBrief) {
        self.data.title = brief.title;
        self.data.goal = brief.goal;
        self.data.planned_tier = brief.tier;
        if self.data.paths.is_empty() {
            self.data.paths = brief.paths;
        }
        self.data.brief_check.screenshot = brief.screenshot;
    }
    pub(crate) fn record_child_plan(&mut self, plan: ChildPlan) {
        if let Some(tests) = plan.tests {
            super::super::checks::edits::replace_planner_files(self, tests);
        }
        self.data.planned_tier = plan.tier;
        self.data.visual_criteria = plan.visual_criteria;
        self.data.brief_check.screenshot = plan.screenshot;
        self.data.brief_check.check = (!plan.check.is_empty()).then_some(plan.check);
        self.data.planned_by_parent = plan.complete;
    }
    pub(crate) fn record_screenshot_command(&mut self, command: Option<String>) {
        self.data.brief_check.screenshot = command;
    }
    pub(crate) fn record_planned_summary(&mut self, index: usize, title: &str) {
        self.data.attempts[index].summary = Some(format!("Drafted: {title}"));
    }
    pub(crate) fn record_request(&mut self, request: String) {
        self.data.request = Some(request);
    }
    pub(crate) fn record_paths(&mut self, paths: Vec<String>) {
        self.data.paths = paths;
    }
    pub(crate) fn record_checklist(&mut self, checklist: Vec<ChecklistItem>) {
        self.data.checklist = checklist;
    }
    pub(crate) fn record_checklist_result(&mut self, index: usize, passes: bool) {
        self.data.checklist[index].passes = passes;
    }
    pub(crate) fn record_metrics(&mut self, metrics: Option<TaskMetrics>) {
        self.data.metrics = metrics;
    }
    pub(crate) fn mark_plan_reviewed(&mut self) {
        self.data.plan_gate.reviewed = true;
    }
    pub(crate) fn record_plan_findings(&mut self, findings: Vec<PlanFinding>) {
        self.data.plan_gate.findings = findings;
    }
    pub(crate) fn record_brief_conflict(&mut self, conflict: String) {
        self.data.brief_check.conflict = Some(conflict);
    }
    pub(crate) fn record_missing_tool(&mut self, missing: MissingTool) {
        self.data.brief_check.missing_tool = Some(missing);
    }
    pub(crate) fn finish_brief_check(&mut self, done: bool) {
        self.data.brief_check.done = done;
    }
    pub(crate) fn mark_dependency_refresh(&mut self) {
        self.data.brief_check.refreshed = true;
    }
    pub(crate) fn mark_feasibility_done(&mut self) {
        self.data.brief_check.feasibility_done = true;
    }
    pub(crate) fn record_non_gating_check(&mut self, command: String) {
        if !self.data.brief_check.non_gating.contains(&command) {
            self.data.brief_check.non_gating.push(command);
        }
    }
    pub(crate) fn mark_criterion_judged(&mut self, criterion: String) {
        self.data.brief_check.judged.push(criterion);
    }
    pub(crate) fn record_answer_progress(&mut self, seen: usize) {
        self.data.brief_check.answers_seen = seen;
    }
}

impl Task {
    pub(crate) fn mark_replanned(&mut self) {
        self.data.plan_gate.replanned = true;
    }
    pub(crate) fn clear_missing_tool(&mut self) {
        self.data.brief_check.missing_tool = None;
    }
    pub(crate) fn record_next_tier(&mut self, tier: Tier) {
        if self.data.tier != tier {
            self.data.tier_fallback = None;
        }
        self.data.tier = tier;
    }
}
