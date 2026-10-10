use super::*;

/// Options of the pre-existing final-check question, in the order shown.
pub(super) const PRE_EXISTING_RETRY: &str = "retry after the base is fixed";
pub(super) const PRE_EXISTING_DROP: &str = "drop all";
const PRE_EXISTING_OPTIONS: [&str; 3] = [PRE_EXISTING_RETRY, PRE_EXISTING_DROP, "stop"];

/// Options of the question about a parent's checks that fail on the base:
/// the subtasks are usually meant to make them pass.
pub(super) const PARENT_KEEP: &str = "keep all";
const PARENT_CHECK_OPTIONS: [&str; 3] = [PARENT_KEEP, PRE_EXISTING_DROP, "stop"];

/// Why a failing check is kept by rule: the files it names do not exist on
/// the base, and the plan creates them.
pub(super) const KEPT_BY_PLAN: &str = "targets absent on base; the plan creates them";
/// Why a failing check is kept by the owner's (or the policy's) `keep all`.
pub(super) const KEPT_BY_ANSWER: &str = "it fails on the base until the subtasks land";

/// The decision line that keeps a check although it fails on the base; it is
/// what stops a relaunch from asking again.
pub(super) fn kept_check_line(command: &str, reason: &str) -> String {
    format!("Orchestrator: kept check {command}, {reason}")
}

/// Whether `task` already keeps `command` although it fails on the base.
pub(super) fn is_kept(task: &Task, command: &str) -> bool {
    [KEPT_BY_PLAN, KEPT_BY_ANSWER]
        .iter()
        .any(|reason| task.decisions.contains(&kept_check_line(command, reason)))
}

/// The one owner question for every check that fails on `base_sha`: what
/// each of them printed is quoted, the commands are listed in `commands`.
fn base_check_question(failed: &[VerifyOutcome], base_sha: &str, options: [&str; 3]) -> Question {
    let sha: String = base_sha.chars().take(7).collect();
    let text = match failed {
        [one] => format!(
            "{} already fails on base {sha}: {}",
            one.command,
            tail_chars(one.tail.trim(), 300)
        ),
        many => {
            let names: Vec<&str> = many.iter().map(|f| f.command.as_str()).collect();
            let mut text = format!("{} already fail on base {sha}:", names.join(", "));
            for f in many {
                text.push_str(&format!(
                    "\n- {}: {}",
                    f.command,
                    tail_chars(f.tail.trim(), 200)
                ));
            }
            text
        }
    };
    let mut q = Question::new(
        text,
        options.iter().map(|o| o.to_string()).collect(),
        QuestionKind::PreexistingFailure,
        AskedBy::Verify,
    );
    q.commands = failed.iter().map(|f| f.command.clone()).collect();
    q
}

/// The owner question for final checks that fail on the base as well.
pub(super) fn pre_existing_question(failed: &[VerifyOutcome], base_sha: &str) -> Question {
    base_check_question(failed, base_sha, PRE_EXISTING_OPTIONS)
}

/// The owner question for a parent's checks that fail on the base: unlike a
/// final check on its own, a `verify` command is usually meant to turn green
/// through the work.
pub(super) fn parent_checks_question(failed: &[VerifyOutcome], base_sha: &str) -> Question {
    let mut q = base_check_question(failed, base_sha, PARENT_CHECK_OPTIONS);
    let (them, all) = if failed.len() == 1 {
        ("it", "this check")
    } else {
        ("them", "all")
    };
    q.text = format!(
        "{} The subtasks may be meant to make {them} pass: keep {all}, or drop {all}?",
        q.text.trim_end()
    );
    q
}

