//! The pinned `gh` surface the watch reads: the pull request's checks, the
//! Actions runs of a commit or of a branch, a run's failed-job log, and the
//! rerun of a run's failed jobs. Code reads only gh's typed JSON fields.

use crate::delivery::pr::command::{gh_bin, run_cmd, run_with_code, GH_TIMEOUT};
use crate::prompts::text::tail_chars;
use crate::task::VerifyOutcome;
use serde::Deserialize;
use std::path::PathBuf;

/// The job log kept in a failure: the end, where test runners summarize.
const LOG_CHARS: usize = 6000;

/// gh's summary of one check's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Bucket {
    Pass,
    Fail,
    Pending,
    Skipping,
    Cancel,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct Check {
    pub name: String,
    pub bucket: Bucket,
    #[serde(default)]
    pub workflow: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RunStatus {
    Completed,
    #[serde(other)]
    Unfinished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Conclusion {
    Success,
    Failure,
    TimedOut,
    StartupFailure,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Run {
    pub database_id: u64,
    pub status: RunStatus,
    pub conclusion: Conclusion,
    pub workflow_name: String,
}

impl Run {
    pub(super) fn failed(&self) -> bool {
        self.status == RunStatus::Completed
            && matches!(
                self.conclusion,
                Conclusion::Failure | Conclusion::TimedOut | Conclusion::StartupFailure
            )
    }
}

/// Which runs to list: those of a commit, or those of a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Runs<'a> {
    Commit(&'a str),
    Branch(&'a str),
}

/// What the watch asks gh; a test stubs it.
pub(super) trait Gh {
    /// The pull request's checks; `None` when gh cannot list them.
    async fn checks(&self, url: &str) -> Option<Vec<Check>>;
    /// Newest first, as gh lists them; empty when gh cannot list them.
    async fn runs(&self, of: Runs<'_>) -> Vec<Run>;
    /// The run's failed-job log; empty when gh cannot read it.
    async fn failed_log(&self, run: u64) -> String;
    async fn rerun_failed(&self, run: u64) -> Result<(), String>;
}

/// The real `gh`, run in the task's repository.
pub(super) struct GhCli {
    pub repo: PathBuf,
}

impl Gh for GhCli {
    async fn checks(&self, url: &str) -> Option<Vec<Check>> {
        let args = ["pr", "checks", url, "--json", "name,bucket,workflow,link"];
        let (code, out) = run_with_code(&gh_bin(), &args, &self.repo, GH_TIMEOUT)
            .await
            .ok()?;
        // 1: a check fails, 8: one is pending. stdout holds the list either way.
        if !matches!(code, Some(0 | 1 | 8)) {
            return None;
        }
        serde_json::from_str(&out).ok()
    }

    async fn runs(&self, of: Runs<'_>) -> Vec<Run> {
        let (flag, value) = match of {
            Runs::Commit(sha) => ("--commit", sha),
            Runs::Branch(branch) => ("--branch", branch),
        };
        let fields = "databaseId,status,conclusion,workflowName,headSha";
        let args = ["run", "list", flag, value, "--json", fields];
        run_cmd(&gh_bin(), &args, &self.repo, GH_TIMEOUT)
            .await
            .ok()
            .and_then(|out| serde_json::from_str(&out).ok())
            .unwrap_or_default()
    }

    async fn failed_log(&self, run: u64) -> String {
        let id = run.to_string();
        let args = ["run", "view", &id, "--log-failed"];
        run_cmd(&gh_bin(), &args, &self.repo, GH_TIMEOUT)
            .await
            .unwrap_or_default()
    }

    async fn rerun_failed(&self, run: u64) -> Result<(), String> {
        let id = run.to_string();
        let args = ["run", "rerun", &id, "--failed"];
        run_cmd(&gh_bin(), &args, &self.repo, GH_TIMEOUT)
            .await
            .map(|_| ())
    }
}

/// The text of a `--log-failed` log, whose lines are
/// `job<TAB>step<TAB>timestamp text`: the text alone, so the test runner's own
/// lines are what the parser reads. A line in another shape is kept whole.
pub(super) fn log_text(log: &str) -> String {
    log.lines()
        .map(|line| match line.splitn(3, '\t').nth(2) {
            Some(rest) => rest.split_once(' ').map_or("", |(_, text)| text),
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One failed CI run as a check outcome: the job log is the output (its
/// end, with its job, step and time columns) and the test names come from
/// the same parser as a local check's.
pub(super) fn outcome(label: &str, log: &str) -> VerifyOutcome {
    let report = crate::policy::test_output::parse(&log_text(log), "");
    VerifyOutcome {
        command: label.to_string(),
        code: Some(1),
        tail: tail_chars(log, LOG_CHARS),
        failing: report.failing,
        passing: report.passing,
        crashed: report.crashed,
        stopped_early: report.stopped_early,
        output_key: report.rest_key,
        ..Default::default()
    }
}

/// A green run of the same workflows: nothing failed.
pub(super) fn passed(label: &str) -> VerifyOutcome {
    VerifyOutcome {
        command: label.to_string(),
        code: Some(0),
        ..Default::default()
    }
}
