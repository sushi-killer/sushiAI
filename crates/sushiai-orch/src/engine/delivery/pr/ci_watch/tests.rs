use super::gh::{log_text, outcome, Bucket, Check, Conclusion, Gh, Run, RunStatus, Runs};
use super::*;
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

const URL: &str = "https://github.example/acme/app/pull/7";
const HEAD: &str = "abc123";

fn log(test: &str) -> String {
    format!(
        "unit\tRun tests\t2026-10-16T10:00:00.0000000Z test {test} ... FAILED\nunit\tRun tests\t2026-10-16T10:00:01.0000000Z test result: FAILED. 0 passed; 1 failed"
    )
}

/// A gh whose `pr checks` answers come from a queue; the last one repeats.
#[derive(Default)]
struct Stub {
    checks: Mutex<VecDeque<Option<Vec<Check>>>>,
    head_runs: Vec<Run>,
    base_runs: Vec<Run>,
    logs: HashMap<u64, String>,
    reruns: Mutex<Vec<u64>>,
    calls: Mutex<Vec<String>>,
}

impl Stub {
    fn answering(answers: Vec<Option<Vec<Check>>>) -> Self {
        Self {
            checks: Mutex::new(answers.into()),
            ..Default::default()
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Gh for Stub {
    async fn checks(&self, url: &str) -> Option<Vec<Check>> {
        self.calls.lock().unwrap().push(format!("checks {url}"));
        let mut queue = self.checks.lock().unwrap();
        if queue.len() > 1 {
            queue.pop_front().unwrap()
        } else {
            queue.front().cloned().flatten()
        }
    }

    async fn runs(&self, of: Runs<'_>) -> Vec<Run> {
        self.calls.lock().unwrap().push(format!("runs {of:?}"));
        match of {
            Runs::Commit(_) => self.head_runs.clone(),
            Runs::Branch(_) => self.base_runs.clone(),
        }
    }

    async fn failed_log(&self, run: u64) -> String {
        self.calls.lock().unwrap().push(format!("log {run}"));
        self.logs.get(&run).cloned().unwrap_or_default()
    }

    async fn rerun_failed(&self, run: u64) -> Result<(), String> {
        self.reruns.lock().unwrap().push(run);
        Ok(())
    }
}

fn check(bucket: Bucket) -> Check {
    Check {
        name: "unit".into(),
        bucket,
        workflow: "ci".into(),
    }
}

fn run(id: u64, conclusion: Conclusion) -> Run {
    Run {
        database_id: id,
        status: RunStatus::Completed,
        conclusion,
        workflow_name: "ci".into(),
    }
}

/// A done task with its pull request open and watched, in a repo whose
/// default branch is `main`.
fn watched(app: &App, dir: &std::path::Path) -> Task {
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    git_in(&repo, &["branch", "-M", "main"]);
    let mut task = task_with_status(TaskStatus::Done);
    task.fixture_data().repo = repo.to_string_lossy().into_owned();
    task.record_pr_opened(URL.into(), "task/do-thing".into());
    task.begin_pr_ci_watch(URL, HEAD, now_ms());
    app.store.save_task(&task).unwrap();
    task
}

async fn poll_once(app: &Arc<App>, gh: &Stub, id: &str) -> Poll {
    let task = app.store.load_task(id).unwrap().unwrap();
    let watch = task.open_pr_ci_watch().unwrap().clone();
    poll(app, gh, &task, &watch).await
}

fn saved(app: &App, id: &str) -> Task {
    app.store.load_task(id).unwrap().unwrap()
}

fn watch_of(task: &Task) -> PrCiWatch {
    task.queue.pr_ci_watch.clone().unwrap()
}

#[test]
fn the_log_prefix_is_stripped_and_the_job_log_is_kept_in_the_failure() {
    let raw = log("routines::own_case");
    assert_eq!(
        log_text(&raw).lines().next(),
        Some("test routines::own_case ... FAILED")
    );
    assert_eq!(log_text("plain line"), "plain line");
    let failed = outcome("ci / unit", &raw);
    assert_eq!(failed.failing, ["routines::own_case"]);
    assert!(failed
        .tail
        .contains("unit\tRun tests\t2026-10-16T10:00:00.0000000Z"));
    assert!(!failed.output_key.is_empty());
}

#[tokio::test]
async fn pending_checks_are_polled_again_and_the_task_is_untouched() {
    let (app, dir) = test_app();
    let task = watched(&app, dir.path());
    let gh = Stub::answering(vec![Some(vec![
        check(Bucket::Pending),
        check(Bucket::Fail),
    ])]);
    assert_eq!(poll_once(&app, &gh, &task.id).await, Poll::Again);
    assert_eq!(
        serde_json::to_value(saved(&app, &task.id)).unwrap(),
        serde_json::to_value(&task).unwrap()
    );
    assert_eq!(gh.calls(), [format!("checks {URL}")]);
}

#[tokio::test]
async fn green_no_checks_or_an_unlisting_gh_ends_the_watch_quietly() {
    for answer in [Some(vec![check(Bucket::Pass)]), Some(vec![]), None] {
        let (app, dir) = test_app();
        let task = watched(&app, dir.path());
        let gh = Stub::answering(vec![answer]);
        assert_eq!(poll_once(&app, &gh, &task.id).await, Poll::Settled);
        let after = saved(&app, &task.id);
        let watch = watch_of(&after);
        assert!(watch.finished && watch.told.is_empty() && watch.rerun.is_none());
        assert_eq!(after.status, TaskStatus::Done);
        assert_eq!(after.decisions, task.decisions);
        assert_eq!(gh.calls().len(), 1, "only the checks were read");
    }
}

#[tokio::test]
async fn a_test_failing_on_the_base_branch_too_is_told_once_without_a_rerun() {
    let (app, dir) = test_app();
    let task = watched(&app, dir.path());
    let gh = Stub {
        head_runs: vec![run(555, Conclusion::Failure)],
        base_runs: vec![run(900, Conclusion::Failure), run(800, Conclusion::Success)],
        logs: HashMap::from([(555, log("x::case")), (900, log("x::case"))]),
        ..Stub::answering(vec![Some(vec![check(Bucket::Fail)])])
    };
    assert_eq!(poll_once(&app, &gh, &task.id).await, Poll::Settled);
    let after = saved(&app, &task.id);
    assert_eq!(watch_of(&after).told, ["base:main:x::case"]);
    assert!(gh.reruns.lock().unwrap().is_empty());
    assert_eq!(after.status, TaskStatus::Done);
    assert_eq!(after.attempts.len(), task.attempts.len());
    assert!(gh.calls().contains(&"runs Branch(\"main\")".to_string()));
}

#[tokio::test]
async fn a_failure_that_passes_on_its_one_rerun_is_told_once_as_flaky() {
    let (app, dir) = test_app();
    let task = watched(&app, dir.path());
    let gh = Stub {
        head_runs: vec![run(555, Conclusion::Failure)],
        base_runs: vec![run(900, Conclusion::Success)],
        logs: HashMap::from([(555, log("x::flaky"))]),
        ..Stub::answering(vec![
            Some(vec![check(Bucket::Fail)]),
            Some(vec![check(Bucket::Pending)]),
            Some(vec![check(Bucket::Pass)]),
        ])
    };
    assert_eq!(poll_once(&app, &gh, &task.id).await, Poll::Again);
    assert_eq!(*gh.reruns.lock().unwrap(), [555]);
    let rerun = watch_of(&saved(&app, &task.id)).rerun.unwrap();
    assert_eq!(
        (rerun.runs, rerun.failed),
        (vec![555], vec!["x::flaky".into()])
    );
    assert_eq!(poll_once(&app, &gh, &task.id).await, Poll::Again);
    assert_eq!(poll_once(&app, &gh, &task.id).await, Poll::Settled);
    let after = saved(&app, &task.id);
    let watch = watch_of(&after);
    assert!(watch.finished);
    assert_eq!(watch.told, ["rerun:x::flaky"]);
    assert_eq!(*gh.reruns.lock().unwrap(), [555], "rerun exactly once");
    assert_eq!(after.status, TaskStatus::Done);
    assert!(after.queue.pr_check_repair.is_none());
}

#[tokio::test]
async fn a_restarted_watch_after_its_rerun_tells_once_and_never_reruns_again() {
    let (app, dir) = test_app();
    let mut task = watched(&app, dir.path());
    task.record_pr_ci_rerun(CiRerun {
        runs: vec![555],
        failed: vec!["x::flaky".into()],
    });
    app.store.save_task(&task).unwrap();
    // What a restart reads back from disk.
    let restored = saved(&app, &task.id);
    assert!(watchable(&restored).is_some());
    let gh = Stub::answering(vec![Some(vec![check(Bucket::Pass)])]);
    assert_eq!(poll_once(&app, &gh, &task.id).await, Poll::Settled);
    assert!(gh.reruns.lock().unwrap().is_empty());
    let watch = watch_of(&saved(&app, &task.id));
    assert_eq!(watch.told, ["rerun:x::flaky"]);
    // The same cause on a later head of the same pull request is not told again.
    let mut again = saved(&app, &task.id);
    again.begin_pr_ci_watch(URL, "def456", now_ms());
    assert!(!again.record_pr_ci_told("rerun:x::flaky"));
}

#[tokio::test]
async fn a_watch_past_its_bound_stops_and_an_older_loop_steps_aside() {
    let (app, dir) = test_app();
    let mut task = watched(&app, dir.path());
    task.begin_pr_ci_watch(URL, HEAD, now_ms() - 10_000);
    app.store.save_task(&task).unwrap();
    let mine = Mine::of(task.open_pr_ci_watch().unwrap());
    let gh = Stub::answering(vec![Some(vec![check(Bucket::Pending)])]);
    let timing = Timing {
        poll: Duration::from_millis(1),
        bound: Duration::from_millis(5_000),
    };
    watch_checks(&app, &gh, &task.id, &mine, timing).await;
    let after = saved(&app, &task.id);
    assert!(watch_of(&after).finished);
    assert!(gh.calls().is_empty(), "past the bound nothing is polled");
    assert!(after.decisions.last().unwrap().contains("stopped watching"));
    // A loop of an earlier head returns without a poll.
    let mut moved = saved(&app, &task.id);
    moved.begin_pr_ci_watch(URL, "def456", now_ms());
    app.store.save_task(&moved).unwrap();
    watch_checks(&app, &gh, &task.id, &mine, timing).await;
    assert!(gh.calls().is_empty());
}

#[tokio::test]
async fn a_watch_with_a_repair_in_flight_or_another_pull_request_does_not_poll() {
    let (app, dir) = test_app();
    let mut task = watched(&app, dir.path());
    task.fixture_data().pr_url = Some("https://github.example/other/1".into());
    assert!(watchable(&task).is_none());
    task.fixture_data().pr_url = Some(URL.into());
    task.schedule_pr_check_repair(
        "cause".into(),
        crate::task::Failure {
            kind: crate::task::FailureKind::Verify,
            detail: "d".into(),
            signature: "d".into(),
        },
    );
    assert!(watchable(&task).is_none());
}