/// The path-like arguments of a check: a token that holds a `/`, a glob
/// character (`*`, `?`, `[`) or a file extension (`name.ext`). Quotes are
/// stripped; flags, URLs, `VAR=x` assignments, shell operators, absolute
/// paths and bare words (`npm`, `run`, `test`) are left out.
pub(super) fn check_targets(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut redirected = false;
    for token in command.split_whitespace() {
        // The file a `>` writes is not something the check reads.
        if std::mem::replace(&mut redirected, matches!(token, ">" | ">>")) {
            continue;
        }
        let token = token
            .trim_matches(|c| c == '"' || c == '\'')
            .trim_end_matches([';', ',', ')'])
            .trim_start_matches('(')
            .trim_matches(|c| c == '"' || c == '\'');
        if token.is_empty()
            || token.starts_with(['-', '/', '~', '$', '>', '<', '|', '&'])
            || token.contains("://")
            || token.contains('>')
            || token.contains('<')
            || is_assignment(token)
        {
            continue;
        }
        let has_ext = token.rsplit_once('.').is_some_and(|(name, ext)| {
            !name.is_empty()
                && !name.ends_with('/')
                && ext.len() <= 10
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
        });
        if token.contains(['/', '*', '?', '[']) || has_ext {
            let token = token.strip_prefix("./").unwrap_or(token).to_string();
            if !token.is_empty() && !out.contains(&token) {
                out.push(token);
            }
        }
    }
    out
}

/// Whether `target` (a file, a directory or a glob) names something in
/// `files`, the paths of a tree.
fn target_in_tree(target: &str, files: &[String]) -> bool {
    let dir = format!("{}/", target.trim_end_matches('/'));
    files
        .iter()
        .any(|f| f == target || f.starts_with(&dir) || glob_match(target, f))
}

/// The targets of `command` that are not in the tree of `base_sha`. Read with
/// `git ls-tree`, because the scratch worktree of a base run is gone by now.
/// Empty when the tree cannot be read: nothing is known to be absent.
pub(super) fn targets_absent_on_base(repo: &str, base_sha: &str, command: &str) -> Vec<String> {
    let targets = check_targets(command);
    if targets.is_empty() {
        return Vec::new();
    }
    let Ok(files) = git::tree_files(Path::new(repo), base_sha) else {
        return Vec::new();
    };
    targets
        .into_iter()
        .filter(|t| !target_in_tree(t, &files))
        .collect()
}

/// Whether `target` lies under `path` or matches it: `path` is a file, a
/// directory or a glob of the plan.
fn target_in_plan_path(target: &str, path: &str) -> bool {
    let path = path.strip_prefix("./").unwrap_or(path);
    let dir = format!("{}/", path.trim_end_matches('/'));
    target == path
        || target.starts_with(&dir)
        || glob_match(path, target)
        || glob_match(target, path)
}

/// The failing checks the plan is meant to turn green, kept by rule: a
/// command is kept when at least one of its targets is absent on the base
/// and the task has subtasks, or a target lies under the paths of the task
/// or of a subtask. One decision per kept check is written (once). This is
/// no answer: the answer policy's once-flag and cap do not apply, and no
/// check is removed.
pub(super) fn keep_planned_checks(app: &App, task: &mut Task, failing: &[String]) -> Vec<String> {
    if failing.is_empty() {
        return Vec::new();
    }
    let children: Vec<Task> = app
        .store
        .list_tasks()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.parent.as_deref() == Some(task.id.as_str()) && !t.archived)
        .collect();
    let paths: Vec<&String> = task
        .paths
        .iter()
        .chain(children.iter().flat_map(|c| c.paths.iter()))
        .collect();
    let mut kept = Vec::new();
    for command in failing {
        if targets_absent_on_base(&task.repo, &task.base_sha, command).is_empty() {
            continue;
        }
        let planned = !children.is_empty()
            || check_targets(command)
                .iter()
                .any(|t| paths.iter().any(|p| target_in_plan_path(t, p)));
        if !planned {
            continue;
        }
        let line = kept_check_line(command, KEPT_BY_PLAN);
        if !task.decisions.contains(&line) {
            task.decisions.push(line);
        }
        kept.push(command.clone());
    }
    kept
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

fn base_check_prefix(who: &str, command: &str, base_sha: &str) -> String {
    let sha: String = base_sha.chars().take(7).collect();
    format!("{who}: base check {command} on {sha} -> ")
}

