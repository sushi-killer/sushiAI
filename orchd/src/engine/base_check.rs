use super::*;

/// Options of the pre-existing final-check question, in the order shown.
pub(super) const PRE_EXISTING_RETRY: &str = "retry after the base is fixed";
pub(super) const PRE_EXISTING_DROP: &str = "drop this check";
const PRE_EXISTING_OPTIONS: [&str; 3] = [PRE_EXISTING_RETRY, PRE_EXISTING_DROP, "stop"];

/// Options of the question about a parent's `verify` command that fails on
/// the base: the subtasks are usually meant to make it pass.
pub(super) const PARENT_KEEP: &str = "keep this check";
const PARENT_CHECK_OPTIONS: [&str; 3] = [PARENT_KEEP, PRE_EXISTING_DROP, "stop"];
const ALREADY_FAILS: &str = " already fails on base ";

/// The owner question for a final check that fails on the base as well.
pub(super) fn pre_existing_question(command: &str, base_sha: &str, base_tail: &str) -> Question {
    let sha: String = base_sha.chars().take(7).collect();
    Question::new(
        format!(
            "{command} already fails on base {sha}: {}",
            tail_chars(base_tail.trim(), 300)
        ),
        PRE_EXISTING_OPTIONS.iter().map(|o| o.to_string()).collect(),
        QuestionKind::PreexistingFailure,
        AskedBy::Verify,
    )
}

/// The owner question for a parent's `verify` command that fails on the base:
/// unlike a final check, it is usually meant to turn green through the work.
pub(super) fn parent_check_question(command: &str, base_sha: &str, base_tail: &str) -> Question {
    let mut q = pre_existing_question(command, base_sha, base_tail);
    q.text = format!(
        "{} The subtasks may be meant to make it pass: keep this check, or drop it?",
        q.text.trim_end()
    );
    q.options = PARENT_CHECK_OPTIONS.iter().map(|o| o.to_string()).collect();
    q
}

/// Whether `answer` picks `option`: whitespace, trailing punctuation and
/// case do not matter, so `Drop this check.` is `drop this check`.
pub(super) fn is_option(answer: &str, option: &str) -> bool {
    answer
        .trim()
        .trim_end_matches(['.', ',', '!', '?', ';', ':'])
        .trim_end()
        .eq_ignore_ascii_case(option)
}

fn owner_base_check_prefix(command: &str, base_sha: &str) -> String {
    let sha: String = base_sha.chars().take(7).collect();
    format!("Owner: base check {command} on {sha} -> ")
}

/// The decision line recording an owner's answer to a base-check question.
pub(super) fn owner_base_check_line(command: &str, base_sha: &str, answer: &str) -> String {
    format!("{}{answer}", owner_base_check_prefix(command, base_sha))
}

/// Whether the owner already answered the base-check question for this
/// command on this base.
pub(super) fn owner_answered_base_check(task: &Task, command: &str, base_sha: &str) -> bool {
    let prefix = owner_base_check_prefix(command, base_sha);
    task.decisions.iter().any(|d| d.starts_with(&prefix))
}

/// Records an owner answer to a base-check question about `command`, asked
/// on `base_sha`, unless the answer policy gave it (`policy_answered`).
pub(super) fn record_owner_base_check(
    task: &mut Task,
    command: &str,
    base_sha: &str,
    answer: &str,
    policy_answered: bool,
) {
    if !policy_answered {
        task.decisions
            .push(owner_base_check_line(command, base_sha, answer));
    }
}

/// The command a pre-existing-failure question is about.
pub(super) fn pre_existing_command(question: &Question) -> Option<&str> {
    if question.kind != QuestionKind::PreexistingFailure {
        return None;
    }
    question
        .text
        .split_once(ALREADY_FAILS)
        .map(|(command, _)| command)
}

