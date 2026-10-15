use crate::task::{Deliverable, DiffStat, FreshBase, LeadTouch, PrPlan, Task, TaskData, Unlanded};

pub(crate) struct ReportObservation {
    pub(crate) text: String,
    pub(crate) lead_touch: Option<LeadTouch>,
    pub(crate) unlanded: Option<Unlanded>,
}

impl Task {
    pub(crate) fn stamp_landing(&mut self, now: i64) {
        self.data.landed_at = Some(now);
        self.data.loop_state.rereviews = 0;
    }
    pub(crate) fn record_landed(&mut self, sha: Option<String>) {
        self.data.landed_sha = sha;
        self.data.unlanded = None;
    }
    pub(crate) fn record_base_ref(&mut self, reference: Option<String>) {
        self.data.base_ref = reference;
    }
    pub(crate) fn record_fresh_base(&mut self, fresh: FreshBase) {
        self.data.fresh_base = Some(fresh);
    }
    pub(crate) fn record_unlanded(&mut self, unlanded: Option<Unlanded>) {
        self.data.unlanded = unlanded;
    }
    pub(crate) fn record_lead_touch(&mut self, touched: Option<LeadTouch>) {
        self.data.lead_touch = touched;
    }
    pub(crate) fn record_report(&mut self, report: ReportObservation, now: i64) {
        self.data.lead_touch = report.lead_touch;
        self.data.unlanded = report.unlanded;
        self.data.report = Some(report.text);
        self.data.report_at.get_or_insert(now);
    }
    pub(crate) fn adopt_report(&mut self, report: &TaskData) {
        self.data.lead_touch = report.lead_touch.clone();
        self.data.unlanded = report.unlanded.clone();
        self.data.deliverables = report.deliverables.clone();
        self.data.report = report.report.clone();
        self.data.report_at = report.report_at;
    }
    pub(crate) fn record_deliverables(&mut self, files: Vec<Deliverable>) {
        self.data.deliverables = files;
    }
    pub(crate) fn record_worktree_removed(&mut self, removed: bool) {
        self.data.worktree_removed = removed;
    }
    pub(crate) fn archive(&mut self, removed: bool, files: Vec<Deliverable>) {
        self.data.archived = true;
        self.data.worktree_removed = removed;
        self.data.deliverables = files;
    }
    pub(crate) fn unarchive(&mut self, removed: bool) {
        self.data.archived = false;
        self.data.worktree_removed = removed;
    }
    pub(crate) fn link_follow_up(&mut self, parent: String) {
        self.data.follow_up_of = Some(parent);
    }
    pub(crate) fn record_follow_ups(&mut self, follow_ups: impl IntoIterator<Item = String>) {
        self.data.follow_ups.extend(follow_ups);
    }
    pub(crate) fn clear_pr_conflict(&mut self) {
        self.data.queue.pr_conflict_tried = false;
    }
    pub(crate) fn record_pr_plan(&mut self, plan: PrPlan) {
        self.data.pr_plan = Some(plan);
    }
    pub(crate) fn defer_pr(&mut self, plan: Option<PrPlan>) {
        self.data.pr_pending = true;
        self.data.pr_plan = plan;
    }
    pub(crate) fn record_pr_pending(&mut self, pending: bool) {
        self.data.pr_pending = pending;
    }
    pub(crate) fn finish_pr(&mut self) {
        self.clear_pr_check_repair();
        self.data.pr_pending = false;
        self.data.pr_plan = None;
        self.data.queue.pr_conflict_tried = false;
    }
    pub(crate) fn record_pr_opened(&mut self, url: String, branch: String) {
        self.clear_pr_check_repair();
        self.data.pr_url = Some(url);
        self.data.pr_branch = Some(branch);
        self.data.queue.pr_conflict_tried = false;
    }
    pub(crate) fn begin_dirty_wait(&mut self, now: i64) -> i64 {
        *self.data.queue.dirty_since.get_or_insert(now)
    }
    pub(crate) fn end_dirty_wait(&mut self) {
        self.data.queue.dirty_since = None;
    }
}

impl Task {
    pub(crate) fn record_task_diff_stat(&mut self, stat: Option<DiffStat>) {
        self.data.diff_stat = stat;
    }
    pub(crate) fn mark_pr_conflict(&mut self) {
        self.data.queue.pr_conflict_tried = true;
    }
}

impl Task {
    pub(crate) fn begin_delivery(&mut self, now: i64) -> bool {
        if !self.delivery_allowed() {
            return false;
        }
        if self.data.status == crate::task::TaskStatus::Running {
            self.end_dirty_wait();
        }
        self.data.status = crate::task::TaskStatus::Landing;
        self.data.updated_at = now;
        true
    }
}

impl TaskData {
    pub(crate) fn carry_allowed(&self, before: &TaskData) -> bool {
        !self.archived
            && self.queue.park.is_none()
            && self.status == before.status
            && matches!(
                self.status,
                crate::task::TaskStatus::Queued
                    | crate::task::TaskStatus::Running
                    | crate::task::TaskStatus::Landing
            )
    }

    pub(crate) fn delivery_allowed(&self) -> bool {
        !self.archived
            && self.queue.park.is_none()
            && matches!(
                self.status,
                crate::task::TaskStatus::Running | crate::task::TaskStatus::Landing
            )
    }
}