/// The decision line recording an answer to a base-check question, by `who`
/// (`Owner` or `Orchestrator`).
pub(super) fn base_check_line(who: &str, command: &str, base_sha: &str, answer: &str) -> String {
    format!("{}{answer}", base_check_prefix(who, command, base_sha))
}

/// Whether the owner or the orchestrator agent already answered the
/// base-check question for this command on this base.
pub(super) fn base_check_answered(task: &Task, command: &str, base_sha: &str) -> bool {
    let prefixes = ["Owner", "Orchestrator"].map(|who| base_check_prefix(who, command, base_sha));
    task.decisions
        .iter()
        .any(|d| prefixes.iter().any(|p| d.starts_with(p)))
}

/// Records an answer to a base-check question about `commands`, asked on
/// `base_sha`, one line per command, unless the answer policy gave it (`policy_answered`). The line
/// says who answered, read from the question history entry the wait just
/// wrote: `Orchestrator:` for the agent, `Owner:` otherwise.
pub(super) fn record_owner_base_check(
    task: &mut Task,
    commands: &[String],
    base_sha: &str,
    answer: &str,
    policy_answered: bool,
) {
    if !policy_answered {
        if let Some(who) = answerer_label(task) {
            for command in commands {
                task.decisions
                    .push(base_check_line(who, command, base_sha, answer));
            }
        }
    }
}

/// Who gave the answer the wait just recorded: `Owner` (also when nothing
/// was recorded), `Orchestrator` for the agent, `None` for a rule or judge,
/// whose own line already says so.
pub(super) fn answerer_label(task: &Task) -> Option<&'static str> {
    match task.question_history.last().map(|h| h.answered_by) {
        Some(AnsweredBy::Orchestrator) => Some("Orchestrator"),
        Some(AnsweredBy::Policy | AnsweredBy::Judge) => None,
        Some(AnsweredBy::Owner) | None => Some("Owner"),
    }
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

/// The result of `command` on `base_sha`: the cache, else the in-flight
/// background job of the same pair, else a job started now (its run folders
/// under `run_dir`). `None` when the base cannot be checked out or the wait
/// was cancelled: nothing is known then. `background` picks the lane of a job
/// started here (see `start_base_jobs`).
pub(super) async fn base_result(
    app: &Arc<App>,
    repo: &str,
    base_sha: &str,
    command: &str,
    run_dir: &Path,
    cancel: &CancelToken,
    background: bool,
) -> Option<VerifyOutcome> {
    start_base_jobs(
        app,
        repo,
        base_sha,
        &[command.to_string()],
        run_dir,
        background,
    );
    wait_for_base(app, base_sha, command, cancel).await
}