/// Runs `commands` on the commit `base_sha` of `repo`, in a throwaway
/// detached worktree that is removed again whatever happens (done, failed or
/// cancelled). Results are cached on the app per (base sha, command), so the
/// same command on an unchanged base runs once. `Err` only when the checkout
/// itself could not be made; the caller then knows nothing about the base.
pub(super) async fn run_on_base(
    app: &Arc<App>,
    repo: &str,
    base_sha: &str,
    commands: &[String],
    run_dir: &Path,
    cancel: &CancelToken,
) -> Result<Vec<VerifyOutcome>, String> {
    let mut known: HashMap<String, VerifyOutcome> = HashMap::new();
    let mut missing: Vec<String> = Vec::new();
    {
        let cache = app.base_runs.lock().unwrap();
        for c in commands {
            match cache.get(&(base_sha.to_string(), c.clone())) {
                Some(r) => {
                    known.insert(c.clone(), r.clone());
                }
                None if !missing.contains(c) => missing.push(c.clone()),
                None => {}
            }
        }
    }
    if !missing.is_empty() {
        let sandbox = app.settings.read().unwrap().sandbox;
        let scratch = app
            .data_dir
            .join("base-runs")
            .join(uuid::Uuid::new_v4().to_string());
        let wt = scratch.join("base");
        let (repo_path, sha, scratch2, wt2) = (
            PathBuf::from(repo),
            base_sha.to_string(),
            scratch.clone(),
            wt.clone(),
        );
        let setup = tokio::task::spawn_blocking(move || -> Result<(), String> {
            std::fs::create_dir_all(&scratch2).map_err(|e| e.to_string())?;
            git::add_detached_worktree(&repo_path, &wt2, &sha).map_err(|e| e.to_string())?;
            git::bootstrap_worktree(&repo_path, &wt2).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        let _ = std::fs::create_dir_all(run_dir);
        let ran = match setup {
            Ok(()) => {
                Ok(run_verify_commands(&wt, run_dir, &missing, sandbox, base_sha, cancel).await)
            }
            Err(e) => Err(format!("could not check out {base_sha}: {e}")),
        };
        let (repo_path, wt3) = (PathBuf::from(repo), wt.clone());
        let _ = tokio::task::spawn_blocking(move || {
            git::remove_worktree(&repo_path, &wt3);
            let _ = std::fs::remove_dir_all(&scratch);
        })
        .await;
        let results = ran?;
        let cancelled = cancel.is_cancelled();
        let mut cache = app.base_runs.lock().unwrap();
        for (c, r) in missing.iter().zip(results) {
            if !cancelled {
                cache.insert((base_sha.to_string(), c.clone()), r.clone());
            }
            known.insert(c.clone(), r);
        }
    }
    Ok(commands
        .iter()
        .filter_map(|c| known.get(c).cloned())
        .collect())
}

/// The base's result for `failed`, when the same command also exits non-zero
/// there: the failure is older than this task. `None` for a command that
/// passes on base or a base that cannot be run.
pub(super) async fn failing_on_base(
    app: &Arc<App>,
    task: &Task,
    failed: &VerifyOutcome,
    run_dir: &Path,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    let base = run_on_base(
        app,
        &task.repo,
        &task.base_sha,
        std::slice::from_ref(&failed.command),
        run_dir,
        cancel,
    )
    .await
    .ok()?
    .into_iter()
    .next()?;
    base.code.is_some_and(|c| c != 0).then_some(base)
}

/// A `verify` command that failed after the work was carried onto a moved
/// base: `Some` with the base's own result when the command fails on the new
/// base alone yet passed on the base the attempt started from, so the base
/// broke it and the attempt did not. A command that already failed on the
/// old base is a check the work is meant to turn green, never blamed on the
/// base. Cached per base commit like every base run.
pub(super) async fn broken_by_the_new_base(
    app: &Arc<App>,
    task: &Task,
    failed: &VerifyOutcome,
    old_base: &str,
    run_dir: &Path,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    let on_new = failing_on_base(app, task, failed, &run_dir.join("new"), cancel).await?;
    let on_old = run_on_base(
        app,
        &task.repo,
        old_base,
        std::slice::from_ref(&failed.command),
        &run_dir.join("old"),
        cancel,
    )
    .await
    .ok()?
    .into_iter()
    .next()?;
    (on_old.code == Some(0)).then_some(on_new)
}

/// What the owner decided about a `verify` command the new base broke.
pub(super) enum BaseBreak {
    /// The loop was cancelled or stopped while waiting.
    Stopped,
    /// Try again on the newest base: the attempt is not counted.
    Retry,
    /// The check is gone from `verify`; the attempt goes on without it.
    Dropped,
}

/// A `verify` command fails on the new base alone, so the attempt is not to
/// blame: the failure is recorded with a signature of its own and does not
/// count against `attempt_budget`, and the task waits for the owner (the
/// same question a final check that fails on the base gets). The detail
/// tells the next attempt not to fix what is out of its scope.
#[allow(clippy::too_many_arguments)]
pub(super) async fn wait_on_broken_base(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    failed: &VerifyOutcome,
    base_run: &VerifyOutcome,
    attempt_budget: &mut u32,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> BaseBreak {
    let command = failed.command.clone();
    let base_sha = task.base_sha.clone();
    task.decisions.push(format!(
        "Orchestrator: {command} fails on the base {} too",
        short_sha(&base_sha)
    ));
    let code = failed
        .code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "null".to_string());
    record_failure(
        task,
        idx,
        FailureKind::Verify,
        format!(
            "{command} exited {code}, and it fails on the base {} without this task's work: the base broke it, not this attempt. Do not fix it as part of this task.\n{}",
            short_sha(&base_sha),
            failed.tail
        ),
    );
    if let Some(f) = task.attempts[idx].failure.as_mut() {
        f.signature = format!("base:{command}");
    }
    *attempt_budget += 1;
    task.question = Some(pre_existing_question(&command, &base_sha, &base_run.tail));
    task.status = TaskStatus::Waiting;
    task.updated_at = now_ms();
    let assumed = task.assumptions.len();
    let Some(answer) = wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await
    else {
        return BaseBreak::Stopped;
    };
    let policy_answered = task.assumptions.len() > assumed;
    record_owner_base_check(task, &command, &base_sha, &answer, policy_answered);
    if is_option(&answer, PRE_EXISTING_DROP) {
        task.verify.retain(|c| c != &command);
        task.decisions
            .push(format!("Orchestrator: dropped check {command}"));
        app.verify_cache.lock().unwrap().remove(task_id);
        *attempt_budget -= 1;
        let a = &mut task.attempts[idx];
        a.status = AttemptStatus::Running;
        a.ended_at = None;
        a.failure = None;
        task.status = TaskStatus::Running;
        task.updated_at = now_ms();
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
        return BaseBreak::Dropped;
    }
    let wt = PathBuf::from(&task.worktree);
    carry_onto_newest_base(task, &wt).await;
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    BaseBreak::Retry
}

/// Runs `checks` on the task's base before its first implement attempt. A
/// command that exits non-zero is run once more on the base, past the cache,
/// so a load-flaky test does not count; the first one that fails both times
/// is returned. `None` when every command passes, the base cannot be checked
/// out, or the run was cancelled.
///
/// A single task passes only its `final_verify`: those are the whole-repo
/// checks that must already be green on the base. A `verify` command is
/// usually the very test the work is meant to turn green, so failing on the
/// base is what it is for. A parent also passes its `verify`, and asks first.
pub(super) async fn failing_twice_on_base(
    app: &Arc<App>,
    task: &Task,
    checks: &[String],
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    let mut commands: Vec<String> = Vec::new();
    for c in checks {
        if !commands.contains(c) {
            commands.push(c.clone());
        }
    }
    // No diff yet: the task's planned paths stand in for it.
    let commands = filter_scoped_planned(
        &app.settings.read().unwrap().scoped_checks,
        &task.repo,
        &commands,
        &task.paths,
    );
    if commands.is_empty() {
        return None;
    }
    let dir = app.store.task_dir(&task.id);
    let fails = |r: &VerifyOutcome| r.code.is_some_and(|c| c != 0);
    let first = run_on_base(
        app,
        &task.repo,
        &task.base_sha,
        &commands,
        &dir.join("preflight-base"),
        cancel,
    )
    .await
    .ok()?;
    let failed: Vec<String> = first
        .iter()
        .filter(|r| fails(r))
        .map(|r| r.command.clone())
        .collect();
    if failed.is_empty() || cancel.is_cancelled() {
        return None;
    }
    {
        let mut cache = app.base_runs.lock().unwrap();
        for c in &failed {
            cache.remove(&(task.base_sha.clone(), c.clone()));
        }
    }
    let second = run_on_base(
        app,
        &task.repo,
        &task.base_sha,
        &failed,
        &dir.join("preflight-base-retry"),
        cancel,
    )
    .await
    .ok()?;
    if cancel.is_cancelled() {
        return None;
    }
    second.into_iter().find(fails)
}

/// Baselines the checks that have none yet on the base commit (not in the
/// task's worktree, which may hold the implementer's changes). `false` when
/// cancelled or the base could not be checked out; nothing is stored then.
pub(super) async fn baseline_on_base(
    app: &Arc<App>,
    task: &mut Task,
    cancel: &CancelToken,
) -> bool {
    let todo = baseline_todo(task);
    if todo.is_empty() {
        return true;
    }
    let commands: Vec<String> = todo.iter().map(|(_, _, run)| run.clone()).collect();
    let run_dir = app.store.task_dir(&task.id).join("baseline-base");
    match run_on_base(app, &task.repo, &task.base_sha, &commands, &run_dir, cancel).await {
        Ok(results) if results.len() == todo.len() && !cancel.is_cancelled() => {
            record_baselines(task, &todo, &results)
        }
        _ => false,
    }
}

/// What `task.amend` changes; a `None` field is left as it is.
#[derive(Debug, Clone, Default)]
pub(super) struct Amendment {
    pub criteria: Option<Vec<String>>,
    /// The `criteria` texts flagged visual; only meaningful when `criteria` is set.
    pub visual_criteria: Vec<String>,
    pub verify: Option<Vec<String>>,
    pub final_verify: Option<Vec<String>>,
    /// The screenshot command; an empty string removes it.
    pub screenshot: Option<String>,
    pub checks: Option<Vec<Check>>,
    pub held_out: Option<Option<Check>>,
}

impl Amendment {
    pub fn is_empty(&self) -> bool {
        self.fields().is_empty()
    }

    /// The camelCase names of the fields it changes, in a fixed order.
    pub fn fields(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.criteria.is_some() {
            names.push("criteria");
        }
        if self.verify.is_some() {
            names.push("verify");
        }
        if self.final_verify.is_some() {
            names.push("finalVerify");
        }
        if self.screenshot.is_some() {
            names.push("screenshot");
        }
        if self.checks.is_some() {
            names.push("checks");
        }
        if self.held_out.is_some() {
            names.push("heldOut");
        }
        names
    }

    /// A later amendment wins field by field.
    pub fn merge(&mut self, later: Amendment) {
        if later.criteria.is_some() {
            self.criteria = later.criteria;
            self.visual_criteria = later.visual_criteria;
        }
        self.verify = later.verify.or(self.verify.take());
        self.final_verify = later.final_verify.or(self.final_verify.take());
        self.screenshot = later.screenshot.or(self.screenshot.take());
        self.checks = later.checks.or(self.checks.take());
        self.held_out = later.held_out.or(self.held_out.take());
    }

    /// Replaces the fields on `task` and adds the decision line: field
    /// names only, so it can never carry a command or a held-out check.
    pub fn apply(self, task: &mut Task) {
        let line = format!("Amended: {}", self.fields().join(", "));
        if let Some(v) = self.criteria {
            task.criteria = v;
            task.visual_criteria = self.visual_criteria;
        }
        if let Some(v) = self.verify {
            task.verify = v;
        }
        if let Some(v) = self.final_verify {
            task.final_verify = v;
        }
        if let Some(v) = self.screenshot {
            let v = v.trim();
            task.brief_check.screenshot = (!v.is_empty()).then(|| v.to_string());
        }
        let len = task.criteria.len();
        if let Some(v) = self.checks {
            task.checks = valid_checks(v, len);
        }
        if let Some(h) = self.held_out {
            task.held_out = valid_check(h, len);
        }
        task.decisions.push(line);
        task.updated_at = now_ms();
    }
}

/// Hands a pending amendment to the loop's own copy of the task: applied,
/// baselined on the base commit with `variant.groundedChecks`, and saved.
/// Called only at attempt boundaries, never while a harness runs.
pub(super) async fn apply_pending_amendment(
    app: &Arc<App>,
    task: &mut Task,
    pending: &Arc<StdMutex<Option<Amendment>>>,
    cancel: &CancelToken,
) {
    let Some(amendment) = pending.lock().unwrap().take() else {
        return;
    };
    if amendment.verify.is_some() {
        // Cached results belong to the old commands.
        app.verify_cache.lock().unwrap().remove(&task.id);
    }
    amendment.apply(task);
    if task.variant().grounded_checks {
        baseline_on_base(app, task, cancel).await;
    }
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_match_ignoring_case_space_and_trailing_punctuation() {
        assert!(is_option("Drop this check.", PRE_EXISTING_DROP));
        assert!(is_option("  DROP THIS CHECK! ", PRE_EXISTING_DROP));
        assert!(is_option("drop this check", PRE_EXISTING_DROP));
        assert!(is_option("Stop?!", "stop"));
        assert!(!is_option("drop this check please", PRE_EXISTING_DROP));
        assert!(!is_option("keep this check", PRE_EXISTING_DROP));
    }

    #[test]
    fn an_answered_pair_is_found_by_command_and_short_base_sha() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        let sha = "0123456789abcdef";
        assert!(!owner_answered_base_check(&task, "npm test", sha));
        record_owner_base_check(&mut task, "npm test", sha, "drop this check", true);
        assert!(!owner_answered_base_check(&task, "npm test", sha));
        record_owner_base_check(&mut task, "npm test", sha, "drop this check", false);
        assert_eq!(
            task.decisions.last().unwrap(),
            "Owner: base check npm test on 0123456 -> drop this check"
        );
        assert!(owner_answered_base_check(&task, "npm test", sha));
        assert!(owner_answered_base_check(&task, "npm test", "0123456ffff"));
        assert!(!owner_answered_base_check(&task, "npm test", "fedcba9876"));
        assert!(!owner_answered_base_check(&task, "npm run lint", sha));
    }
}
