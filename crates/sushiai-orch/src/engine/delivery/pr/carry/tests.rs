use super::*;
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::policy::settings::SandboxMode;
use crate::task::{Assumption, QuestionKind, TaskStatus, VerifyOutcome};
use std::path::Path;
use std::time::Duration;

struct Fixture {
    app: Arc<App>,
    _dir: tempfile::TempDir,
    task: Task,
    repo: PathBuf,
    started: PathBuf,
    release: PathBuf,
    old_head: String,
}

impl Fixture {
    fn new(already_carried: bool) -> Self {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        let started = dir.path().join("check-started");
        let release = dir.path().join("check-release");
        let (old_base, current_base) = prepare_repository(&repo, dir.path(), already_carried);
        let mut task = task_with_status(TaskStatus::Done);
        task.fixture_data().repo = repo.to_string_lossy().into_owned();
        task.fixture_data().branch = "task/owner-carry".into();
        task.fixture_data().base_sha = if already_carried {
            current_base.clone()
        } else {
            old_base
        };
        task.fixture_data().final_verify = vec!["sh ci.sh".into()];
        task.record_pr_pending(true);
        task.record_assumption(Assumption {
            question: "Accept the inherited check?".into(),
            answer: crate::task::questions::WAIVED.into(),
            evidence: "owner_case_17 fails on the base".into(),
            by: crate::task::questions::WAIVER_BY.into(),
            kind: Some(QuestionKind::PreexistingFailure),
            attempt: None,
            commands: vec!["sh ci.sh".into()],
            overturned: false,
            owner_answer: None,
        });
        app.store.save_task(&task).unwrap();
        app.base_runs.lock().unwrap().insert(
            (current_base, "sh ci.sh".into()),
            VerifyOutcome {
                command: "sh ci.sh".into(),
                code: Some(1),
                tail: "not ok 1 - owner_case_17".into(),
                failing: vec!["owner_case_17".into()],
                output_key: crate::policy::test_output::parse("not ok 1 - owner_case_17\n", "")
                    .rest_key,
                ..Default::default()
            },
        );
        let old_head = git_in(&repo, &["rev-parse", "task/owner-carry"]);
        Self {
            app,
            _dir: dir,
            task,
            repo,
            started,
            release,
            old_head,
        }
    }

    async fn overturn(&self) {
        self.app
            .dispatch(
                "task.overturn",
                serde_json::json!({
                    "id": self.task.id, "index": 0, "answer": "Keep this check; repair its failure"
                }),
            )
            .await
            .unwrap();
    }

    fn assert_owner_has_the_failed_check(&self) {
        let saved = self.app.store.load_task(&self.task.id).unwrap().unwrap();
        assert_eq!(
            saved.status,
            TaskStatus::Waiting,
            "the carry left Done without a PR or active owner: {saved:?}"
        );
        assert!(saved.pr_pending && saved.pr_url.is_none());
        assert!(saved.assumptions[0].overturned);
        assert_eq!(
            saved.assumptions.len(),
            1,
            "the rejected waiver was recreated"
        );
        let question = saved.question.as_ref().unwrap();
        assert_eq!(question.kind, QuestionKind::PrCheck);
        assert!(question.for_owner && question.commands == ["sh ci.sh"]);
        assert!(question.text.contains("owner_case_17"));
        assert!(saved.queue.pr_check_repair.is_some());
        assert!(saved
            .delivery_checks
            .iter()
            .any(|run| run.command == "sh ci.sh" && run.code == Some(1)));
        assert_eq!(
            git_in(&self.repo, &["rev-parse", "task/owner-carry"]),
            self.old_head
        );
    }
}

fn prepare_repository(repo: &Path, tools: &Path, already_carried: bool) -> (String, String) {
    let remote = tools.join("origin.git");
    let started = tools.join("check-started");
    let release = tools.join("check-release");
    std::fs::create_dir_all(repo).unwrap();
    init_git_repo(repo);
    git_in(repo, &["branch", "-M", "main"]);
    let script = format!(
        "if [ -e newer.txt ] && [ -e task.txt ]; then\n : > {}\n while [ ! -e {} ]; do sleep 0.01; done\n printf 'not ok 1 - owner_case_17\\n'\n exit 1\nfi\n",
        shell_path(&started), shell_path(&release),
    );
    std::fs::write(repo.join("ci.sh"), script).unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-qm", "check fixture"]);
    let old_base = git_in(repo, &["rev-parse", "HEAD"]);
    git_in(tools, &["init", "--bare", "-q", remote.to_str().unwrap()]);
    git_in(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git_in(repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git_in(repo, &["push", "-qu", "origin", "main"]);
    git_in(repo, &["checkout", "-qb", "task/owner-carry"]);
    std::fs::write(repo.join("task.txt"), "actual task work\n").unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-qm", "task work"]);
    git_in(repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("newer.txt"), "new base\n").unwrap();
    git_in(repo, &["add", "."]);
    git_in(repo, &["commit", "-qm", "advance base"]);
    git_in(repo, &["push", "-q", "origin", "main"]);
    let current_base = git_in(repo, &["rev-parse", "HEAD"]);
    if already_carried {
        git_in(repo, &["checkout", "-q", "task/owner-carry"]);
        git_in(repo, &["rebase", "main"]);
        git_in(repo, &["checkout", "-q", "main"]);
    }
    (old_base, current_base)
}

fn shell_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

async fn wait_for_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the real carried check did not reach its barrier");
}

#[tokio::test]
async fn an_owner_revocation_during_automatic_carry_reaches_the_current_check_question() {
    let fixture = Fixture::new(false);
    let (app, task) = (fixture.app.clone(), fixture.task.clone());
    let worker = tokio::spawn(async move {
        crate::engine::delivery::pr::automatic_carry::carry_before_draft(&app, &task).await
    });
    wait_for_file(&fixture.started).await;
    fixture.overturn().await;
    std::fs::write(&fixture.release, "").unwrap();
    assert!(
        !worker.await.unwrap(),
        "the revoked failure admitted a PR draft"
    );
    fixture.assert_owner_has_the_failed_check();
}

#[tokio::test]
async fn an_already_carried_branch_rechecks_the_failure_whose_waiver_the_owner_revoked() {
    let fixture = Fixture::new(true);
    fixture.overturn().await;
    std::fs::write(&fixture.release, "").unwrap();
    let fresh = fixture
        .app
        .store
        .load_task(&fixture.task.id)
        .unwrap()
        .unwrap();
    let outcome = carry_for_pr(&fixture.app, &fresh, None).await.unwrap();
    assert!(
        matches!(outcome, PrCarry::ChecksPending),
        "contains bypassed the rejected final check"
    );
    fixture.assert_owner_has_the_failed_check();
}