/// The base's result for `failed`, when the same command also exits non-zero
/// there: the failure is older than this task. `None` for a command that
/// passes on base or a base that cannot be run. A final check: a job started
/// here runs in the background lane.
pub(super) async fn failing_on_base(
    app: &Arc<App>,
    task: &Task,
    failed: &VerifyOutcome,
    run_dir: &Path,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    let base = base_result(
        app,
        &task.repo,
        &task.base_sha,
        &failed.command,
        run_dir,
        cancel,
        true,
    )
    .await?;
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
    // A `verify` command: its lookups use the blocking lane, so a slow
    // background final of the same repo never holds them back.
    let on_new = base_result(
        app,
        &task.repo,
        &task.base_sha,
        &failed.command,
        &run_dir.join("new"),
        cancel,
        false,
    )
    .await
    .filter(|b| b.code.is_some_and(|c| c != 0))?;
    let on_old = base_result(
        app,
        &task.repo,
        old_base,
        &failed.command,
        &run_dir.join("old"),
        cancel,
        false,
    )
    .await?;
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
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
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
    task.question = Some(pre_existing_question(
        std::slice::from_ref(base_run),
        &base_sha,
    ));
    task.status = TaskStatus::Waiting;
    task.updated_at = now_ms();
    let assumed = task.assumptions.len();
    let Some(answer) = wait_for_answer(app, task_id, task, pending_answer, cancel, permit).await
    else {
        return BaseBreak::Stopped;
    };
    let policy_answered = task.assumptions.len() > assumed;
    record_owner_base_check(
        task,
        std::slice::from_ref(&command),
        &base_sha,
        &answer,
        policy_answered,
    );
    if is_option(&answer, PRE_EXISTING_DROP) {
        let settings = app.settings.read().unwrap().clone();
        let origin = origin_of_last_answer(task);
        match remove_check_command(&settings, task, &command, origin) {
            Ok(()) => {
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
            // Refused: the check stays and the attempt stays failed. The
            // answer counts as a retry on the newest base.
            Err(why) => task
                .decisions
                .push(format!("Orchestrator: check {command} stays: {why}")),
        }
    }
    let wt = PathBuf::from(&task.worktree);
    carry_onto_newest_base(task, &wt).await;
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    BaseBreak::Retry
}

/// Waits for the turn of `repo`'s base-check pairs; `None` when cancelled.
/// Two lanes per repo: blocking runs (a parent's `verify`, a carried
/// attempt's lookups) and background `finalVerify` jobs. Each lane runs one
/// pair at a time; a slow final never queues a blocking run behind it.
async fn lock_base_checks(
    app: &App,
    repo: &str,
    background: bool,
    cancel: &CancelToken,
) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    let key = if background {
        format!("{repo}\u{0}background")
    } else {
        repo.to_string()
    };
    let lock = app
        .base_check_locks
        .lock()
        .unwrap()
        .entry(key)
        .or_default()
        .clone();
    tokio::select! {
        guard = lock.lock_owned() => Some(guard),
        _ = cancel.cancelled() => None,
    }
}

/// Starts one background job for the pairs of `commands` on `base_sha` that
/// are neither cached nor in flight; their run folders go under `run_dir`
/// (`preflight-base`, `preflight-base-retry`). The job is the run plus the
/// rerun of a command that exits non-zero, past the cache and with no other
/// base-check pair of its repo beside it, so a load-flaky test does not
/// count. It runs under the app's own `base_cancel` (a task stop does not
/// end it: another task may wait on the same job), writes its results into
/// `base_runs` and leaves `base_jobs` when done. A cancelled job caches
/// nothing. `background` jobs (finals) take the repo's background lane,
/// the others the blocking lane.
pub(super) fn start_base_jobs(
    app: &Arc<App>,
    repo: &str,
    base_sha: &str,
    commands: &[String],
    run_dir: &Path,
    background: bool,
) {
    let mut todo: Vec<String> = Vec::new();
    let (tx, rx) = watch::channel(false);
    {
        // Jobs first, then the cache: a job caches before it leaves the
        // registry, so a pair is never missed in between.
        let mut jobs = app.base_jobs.lock().unwrap();
        let cache = app.base_runs.lock().unwrap();
        for c in commands {
            let key = (base_sha.to_string(), c.clone());
            if jobs.contains_key(&key) || cache.contains_key(&key) || todo.contains(c) {
                continue;
            }
            jobs.insert(key, rx.clone());
            todo.push(c.clone());
        }
    }
    if todo.is_empty() {
        return;
    }
    let (app, repo, base_sha, run_dir) = (
        app.clone(),
        repo.to_string(),
        base_sha.to_string(),
        run_dir.to_path_buf(),
    );
    // Leaves the registry and wakes the waiters even when the job panics.
    struct Leave {
        app: Arc<App>,
        base_sha: String,
        todo: Vec<String>,
        tx: watch::Sender<bool>,
    }
    impl Drop for Leave {
        fn drop(&mut self) {
            if self.app.base_cancel.is_cancelled() {
                let mut cache = self.app.base_runs.lock().unwrap();
                for c in &self.todo {
                    cache.remove(&(self.base_sha.clone(), c.clone()));
                }
            }
            let mut jobs = self.app.base_jobs.lock().unwrap();
            for c in &self.todo {
                jobs.remove(&(self.base_sha.clone(), c.clone()));
            }
            drop(jobs);
            let _ = self.tx.send(true);
        }
    }
    tokio::spawn(async move {
        let _leave = Leave {
            app: app.clone(),
            base_sha: base_sha.clone(),
            todo: todo.clone(),
            tx,
        };
        run_base_job(&app, &repo, &base_sha, &todo, &run_dir, background).await;
    });
}

