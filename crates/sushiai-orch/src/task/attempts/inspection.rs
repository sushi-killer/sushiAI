use crate::task::{AttemptStatus, DiffStat, Failure, Stage, Task};

use super::observations::AttemptReport;

/// The completed implementer's blocked report, independent of the answer it needs.
pub(crate) struct BlockedInspection {
    task_id: String,
    number: u32,
    started_at: i64,
    ended_at: i64,
    report: AttemptReport,
    files: Vec<String>,
    diff: Option<DiffStat>,
    failure: Failure,
}

impl BlockedInspection {
    pub(crate) fn capture(task: &Task, index: usize) -> Result<Self, String> {
        let attempt = task
            .attempts
            .get(index)
            .filter(|attempt| {
                attempt.stage == Stage::Implement && attempt.status == AttemptStatus::Blocked
            })
            .ok_or_else(|| "blocked implement attempt not found".to_string())?;
        Ok(Self {
            task_id: task.id.clone(),
            number: attempt.n,
            started_at: attempt.started_at,
            ended_at: attempt
                .ended_at
                .ok_or_else(|| "blocked attempt has not ended".to_string())?,
            report: AttemptReport {
                summary: attempt.summary.clone(),
                handoff: attempt.handoff.clone(),
                declines: attempt.declines.clone(),
            },
            files: attempt.changed_files.clone(),
            diff: attempt.diff_stat,
            failure: attempt
                .failure
                .clone()
                .ok_or_else(|| "blocked attempt has no failure".to_string())?,
        })
    }

    pub(crate) fn apply(&self, task: &mut Task) -> Result<(), String> {
        if task.id != self.task_id {
            return Err("blocked inspection belongs to another task".into());
        }
        let attempt = task
            .data
            .attempts
            .last_mut()
            .filter(|attempt| {
                attempt.n == self.number
                    && attempt.started_at == self.started_at
                    && attempt.stage == Stage::Implement
                    && matches!(
                        attempt.status,
                        AttemptStatus::Running | AttemptStatus::Blocked
                    )
            })
            .ok_or_else(|| "blocked inspection attempt was replaced".to_string())?;
        attempt.status = AttemptStatus::Blocked;
        attempt.ended_at = Some(self.ended_at);
        attempt.failure = Some(self.failure.clone());
        attempt.summary.clone_from(&self.report.summary);
        attempt.handoff.clone_from(&self.report.handoff);
        attempt.declines.clone_from(&self.report.declines);
        attempt.changed_files.clone_from(&self.files);
        attempt.diff_stat.clone_from(&self.diff);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
