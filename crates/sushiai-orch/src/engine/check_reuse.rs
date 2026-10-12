use super::*;
use std::collections::VecDeque;

const PASSED_CHECKS_LIMIT: usize = 512;
type CheckKey = (String, String, Option<String>);

#[derive(Default)]
pub(super) struct PassedChecks {
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

    pub(super) fn contains(&self, tree: &str, command: &str, base: &str) -> bool {
        self.entries.contains_key(&Self::key(tree, command, base))
    }

    pub(super) fn insert(&mut self, tree: &str, base: &str, outcome: &VerifyOutcome) {
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

pub(super) fn is_interrupted(outcome: &VerifyOutcome) -> bool {
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
    if matches!(outcome.code, Some(124 | 137 | 143)) {
        return true;
    }
    let mut failures = false;
    outcome.tail.lines().any(|line| {
        if line.starts_with("--- ") {
            failures = line.trim() == "--- failures ---";
            return false;
        }
        let line = line.to_ascii_lowercase();
        failures && (line.contains("timed out") || line.contains("timeout"))
    })
}

pub(super) async fn check_tree(cwd: &Path) -> Option<String> {
    let cwd = cwd.to_path_buf();
    tokio::task::spawn_blocking(move || git::worktree_tree(&cwd).ok())
        .await
        .ok()
        .flatten()
}

pub(super) async fn passed_on_tree(
    ctx: CheckContext<'_>,
    cwd: &Path,
    command: &str,
    base: &str,
) -> Option<VerifyOutcome> {
    let tree = check_tree(cwd).await?;
    if !ctx
        .app
        .gate_passes
        .lock()
        .unwrap()
        .contains(&tree, command, base)
    {
        return None;
    }
    let mut decisions = Vec::new();
    ctx.decision(
        format!(
            "Orchestrator: {} already passed on tree {}; not run again",
            ctx.label.unwrap_or(command),
            short_sha(&tree)
        ),
        &mut decisions,
    );
    Some(VerifyOutcome {
        command: command.into(),
        code: Some(0),
        decisions,
        ..Default::default()
    })
}

pub(super) struct CheckRunner<'a> {
    ctx: CheckContext<'a>,
    cwd: &'a Path,
    sandbox: SandboxMode,
    timeout: Duration,
    allow_write: Vec<PathBuf>,
    base: &'a str,
    cancel: &'a CancelToken,
}

impl<'a> CheckRunner<'a> {
    pub(super) fn new(
        app: &'a App,
        cwd: &'a Path,
        run_dir: &Path,
        sandbox: SandboxMode,
        base: &'a str,
        cancel: &'a CancelToken,
    ) -> Self {
        Self {
            ctx: CheckContext::new(app, None),
            cwd,
            sandbox,
            timeout: Duration::from_secs(app.settings.read().unwrap().verify_timeout_secs),
            allow_write: verify_allow_write_paths(cwd, run_dir),
            base,
            cancel,
        }
    }

    #[cfg(test)]
    pub(super) async fn run(&self, commands: &[String]) -> (Vec<VerifyOutcome>, Vec<String>) {
        let mut results = Vec::new();
        let mut lines = Vec::new();
        for command in commands {
            let (result, decisions) = self.run_one(command, command, None).await;
            results.push(result);
            lines.extend(decisions);
        }
        (results, lines)
    }

    pub(super) async fn run_one(
        &self,
        command: &str,
        label: &str,
        cached: Option<&VerifyOutcome>,
    ) -> (VerifyOutcome, Vec<String>) {
        let ctx = self.ctx.with_label(label);
        if !self.cancel.is_cancelled() {
            if let Some(outcome) = passed_on_tree(ctx, self.cwd, command, self.base).await {
                let lines = outcome.decisions.clone();
                return (outcome, lines);
            }
            if let Some(cached) =
                cached.filter(|r| r.code.is_some_and(|c| c != 0) && !is_interrupted(r))
            {
                return (cached.clone(), Vec::new());
            }
        }
        let outcome = run_one_verify_command(
            ctx,
            self.cwd,
            command,
            self.timeout,
            self.sandbox,
            &self.allow_write,
            self.base,
            self.cancel,
        )
        .await;
        let lines = outcome.decisions.clone();
        (outcome, lines)
    }