async fn run_base_job(
    app: &Arc<App>,
    repo: &str,
    base_sha: &str,
    commands: &[String],
    dir: &Path,
    background: bool,
) {
    let cancel = app.base_cancel.clone();
    let Some(_alone) = lock_base_checks(app, repo, background, &cancel).await else {
        return;
    };
    let fails = |r: &VerifyOutcome| r.code.is_some_and(|c| c != 0);
    let Ok(first) = run_on_base(
        app,
        repo,
        base_sha,
        commands,
        &dir.join("preflight-base"),
        &cancel,
    )
    .await
    else {
        return;
    };
    let failed: Vec<String> = first
        .iter()
        .filter(|r| fails(r))
        .map(|r| r.command.clone())
        .collect();
    if failed.is_empty() || cancel.is_cancelled() {
        return;
    }
    {
        let mut cache = app.base_runs.lock().unwrap();
        for c in &failed {
            cache.remove(&(base_sha.to_string(), c.clone()));
        }
    }
    let Ok(second) = run_on_base(
        app,
        repo,
        base_sha,
        &failed,
        &dir.join("preflight-base-retry"),
        &cancel,
    )
    .await
    else {
        return;
    };
    let mut flaky = app.base_flaky.lock().unwrap();
    for c in &failed {
        if second.iter().any(|r| &r.command == c && !fails(r)) {
            flaky.insert((base_sha.to_string(), c.clone()));
        }
    }
}

/// Waits for the in-flight job of the pair (if any), then reads the cache.
/// `None`: the job was cancelled or could not check out the base, or `cancel`
/// fired while waiting.
async fn wait_for_base(
    app: &Arc<App>,
    base_sha: &str,
    command: &str,
    cancel: &CancelToken,
) -> Option<VerifyOutcome> {
    let key = (base_sha.to_string(), command.to_string());
    let job = app.base_jobs.lock().unwrap().get(&key).cloned();
    if let Some(mut rx) = job {
        tokio::select! {
            // An `Err` is a dropped sender: the job is over either way.
            _ = rx.wait_for(|done| *done) => {}
            _ = cancel.cancelled() => return None,
        }
    }
    app.base_runs.lock().unwrap().get(&key).cloned()
}

/// Records on `task` the commands whose first base run failed and whose
/// rerun passed: `base check <cmd> flaky on <sha>: passed on rerun`.
pub(super) fn note_flaky_base_checks(app: &App, task: &mut Task, commands: &[String]) {
    let flaky = app.base_flaky.lock().unwrap();
    for c in commands {
        if !flaky.contains(&(task.base_sha.clone(), c.clone())) {
            continue;
        }
        let line = format!(
            "Orchestrator: base check {c} flaky on {}: passed on rerun",
            short_sha(&task.base_sha)
        );
        if !task.decisions.contains(&line) {
            task.decisions.push(line);
        }
    }
}

/// Runs `checks` on the task's base, waiting for the result: every command
/// whose pair is not cached starts a background job (or joins the one in
/// flight). A command that exits non-zero is run once more by the job (the
/// task records `base check <cmd> flaky on <sha>: passed on rerun`); every
/// one that fails both times is returned. Empty when every command passes,
/// the base cannot be checked out, or the run was cancelled.
///
/// A parent passes its unkept `verify` commands here and asks first: a
/// `verify` command is usually the very test the work is meant to turn
/// green, so failing on the base is what it is for.
pub(super) async fn failing_twice_on_base(
    app: &Arc<App>,
    task: &mut Task,
    checks: &[String],
    cancel: &CancelToken,
) -> Vec<VerifyOutcome> {
    let commands = planned_base_commands(app, task, checks);
    if commands.is_empty() {
        return Vec::new();
    }
    let dir = app.store.task_dir(&task.id);
    start_base_jobs(app, &task.repo, &task.base_sha, &commands, &dir, false);
    let mut failing = Vec::new();
    for c in &commands {
        if let Some(r) = wait_for_base(app, &task.base_sha, c, cancel).await {
            if r.code.is_some_and(|code| code != 0) {
                failing.push(r);
            }
        }
    }
    if cancel.is_cancelled() {
        return Vec::new();
    }
    note_flaky_base_checks(app, task, &commands);
    failing
}

