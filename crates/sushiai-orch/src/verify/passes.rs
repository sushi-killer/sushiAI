use crate::task::attempt::VerifyOutcome;
use crate::verify::load::timed_out_under_load;
use crate::verify::output::CANCELLED_TAIL;
use crate::verify::output::INTERNAL_ERROR_PREFIX;
use crate::verify::output::SPAWN_FAILED_PREFIX;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::Path;

const PASSED_CHECKS_LIMIT: usize = 512;

type CheckKey = (String, String, Option<String>);

#[derive(Default)]
pub(crate) struct PassedChecks {
    entries: HashMap<CheckKey, VerifyOutcome>,
    order: VecDeque<CheckKey>,
}

impl PassedChecks {
    fn key(tree: &str, command: &str, base: &str) -> CheckKey {
        (
            tree.to_string(),
            command.to_string(),
            command.contains("ORCHD_BASE_SHA").then(|| base.to_string()),
        )
    }

    pub(crate) fn contains(&self, tree: &str, command: &str, base: &str) -> bool {
        self.entries.contains_key(&Self::key(tree, command, base))
    }

    pub(crate) fn insert(&mut self, tree: &str, base: &str, outcome: &VerifyOutcome) {
        if outcome.code != Some(0) || is_interrupted(outcome) {
            return;
        }
        let key = Self::key(tree, &outcome.command, base);
        if self.entries.contains_key(&key) {
            return;
        }
        if self.order.len() == PASSED_CHECKS_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, outcome.clone());
    }
}

pub(crate) fn is_interrupted(outcome: &VerifyOutcome) -> bool {
    if outcome.code != Some(0) && timed_out_under_load(outcome) {
        return true;
    }
    if outcome.code.is_none()
        && outcome.tail != CANCELLED_TAIL
        && !outcome.tail.starts_with(SPAWN_FAILED_PREFIX)
        && !outcome.tail.starts_with(INTERNAL_ERROR_PREFIX)
    {
        return true;
    }
    matches!(outcome.code, Some(124 | 137 | 143))
}

pub(crate) async fn check_tree(cwd: &Path) -> Option<String> {
    let cwd = cwd.to_path_buf();
    tokio::task::spawn_blocking(move || crate::git::changes::worktree_tree(&cwd).ok())
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
#[path = "passes/tests.rs"]
mod tests;