    pub(super) fn for_task(mut self, task_id: &'a str) -> Self {
        self.ctx = CheckContext::new(self.ctx.app, Some(task_id));
        self
    }
}

#[cfg(test)]
pub(super) async fn run_checks(
    app: &App,
    cwd: &Path,
    run_dir: &Path,
    commands: &[String],
    sandbox: SandboxMode,
    base: &str,
    cancel: &CancelToken,
) -> (Vec<VerifyOutcome>, Vec<String>) {
    CheckRunner::new(app, cwd, run_dir, sandbox, base, cancel)
        .run(commands)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(code: Option<i32>, tail: &str) -> VerifyOutcome {
        VerifyOutcome {
            command: "check".into(),
            code,
            tail: tail.into(),
            ms: 1,
            ..Default::default()
        }
    }

    #[test]
    fn an_outcome_is_interrupted_by_a_timeout_a_signal_or_its_failure_lines() {
        for result in [
            outcome(None, &timed_out_tail(Duration::from_secs(1))),
            outcome(None, ""),
            outcome(Some(124), ""),
            outcome(Some(137), ""),
            outcome(Some(143), ""),
            outcome(Some(1), "output\n--- failures ---\nTest TIMED OUT"),
            outcome(Some(1), "--- failures ---\nTimeout in test"),
        ] {
            assert!(is_interrupted(&result), "{result:?}");
        }
        for result in [
            outcome(Some(0), ""),
            outcome(Some(1), "Timeout is the test's name"),
            outcome(
                Some(1),
                "--- failures ---\nassertion failed\n--- stdout (tail) ---\ntimeout",
            ),
            outcome(None, CANCELLED_TAIL),
            outcome(None, &format!("{SPAWN_FAILED_PREFIX}missing")),
            outcome(None, &format!("{INTERNAL_ERROR_PREFIX}pipe")),
        ] {
            assert!(!is_interrupted(&result), "{result:?}");
        }
    }

    #[tokio::test]
    async fn a_timed_out_gate_runs_twice_then_runs_afresh() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let counter = dir.path().join("counter");
        let commands = vec![format!(
            "echo run >> '{}'; n=$(wc -l < '{}'); if [ \"$n\" -le 2 ]; then sleep 60; fi",
            counter.display(),
            counter.display()
        )];
        let cancel = CancelToken::new();
        let mut runner =
            CheckRunner::new(&app, &repo, dir.path(), SandboxMode::Host, "base", &cancel);
        runner.timeout = Duration::from_millis(100);
        let (results, lines) = runner.run(&commands).await;
        assert!(is_interrupted(&results[0]));
        assert_eq!(results[0].code, None);
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("was interrupted after")
                && lines[0].contains("; ran it again at once: interrupted after")
        );
        let (results, lines) = runner.run(&commands).await;
        assert_eq!(results[0].code, Some(0));
        assert!(lines.is_empty());
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            3
        );
    }

    #[tokio::test]
    async fn a_tree_change_runs_the_command_again() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let counter = dir.path().join("counter");
        let commands = vec![format!("echo run >> '{}'", counter.display())];
        let cancel = CancelToken::new();
        let run = || {
            run_checks(
                &app,
                &repo,
                dir.path(),
                &commands,
                SandboxMode::Host,
                "base",
                &cancel,
            )
        };
        let (_, lines) = run().await;
        assert!(lines.is_empty());
        let (results, lines) = run().await;
        assert_eq!(results[0].ms, 0);
        assert_eq!(lines.len(), 1);
        std::fs::write(repo.join("README.md"), "a different tree").unwrap();
        let (_, lines) = run().await;
        assert!(lines.is_empty());
        assert_eq!(std::fs::read_to_string(counter).unwrap().lines().count(), 2);
    }

    #[tokio::test]
    async fn a_check_that_changes_the_tree_cannot_supply_a_pass_for_either_tree() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let counter = dir.path().join("counter");
        let commands = vec![format!(
            "echo run >> '{}'; if [ \"$(cat README.md)\" = hello ]; then echo changed > README.md; fi",
            counter.display()
        )];
        let cancel = CancelToken::new();
        let run = || {
            run_checks(
                &app,
                &repo,
                dir.path(),
                &commands,
                SandboxMode::Host,
                "base",
                &cancel,
            )
        };
        let original = check_tree(&repo).await.unwrap();
        let (outcomes, _) = run().await;
        assert_eq!(outcomes[0].code, Some(0));
        let changed = check_tree(&repo).await.unwrap();
        assert_ne!(original, changed);
        for tree in [&original, &changed] {
            assert!(!app
                .gate_passes
                .lock()
                .unwrap()
                .contains(tree, &commands[0], "base"));
        }

        let (outcomes, lines) = run().await;
        assert_eq!(outcomes[0].code, Some(0));
        assert!(lines.is_empty());
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );
        let (_, lines) = run().await;
        assert!(lines
            .iter()
            .any(|line| line.contains("already passed on tree")));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );

        std::fs::write(repo.join("README.md"), "hello\n").unwrap();
        let (_, lines) = run().await;
        assert!(lines.is_empty());
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            3
        );
    }

    #[tokio::test]
    async fn a_failed_final_gate_retries_only_its_failed_command() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let passed = dir.path().join("passed");
        let failed = dir.path().join("failed");
        let ready = dir.path().join("ready");
        let commands = vec![
            format!("echo run >> '{}'", passed.display()),
            format!(
                "echo run >> '{}'; test -f '{}'",
                failed.display(),
                ready.display()
            ),
        ];
        let cancel = CancelToken::new();
        let run = || {
            run_verify_commands(
                CheckContext::new(&app, None).with_gate("final checks"),
                &repo,
                dir.path(),
                &commands,
                SandboxMode::Host,
                "base",
                &cancel,
            )
        };
        let first = run().await;
        assert_eq!(first[0].code, Some(0));
        assert_eq!(first[1].code, Some(1));
        std::fs::write(&ready, "").unwrap();
        let second = run().await;
        assert_eq!(second[0].ms, 0);
        assert!(second[0].decisions[0].contains("already passed on tree"));
        assert_eq!(second[1].code, Some(0));
        assert_eq!(std::fs::read_to_string(passed).unwrap().lines().count(), 1);
        assert_eq!(std::fs::read_to_string(failed).unwrap().lines().count(), 2);
    }

    #[tokio::test]
    async fn an_interrupted_check_records_the_tree_of_its_rerun() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let counter = dir.path().join("counter");
        let commands = vec![format!(
            "echo run >> '{}'; if [ \"$(cat README.md)\" = hello ]; then echo changed > README.md; exit 137; fi",
            counter.display()
        )];
        let cancel = CancelToken::new();
        let run = || {
            run_checks(
                &app,
                &repo,
                dir.path(),
                &commands,
                SandboxMode::Host,
                "base",
                &cancel,
            )
        };
        let (outcomes, lines) = run().await;
        assert_eq!(outcomes[0].code, Some(0));
        assert_eq!(lines.len(), 1);
        let (outcomes, lines) = run().await;
        assert_eq!(outcomes[0].ms, 0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("already passed on tree"));
        assert_eq!(std::fs::read_to_string(counter).unwrap().lines().count(), 2);
    }

    #[tokio::test]
    async fn a_held_out_decision_uses_its_label_and_hides_its_command() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let task = task_with_status(TaskStatus::Running);
        let cancel = CancelToken::new();
        let runner = CheckRunner::new(
            &app,
            &repo,
            dir.path(),
            SandboxMode::Host,
            &task.base_sha,
            &cancel,
        );
        let mut check = Check {
            criterion: 2,
            run: "true".into(),
            baseline: Some(Baseline::Fail),
        };
        let (_, _, lines) = run_held_out_cached(&runner, &task, &check).await;
        assert!(lines.is_empty());
        let (result, detail, lines) = run_held_out_cached(&runner, &task, &check).await;
        assert_eq!(result.command, "held-out check (criterion 2)");
        assert_eq!(result.ms, 0);
        assert!(detail.is_none());
        assert_eq!(lines.len(), 1);
        assert!(lines[0]
            .starts_with("Orchestrator: held-out check (criterion 2) already passed on tree "));
        assert!(!lines[0].contains(&check.run));
        check.run = "cat held.sh; exit 137".into();
        std::fs::write(repo.join("held.sh"), &check.run).unwrap();
        let (result, detail, lines) = run_held_out_cached(&runner, &task, &check).await;
        assert_eq!(result.code, Some(137));
        assert!(detail.is_some());
        assert!(!result.tail.contains(&check.run));
        assert_eq!(lines.len(), 1);
        assert!(lines[0]
            .starts_with("Orchestrator: held-out check (criterion 2) was interrupted after "));
        assert!(!lines[0].contains(&check.run));
        assert!(lines[0].contains("<held-out check>"));
    }

    #[test]
    fn the_pass_cache_keeps_at_most_512_entries_and_only_exit_0() {
        let mut cache = PassedChecks::default();
        for code in [None, Some(1), Some(124), Some(137), Some(143)] {
            cache.insert("failed", "base", &outcome(code, ""));
        }
        assert!(cache.entries.is_empty());
        for i in 0..512 {
            cache.insert(&format!("tree-{i}"), "base", &outcome(Some(0), ""));
        }
        cache.insert("tree-0", "other-base", &outcome(Some(0), ""));
        cache.insert("tree-512", "base", &outcome(Some(0), ""));
        assert_eq!(cache.entries.len(), 512);
        assert_eq!(cache.order.len(), 512);
        assert!(!cache.contains("tree-0", "check", "base"));
        assert!(cache.contains("tree-1", "check", "other-base"));
        let mut sensitive = outcome(Some(0), "");
        sensitive.command = "check $ORCHD_BASE_SHA".into();
        cache.insert("sensitive", "base-a", &sensitive);
        assert!(cache.contains("sensitive", &sensitive.command, "base-a"));
        assert!(!cache.contains("sensitive", &sensitive.command, "base-b"));
    }
}
