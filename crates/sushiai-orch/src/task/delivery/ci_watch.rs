use crate::task::Task;
use serde::{Deserialize, Serialize};

/// The watch of an open pull request's checks. Saved with the task so a
/// restart resumes an unfinished watch, and so the owner hears about one
/// cause of one pull request once.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrCiWatch {
    /// The pull request watched.
    pub url: String,
    /// The pushed head commit the checks run on.
    pub head: String,
    /// When the watch of this head began (ms); the bound counts from here.
    pub started_at: i64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub finished: bool,
    /// The failed jobs of this head were rerun once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerun: Option<CiRerun>,
    /// Causes the owner was told about on this pull request.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub told: Vec<String>,
    /// Causes sent to the task's repair step on this pull request.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub repaired: Vec<String>,
}

/// The one rerun of a head's failed jobs: the runs and what failed before it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CiRerun {
    pub runs: Vec<u64>,
    /// The failing tests, else the failing checks, before the rerun.
    pub failed: Vec<String>,
}

impl Task {
    /// Starts watching `head` of the pull request `url`. A new head of the
    /// same pull request keeps what the owner was told and what was repaired.
    pub(crate) fn begin_pr_ci_watch(&mut self, url: &str, head: &str, now: i64) {
        let previous = self
            .data
            .queue
            .pr_ci_watch
            .take()
            .filter(|watch| watch.url == url);
        let (told, repaired) =
            previous.map_or_else(Default::default, |watch| (watch.told, watch.repaired));
        self.data.queue.pr_ci_watch = Some(PrCiWatch {
            url: url.to_string(),
            head: head.to_string(),
            started_at: now,
            finished: false,
            rerun: None,
            told,
            repaired,
        });
    }

    /// The watch while it is unfinished and still about the open pull request.
    pub(crate) fn open_pr_ci_watch(&self) -> Option<&PrCiWatch> {
        self.queue
            .pr_ci_watch
            .as_ref()
            .filter(|watch| !watch.finished && self.pr_url.as_deref() == Some(watch.url.as_str()))
    }

    pub(crate) fn finish_pr_ci_watch(&mut self) {
        if let Some(watch) = self.data.queue.pr_ci_watch.as_mut() {
            watch.finished = true;
        }
    }

    pub(crate) fn record_pr_ci_rerun(&mut self, rerun: CiRerun) {
        if let Some(watch) = self.data.queue.pr_ci_watch.as_mut() {
            watch.rerun = Some(rerun);
        }
    }

    /// Records that the owner is told about `cause`; false when they already were.
    pub(crate) fn record_pr_ci_told(&mut self, cause: &str) -> bool {
        record_once(
            self.data.queue.pr_ci_watch.as_mut().map(|w| &mut w.told),
            cause,
        )
    }

    /// Records that `cause` goes to the repair step; false when it already went.
    pub(crate) fn record_pr_ci_repair(&mut self, cause: &str) -> bool {
        record_once(
            self.data
                .queue
                .pr_ci_watch
                .as_mut()
                .map(|w| &mut w.repaired),
            cause,
        )
    }

    /// Whether the pull request flow may still push this task's branch: no
    /// pull request is open yet, or the open one has a check repair pending.
    pub(crate) fn pr_push_owed(&self) -> bool {
        self.pr_url.is_none() || self.queue.pr_check_repair.is_some()
    }
}

fn record_once(list: Option<&mut Vec<String>>, cause: &str) -> bool {
    match list {
        Some(list) if !list.iter().any(|known| known == cause) => {
            list.push(cause.to_string());
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::task::test_support::sample_task;
    use crate::task::{AttemptStatus, TaskStatus};

    #[test]
    fn a_new_head_of_the_same_pull_request_keeps_what_was_told_and_repaired() {
        let mut task = sample_task("watch", TaskStatus::Done, AttemptStatus::Passed);
        task.fixture_data().pr_url = Some("u1".into());
        task.begin_pr_ci_watch("u1", "a", 1);
        assert!(task.record_pr_ci_told("flaky:x"));
        assert!(!task.record_pr_ci_told("flaky:x"));
        assert!(task.record_pr_ci_repair("own:y"));
        task.record_pr_ci_rerun(super::CiRerun {
            runs: vec![5],
            failed: vec!["x".into()],
        });
        task.finish_pr_ci_watch();
        assert!(task.open_pr_ci_watch().is_none());
        task.begin_pr_ci_watch("u1", "b", 2);
        let watch = task.open_pr_ci_watch().unwrap();
        assert_eq!((watch.head.as_str(), watch.started_at), ("b", 2));
        assert!(watch.rerun.is_none());
        assert!(!task.record_pr_ci_told("flaky:x"));
        assert!(!task.record_pr_ci_repair("own:y"));
        task.begin_pr_ci_watch("u2", "c", 3);
        assert!(task.open_pr_ci_watch().is_none(), "another url is not open");
        assert!(task.record_pr_ci_told("flaky:x"));
    }

    #[test]
    fn the_watch_roundtrips_and_a_push_is_owed_only_for_a_pending_repair() {
        let mut task = sample_task("watch", TaskStatus::Done, AttemptStatus::Passed);
        assert!(task.pr_push_owed());
        task.fixture_data().pr_url = Some("u".into());
        assert!(!task.pr_push_owed());
        task.schedule_pr_check_repair(
            "c".into(),
            crate::task::Failure {
                kind: crate::task::FailureKind::Verify,
                detail: "d".into(),
                signature: "d".into(),
            },
        );
        assert!(task.pr_push_owed());
        task.begin_pr_ci_watch("u", "h", 9);
        let restored: crate::task::Task =
            serde_json::from_value(serde_json::to_value(&task).unwrap()).unwrap();
        assert_eq!(restored.queue.pr_ci_watch, task.queue.pr_ci_watch);
    }
}