/// `checks` without duplicates, minus what the scoped-checks policy skips
/// for the task's planned paths (no diff yet: they stand in for it).
fn planned_base_commands(app: &App, task: &Task, checks: &[String]) -> Vec<String> {
    let mut commands: Vec<String> = Vec::new();
    for c in checks {
        if !commands.contains(c) {
            commands.push(c.clone());
        }
    }
    filter_scoped_planned(
        &app.settings.read().unwrap().scoped_checks,
        &task.repo,
        &commands,
        &task.paths,
    )
}

/// Starts the background base jobs of the task's `finalVerify` that are not
/// answered yet; the first attempt does not wait for them.
pub(super) fn start_final_base_jobs(app: &Arc<App>, task: &Task) {
    // A command deferred to the parent does not run here, so it has no base
    // run here either.
    let (finals, _) = {
        let settings = app.settings.read().unwrap().clone();
        split_deferred(app, &settings, task)
    };
    let unanswered: Vec<String> = finals
        .into_iter()
        .filter(|c| !is_kept(task, c) && !base_check_answered(task, c, &task.base_sha))
        .collect();
    let commands = planned_base_commands(app, task, &unanswered);
    if commands.is_empty() {
        return;
    }
    let dir = app.store.task_dir(&task.id);
    start_base_jobs(app, &task.repo, &task.base_sha, &commands, &dir, true);
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

    fn outcome(command: &str, code: i32) -> VerifyOutcome {
        VerifyOutcome {
            command: command.to_string(),
            code: Some(code),
            tail: String::new(),
            ms: 1,
        }
    }

    #[tokio::test]
    async fn a_pair_in_flight_is_waited_for_and_never_started_twice() {
        let (app, _dir) = crate::engine::test_support::test_app();
        let key = ("abc".to_string(), "npm run ci".to_string());
        let (tx, rx) = watch::channel(false);
        app.base_jobs.lock().unwrap().insert(key.clone(), rx);
        let cancel = CancelToken::new();
        let waiter = {
            let app = app.clone();
            let run_dir = app.data_dir.join("runs");
            tokio::spawn(async move {
                base_result(
                    &app,
                    "/no/such/repo",
                    "abc",
                    "npm run ci",
                    &run_dir,
                    &cancel,
                    true,
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!waiter.is_finished(), "it waits for the job");
        assert_eq!(app.base_jobs.lock().unwrap().len(), 1, "no second job");
        // The job ends: its result is cached first, then it leaves the registry.
        app.base_runs
            .lock()
            .unwrap()
            .insert(key.clone(), outcome("npm run ci", 1));
        app.base_jobs.lock().unwrap().remove(&key);
        tx.send(true).unwrap();
        let got = waiter.await.unwrap().expect("the job's result");
        assert_eq!(got.code, Some(1));
        assert!(app.base_jobs.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_cancelled_job_caches_nothing_and_its_waiters_get_base_unknown() {
        let (app, dir) = crate::engine::test_support::test_app();
        app.base_cancel.cancel();
        let run_dir = dir.path().join("runs");
        let got = base_result(
            &app,
            "/no/such/repo",
            "abc",
            "npm run ci",
            &run_dir,
            &CancelToken::new(),
            true,
        )
        .await;
        assert!(got.is_none());
        assert!(app.base_runs.lock().unwrap().is_empty());
        assert!(app.base_jobs.lock().unwrap().is_empty());
        let left = std::fs::read_dir(dir.path().join("data/base-runs")).map_or(0, |d| d.count());
        assert_eq!(left, 0, "no scratch worktree is left");
    }

    #[tokio::test]
    async fn base_check_pairs_of_one_repo_run_one_at_a_time() {
        let (app, dir) = crate::engine::test_support::test_app();
        let task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        let lock = app
            .base_check_locks
            .lock()
            .unwrap()
            .entry(task.repo.clone())
            .or_default()
            .clone();
        let _busy = lock.lock().await;
        let cancel = CancelToken::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            stopper.cancel();
        });
        let started = std::time::Instant::now();
        // Another pair of the same repo is running: this one waits for it
        // (here until it is cancelled) instead of running beside it.
        let mut task = task;
        let failed = failing_twice_on_base(&app, &mut task, &["true".to_string()], &cancel).await;
        assert!(failed.is_empty());
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(
            !dir.path().join("data/base-runs").exists(),
            "it never started"
        );
    }

    #[tokio::test]
    async fn a_refused_drop_leaves_the_check_and_the_failed_attempt_and_retries() {
        let (app, _dir) = crate::engine::test_support::test_app();
        app.settings.write().unwrap().answer_policy = false;
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        task.verify = vec!["npm run ci".into()];
        task.attempts = vec![crate::engine::test_support::attempt_with_failure(1, "x")];
        task.attempts[0].status = AttemptStatus::Failed;
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        let failed = VerifyOutcome {
            command: "npm run ci".into(),
            code: Some(1),
            tail: "boom".into(),
            ms: 1,
        };
        let pending: Arc<StdMutex<Option<oneshot::Sender<Reply>>>> = Arc::new(StdMutex::new(None));
        let cancel = CancelToken::new();
        let mut budget = 3u32;
        let mut permit = None;
        // An agent's "drop all" that got past the RPC gate (the
        // setting changed in between, say) is still refused by the loop.
        let deliver = async {
            loop {
                let tx = pending.lock().unwrap().take();
                if let Some(tx) = tx {
                    app.agent_answers.lock().unwrap().insert(id.clone());
                    tx.send(Reply {
                        text: PRE_EXISTING_DROP.to_string(),
                        default_evidence: None,
                    })
                    .ok();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        let (end, ()) = tokio::join!(
            wait_on_broken_base(
                &app,
                &id,
                &mut task,
                0,
                &failed,
                &failed,
                &mut budget,
                &pending,
                &cancel,
                &mut permit,
            ),
            deliver
        );
        assert!(matches!(end, BaseBreak::Retry));
        assert_eq!(task.verify, vec!["npm run ci".to_string()]);
        assert_eq!(task.attempts[0].status, AttemptStatus::Failed);
        assert!(task.attempts[0].failure.is_some());
        assert_ne!(task.status, TaskStatus::Running);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("check npm run ci stays")));
    }

    #[test]
    fn a_failing_final_whose_missing_target_the_plan_creates_is_kept_by_rule() {
        let (app, _dir) = crate::engine::test_support::test_app();
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q"]);
        std::fs::write(repo.path().join("a.txt"), "a").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        let sha = git(&["rev-parse", "HEAD"]);
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        task.repo = repo.path().to_string_lossy().to_string();
        task.base_sha = sha;
        task.paths = vec!["tests/new_check.sh".to_string()];
        let failing = vec![
            "sh tests/new_check.sh".to_string(),
            "sh tests/other.sh".to_string(),
        ];
        let kept = keep_planned_checks(&app, &mut task, &failing);
        assert_eq!(kept, vec!["sh tests/new_check.sh".to_string()]);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("sh tests/new_check.sh")));
    }

    #[test]
    fn options_match_ignoring_case_space_and_trailing_punctuation() {
        assert!(is_option("Drop all.", PRE_EXISTING_DROP));
        assert!(is_option("  DROP ALL! ", PRE_EXISTING_DROP));
        assert!(is_option("drop all", PRE_EXISTING_DROP));
        assert!(is_option("Stop?!", "stop"));
        assert!(!is_option("drop all please", PRE_EXISTING_DROP));
        assert!(!is_option("keep all", PRE_EXISTING_DROP));
    }

    #[test]
    fn an_answered_pair_is_found_by_command_and_short_base_sha() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        let sha = "0123456789abcdef";
        assert!(!base_check_answered(&task, "npm test", sha));
        let cmds = ["npm test".to_string()];
        record_owner_base_check(&mut task, &cmds, sha, "drop all", true);
        assert!(!base_check_answered(&task, "npm test", sha));
        record_owner_base_check(&mut task, &cmds, sha, "drop all", false);
        assert_eq!(
            task.decisions.last().unwrap(),
            "Owner: base check npm test on 0123456 -> drop all"
        );
        assert!(base_check_answered(&task, "npm test", sha));
        assert!(base_check_answered(&task, "npm test", "0123456ffff"));
        assert!(!base_check_answered(&task, "npm test", "fedcba9876"));
        assert!(!base_check_answered(&task, "npm run lint", sha));
    }

    #[test]
    fn an_orchestrator_answer_is_labelled_so_and_not_asked_again() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        let sha = "0123456789abcdef";
        let cmds = ["npm test".to_string()];
        let failed = VerifyOutcome {
            command: "npm test".into(),
            code: Some(1),
            tail: "tail".into(),
            ms: 1,
        };
        let q = pre_existing_question(&[failed], sha);
        crate::engine::answer_policy::record_answered_question(
            &mut task,
            &q,
            "keep all",
            AnsweredBy::Orchestrator,
        );
        record_owner_base_check(&mut task, &cmds, sha, "keep all", false);
        assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
        assert_eq!(
            task.decisions.last().unwrap(),
            "Orchestrator: base check npm test on 0123456 -> keep all"
        );
        assert!(base_check_answered(&task, "npm test", sha));
    }

    #[test]
    fn check_targets_finds_files_and_globs_only() {
        let t = |c: &str| check_targets(c);
        assert_eq!(
            t("node --test tests/a.test.cjs 'tests/b/*.cjs' \"src/c.ts\""),
            ["tests/a.test.cjs", "tests/b/*.cjs", "src/c.ts"]
        );
        assert_eq!(
            t("test -f ./Cargo.toml && grep -q x docs/[a-z]?.md"),
            ["Cargo.toml", "docs/[a-z]?.md"]
        );
        assert_eq!(t("cat build.log | wc -l > out.txt"), ["build.log"]);
        // Left out: flags, URLs, assignments, operators, bare words, versions.
        assert!(t("npm run test -- --reporter=dot -x").is_empty());
        assert!(t("cargo test -p sushiai-orch --test graph").is_empty());
        assert!(t("curl -fsS https://example.com/a/b.json").is_empty());
        assert!(t("CI=1 FILE=src/a.rs make check").is_empty());
        assert!(t("true && false || echo ok ; ls | cat < /dev/null").is_empty());
        assert!(t("node --version 18.2 2>/dev/null").is_empty());
    }

    #[test]
    fn a_target_is_present_by_path_directory_or_glob() {
        let files: Vec<String> = ["src/a.rs", "docs/guide/x.md"].map(String::from).to_vec();
        assert!(target_in_tree("src/a.rs", &files));
        assert!(target_in_tree("docs/guide", &files));
        assert!(target_in_tree("docs/guide/", &files));
        assert!(target_in_tree("src/*.rs", &files));
        assert!(!target_in_tree("src/b.rs", &files));
        assert!(!target_in_tree("docs/gui", &files));
        assert!(target_in_plan_path("src/new/a.rs", "src/new"));
        assert!(target_in_plan_path("src/new/a.rs", "src/**"));
        assert!(target_in_plan_path("src/new/*.rs", "src/new/a.rs"));
        assert!(!target_in_plan_path("tests/a.rs", "src/new"));
    }
}
