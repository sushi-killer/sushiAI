use crate::policy::delivery::BaseFrom;
use crate::policy::routing::Tier;
use crate::policy::variant::Variant;
use crate::task::{
    Backlog, Check, ChecklistItem, FreshBase, QueueState, Task, TaskData, TaskStatus,
};
use std::path::PathBuf;

/// Everything `create_task_record` needs for a new task.
pub(crate) struct NewTask {
    pub(crate) id: String,
    pub(crate) backlog: Option<Backlog>,
    pub(crate) repo_root: PathBuf,
    pub(crate) project_id: Option<String>,
    pub(crate) title: String,
    pub(crate) decisions: Vec<String>,
    pub(crate) goal: String,
    pub(crate) criteria: Vec<String>,
    pub(crate) verify: Vec<String>,
    pub(crate) final_verify: Vec<String>,
    pub(crate) checks: Vec<Check>,
    pub(crate) held_out: Option<Check>,
    pub(crate) request: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) base: String,
    /// Whether `base` is fetched first (see `fresh_base`).
    pub(crate) base_from: BaseFrom,
    pub(crate) variant: Variant,
    pub(crate) depends_on: Vec<String>,
    pub(crate) parent: Option<String>,
    pub(crate) paths: Vec<String>,
    /// The sibling subtask whose branch this one continues.
    pub(crate) relay_of: Option<String>,
    /// The ordered parts a merged task checks after every attempt.
    pub(crate) checklist: Vec<ChecklistItem>,
    pub(crate) eval_set: Option<String>,
    pub(crate) eval_name: Option<String>,
    pub(crate) eval_check_cmd: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) created_at: i64,
}

/// The resolved checkout returned by repository preparation.
pub(crate) struct Workspace {
    pub(crate) path: String,
    pub(crate) branch: String,
    pub(crate) base_sha: String,
    pub(crate) base_ref: Option<String>,
    pub(crate) fresh_base: Option<FreshBase>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn create(new: NewTask, workspace: Workspace) -> Task {
    Task {
        data: TaskData {
            id: new.id,
            title: new.title,
            goal: new.goal,
            criteria: new.criteria,
            verify: new.verify,
            final_verify: new.final_verify,
            checks: new.checks,
            held_out: new.held_out,
            planner_tests: Default::default(),
            // A backlog task with a request is planned when it starts, on
            // the base as it is then.
            status: if new.request.is_some() && new.backlog.is_none() {
                TaskStatus::Drafting
            } else {
                TaskStatus::Queued
            },
            request: new.request,
            repo: new.repo_root.to_string_lossy().to_string(),
            project_id: new.project_id,
            worktree: workspace.path,
            worktree_removed: false,
            deliverables: vec![],
            visual_criteria: vec![],
            landed_sha: None,
            landed_at: None,
            unlanded: None,
            diff_stat: None,
            report: None,
            report_at: None,
            lead_touch: None,
            follow_up_of: None,
            follow_ups: vec![],
            branch: workspace.branch,
            base_sha: workspace.base_sha,
            base_ref: workspace.base_ref,
            fresh_base: workspace.fresh_base,
            depends_on: new.depends_on,
            addenda: Vec::new(),
            paths: new.paths,
            parent: new.parent,
            tier: Tier::Standard,
            question: None,
            pending_amendment: None,
            delivery_checks: vec![],
            decisions: workspace
                .warnings
                .into_iter()
                .chain(new.decisions)
                .collect(),
            attempts: vec![],
            cost_usd: 0.0,
            budget_raises: 0,
            daily_budget_ok_day: None,
            assumptions: vec![],
            question_history: vec![],
            findings: vec![],
            archived: false,
            pr_url: None,
            pr_branch: None,
            pr_pending: false,
            pr_plan: None,
            overseer_woken: false,
            unreviewed: false,
            failed_count: 0,
            loop_state: Default::default(),
            planned_tier: None,
            planned_by_parent: false,
            tier_fallback: None,
            variant: Some(new.variant),
            eval_set: new.eval_set,
            eval_name: new.eval_name,
            eval_check_cmd: new.eval_check_cmd,
            source: new.source,
            eval_check: None,
            brief_check: Default::default(),
            plan_gate: Default::default(),
            metrics: None,
            checklist: new.checklist,
            queue: QueueState {
                relay_of: new.relay_of,
                backlog: new.backlog,
                ..Default::default()
            },
            created_at: new.created_at,
            updated_at: new.created_at,
        },
    }
}
