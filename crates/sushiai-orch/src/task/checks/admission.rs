use super::snapshot::CheckSnapshot;
use crate::task::{Assumption, TaskData};

/// Async gate input includes Owner answers; automatic assumptions remain observations.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CheckAdmission {
    contract: CheckSnapshot,
    owner_answers: Vec<Assumption>,
}

impl CheckAdmission {
    pub(crate) fn capture(task: &TaskData) -> Self {
        Self {
            contract: CheckSnapshot::capture(task),
            owner_answers: task
                .assumptions
                .iter()
                .filter(|answer| answer.overturned || answer.owner_answer.is_some())
                .cloned()
                .collect(),
        }
    }

    pub(crate) fn matches(&self, task: &TaskData) -> bool {
        *self == Self::capture(task)
    }
}

#[cfg(test)]
mod tests;
