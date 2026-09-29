use super::*;

/// Whether the wait-for graph has a cycle; `edges[a]` lists what `a` waits
/// for.
pub(super) fn has_cycle(edges: &HashMap<String, Vec<String>>) -> bool {
    fn visit<'a>(
        node: &'a str,
        edges: &'a HashMap<String, Vec<String>>,
        state: &mut HashMap<&'a str, bool>,
    ) -> bool {
        match state.get(node) {
            Some(true) => return true,
            Some(false) => return false,
            None => {}
        }
        state.insert(node, true);
        for next in edges.get(node).into_iter().flatten() {
            if visit(next, edges, state) {
                return true;
            }
        }
        state.insert(node, false);
        false
    }
    // `true` = on the current path, `false` = finished without a cycle.
    let mut state: HashMap<&str, bool> = HashMap::new();
    edges.keys().any(|n| visit(n, edges, &mut state))
}

/// A task waits for its dependencies; a parent also waits for each child.
pub(super) fn wait_edges(tasks: &[Task]) -> HashMap<String, Vec<String>> {
    let mut edges: HashMap<String, Vec<String>> = HashMap::new();
    for t in tasks {
        edges
            .entry(t.id.clone())
            .or_default()
            .extend(t.depends_on.iter().cloned());
        if let Some(parent) = &t.parent {
            edges.entry(parent.clone()).or_default().push(t.id.clone());
        }
    }
    edges
}

/// What `task` waits for among `all` (its repo's tasks): the dependencies
/// that still exist, and for a parent its children that are not archived.
fn waits_for<'a>(task: &Task, all: &'a [Task]) -> Vec<&'a Task> {
    all.iter()
        .filter(|t| {
            task.depends_on.contains(&t.id)
                || (t.parent.as_deref() == Some(task.id.as_str()) && !t.archived)
        })
        .collect()
}

pub(super) fn is_parent(task: &Task, all: &[Task]) -> bool {
    all.iter()
        .any(|t| t.parent.as_deref() == Some(task.id.as_str()))
}

#[derive(Debug, PartialEq)]
pub(super) enum Waits {
    /// Waits for nothing.
    Nothing,
    Ready,
    Pending,
    /// These ended `failed` or `stopped` and will not finish on their own.
    Ended(Vec<String>),
}

pub(super) fn waits_state(task: &Task, all: &[Task]) -> Waits {
    state_of(waits_for(task, all))
}

/// Like `waits_state` but only for the task's own `dependsOn`, not for its
/// children: what a parent must wait for before its children may start.
pub(super) fn own_waits_state(task: &Task, all: &[Task]) -> Waits {
    state_of(
        all.iter()
            .filter(|t| task.depends_on.contains(&t.id))
            .collect(),
    )
}

fn state_of(waits: Vec<&Task>) -> Waits {
    if waits.is_empty() {
        return Waits::Nothing;
    }
    let ended: Vec<String> = waits
        .iter()
        .filter(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Stopped))
        .map(|t| t.id.clone())
        .collect();
    if !ended.is_empty() {
        Waits::Ended(ended)
    } else if waits.iter().all(|t| t.status == TaskStatus::Done) {
        Waits::Ready
    } else {
        Waits::Pending
    }
}

/// Asked when something a task waits for ended `failed`/`stopped`; its
/// answer is handled by `answer_dependency_question`, not the attempt loop.
pub(super) const DEPENDENCY_QUESTION: &str = "A task this one waits for ended";

fn status_word(status: TaskStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// A parent's cost: its own runs plus every child's total.
fn parent_cost(parent: &Task, all: &[Task]) -> f64 {
    let own: f64 = parent
        .attempts
        .iter()
        .map(|a| a.cost_usd.unwrap_or(0.0) + a.review_cost_usd.unwrap_or(0.0))
        .sum();
    own + all
        .iter()
        .filter(|t| t.parent.as_deref() == Some(parent.id.as_str()))
        .map(|t| t.cost_usd)
        .sum::<f64>()
}

/// Undoes a split that could not create every part: removes each child
/// already created (its record, worktree and branch). None has a loop or
/// has been broadcast yet, so nothing else has seen them.
pub(super) async fn discard_children(app: &Arc<App>, children: Vec<Task>) {
    for child in children {
        let _ = std::fs::remove_dir_all(app.store.task_dir(&child.id));
        let _ = tokio::task::spawn_blocking(move || {
            git::discard_worktree(
                Path::new(&child.repo),
                Path::new(&child.worktree),
                &child.branch,
            )
        })
        .await;
    }
}

/// A parent's loop: starts its children that have not started yet, and once
/// every child has landed runs its own checks on its branch and ends `done`
/// (`failed` when a check fails). Until then it leaves the task `running`
/// with no loop; `advance_graph` comes back to it.
pub(super) async fn run_parent(app: &Arc<App>, task_id: &str, cancel: &CancelToken) {
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return;
    };
    let all = app.repo_tasks(&task.repo);
    // No child starts before the parent's own dependencies are done.
    match own_waits_state(&task, &all) {
        Waits::Ready | Waits::Nothing => {}
        Waits::Ended(ended) => {
            app.ask_dependency_question(&task, &all, &ended);
            return;
        }
        Waits::Pending => {
            if task.status != TaskStatus::Queued {
                task.status = TaskStatus::Queued;
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
            }
            return;
        }
    }
    // Before the first child starts: the parent's checks judge the base, not
    // the children's work. Idempotent across relaunches.
    if task.variant().grounded_checks && needs_baseline(&task) {
        let wt = PathBuf::from(&task.worktree);
        if !baseline_checks(app, task_id, &mut task, &wt, cancel).await {
            mark_stopped_if_not_already(app, task_id).await;
            return;
        }
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);
    }
    // `advance_graph` only relaunches a queued or running parent, so a
    // stopped or failed one here is the owner's own restart: its unfinished
    // children restart with it.
    let restarted = matches!(task.status, TaskStatus::Stopped | TaskStatus::Failed);
    for child in all
        .iter()
        .filter(|t| t.parent.as_deref() == Some(task_id) && !t.archived)
    {
        match child.status {
            TaskStatus::Drafting | TaskStatus::Queued => {
                app.spawn_task_loop(child.id.clone(), true);
            }
            TaskStatus::Stopped | TaskStatus::Failed if restarted => {
                // Queued before its loop runs, so this parent never reads
                // it as ended.
                let mut child = child.clone();
                child.question = None;
                child.status = TaskStatus::Queued;
                child.updated_at = now_ms();
                let _ = app.store.save_task(&child);
                app.broadcast_task(&child);
                app.spawn_task_loop(child.id.clone(), true);
            }
            _ => {}
        }
    }
    let all = if restarted {
        app.repo_tasks(&task.repo)
    } else {
        all
    };
    if !matches!(waits_state(&task, &all), Waits::Ready | Waits::Nothing) {
        if task.status != TaskStatus::Running {
            task.status = TaskStatus::Running;
            task.updated_at = now_ms();
            let _ = app.store.save_task(&task);
            app.broadcast_task(&task);
        }
        return;
    }

    let permit = tokio::select! {
        _ = cancel.cancelled() => None,
        p = app.slots.clone().acquire_owned() => p.ok(),
    };
    if permit.is_none() {
        mark_stopped_if_not_already(app, task_id).await;
        return;
    }
    task.status = TaskStatus::Running;
    task.updated_at = now_ms();
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);

    let checks: Vec<String> = task
        .verify
        .iter()
        .chain(&task.final_verify)
        .cloned()
        .collect();
    let sandbox = app.settings.read().unwrap().sandbox;
    let results = run_verify_commands(
        Path::new(&task.worktree),
        &app.store.task_dir(task_id).join("runs").join("parent"),
        &checks,
        sandbox,
        &task.base_sha,
        cancel,
    )
    .await;
    // Once, after verify and finalVerify: the gated checks, then the held-out
    // one. The line for a failure names its kind and criterion, and never the
    // held-out command.
    let mut grounded_failure: Option<String> = None;
    if task.variant().grounded_checks && results.iter().all(|v| v.code == Some(0)) {
        let dir = app.store.task_dir(task_id).join("runs").join("parent");
        let wt = Path::new(&task.worktree);
        let gated = gated_checks(&task);
        let commands: Vec<String> = gated.iter().map(|c| c.run.clone()).collect();
        let gated_results =
            run_verify_commands(wt, &dir, &commands, sandbox, &task.base_sha, cancel).await;
        if let Some((check, failed)) = gated
            .iter()
            .zip(&gated_results)
            .find(|(_, v)| v.code != Some(0))
        {
            grounded_failure = Some(format!(
                "Orchestrator: every subtask landed, but the check for criterion {} failed (verify) on {}: `{}` exited {}: {}",
                check.criterion,
                task.branch,
                failed.command,
                failed
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                tail_chars(failed.tail.trim(), 600)
            ));
        } else if let Some(held) = gated_held_out(&task) {
            let (result, detail) = run_held_out(&task, &held, wt, &dir, sandbox, cancel).await;
            if detail.is_some() {
                grounded_failure = Some(format!(
                    "Orchestrator: every subtask landed, but the held-out check for criterion {} failed (heldout) on {}: {}",
                    held.criterion,
                    task.branch,
                    tail_chars(result.tail.trim(), 600)
                ));
            }
        }
    }
    if cancel.is_cancelled() {
        mark_stopped_if_not_already(app, task_id).await;
        return;
    }
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return;
    };
    task.cost_usd = parent_cost(&task, &app.repo_tasks(&task.repo));
    match results.iter().find(|v| v.code != Some(0)) {
        Some(failed) => {
            task.decisions.push(format!(
                "Orchestrator: every subtask landed, but `{}` exited {} on {}: {}",
                failed.command,
                failed
                    .code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "null".to_string()),
                task.branch,
                tail_chars(failed.tail.trim(), 600)
            ));
            task.status = TaskStatus::Failed;
        }
        None => match grounded_failure {
            Some(line) => {
                task.decisions.push(line);
                task.status = TaskStatus::Failed;
            }
            None => {
                task.decisions.push(format!(
                    "Orchestrator: every subtask landed on {}",
                    task.branch
                ));
                task.status = TaskStatus::Done;
            }
        },
    }
    if task.status == TaskStatus::Done {
        task.updated_at = now_ms();
        app.write_report(&mut task).await;
        app.release_worktree(&mut task, "every subtask landed")
            .await;
    }
    task.updated_at = now_ms();
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);
}

/// The carry-onto-moved-base failure detail the agent gets: which files
/// conflict and how to resolve them.
pub(super) fn conflict_detail(
    base_ref: &str,
    new_sha: &str,
    files: &[String],
    task_id: &str,
) -> String {
    format!(
        "{base_ref} moved ahead to {}, and your changes were carried onto it. \
         These files conflict: {}. Text files carry <<<<<<< / >>>>>>> markers; \
         for a binary or deleted file, your version is `git show {}:<path>`. \
         Resolve each one so both the base's change and yours survive, then finish the task.",
        short_sha(new_sha),
        files.join(", "),
        git::wip_ref(task_id)
    )
}

pub(super) enum Landing {
    /// Committed and fast-forwarded onto the parent's branch (or the parent
    /// already held the work).
    Landed,
    /// Back to the agent as an ordinary failure; it retries on the new base.
    Failed {
        kind: FailureKind,
        detail: String,
    },
    Cancelled,
    /// The parent is gone, or landing on the base was refused: commit like a
    /// task without one.
    NoParent,
    /// The base branch's checkout has uncommitted changes: the task waits
    /// `landing` and is retried.
    Waiting,
}

/// Lands a finished child on its parent's branch, one child per parent at a
/// time: its work is carried onto the parent's current head (earlier
/// siblings may have landed since it started), verify runs again when the
/// head moved, then the child commits and the parent's branch
/// fast-forwards to that commit.
#[allow(clippy::too_many_arguments)]
pub(super) async fn land_on_parent(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    attempt_n: u32,
    worktree: &Path,
    run_dir: &Path,
    parent_id: &str,
    cancel: &CancelToken,
) -> Landing {
    let lock = app.landing_lock(parent_id);
    let _landing = tokio::select! {
        _ = cancel.cancelled() => return Landing::Cancelled,
        guard = lock.lock_owned() => guard,
    };
    let Ok(Some(parent)) = app.store.load_task(parent_id) else {
        return Landing::NoParent;
    };
    let branch = parent.branch.clone();
    let failed = |kind, detail| Landing::Failed { kind, detail };

    match carry_and_check(
        app,
        task,
        idx,
        worktree,
        run_dir,
        &branch,
        "Another subtask",
        &[],
        cancel,
    )
    .await
    {
        Ok(true) => return Landing::Landed,
        Ok(false) => {}
        Err(landing) => return landing,
    }

    let (wt, parent_wt, title, tid, base) = (
        worktree.to_path_buf(),
        PathBuf::from(&parent.worktree),
        task.title.clone(),
        task.id.clone(),
        task.base_sha.clone(),
    );
    let landed = tokio::task::spawn_blocking(move || {
        git::commit(&wt, &title, &tid, attempt_n)?;
        let sha = git::head_sha(&wt)?;
        if let Err(e) = git::fast_forward(&parent_wt, &sha) {
            // Keep the work, uncommitted, for the next attempt to carry.
            let _ = git::uncommit_to(&wt, &base);
            return Err(e);
        }
        Ok(sha)
    })
    .await
    .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
    match landed {
        Ok(sha) => {
            task.decisions
                .push(format!("Land: landed on {branch} at {}", short_sha(&sha)));
            Landing::Landed
        }
        Err(e) => failed(
            FailureKind::Error,
            format!("Could not land on {branch}: {e}"),
        ),
    }
}

/// Carries the task's work onto `branch`'s current head; when the head had
/// moved, `verify` and then `extra_final` run again on the carried tree.
/// `Ok(true)`: the branch already holds this work. `Err` is what the caller
/// returns: a failed attempt, or a cancel.
#[allow(clippy::too_many_arguments)]
pub(super) async fn carry_and_check(
    app: &Arc<App>,
    task: &mut Task,
    idx: usize,
    worktree: &Path,
    run_dir: &Path,
    branch: &str,
    first: &str,
    extra_final: &[String],
    cancel: &CancelToken,
) -> Result<bool, Landing> {
    let failed = |kind, detail| Err(Landing::Failed { kind, detail });
    let branch = branch.to_string();
    let (wt, base, tid, b) = (
        worktree.to_path_buf(),
        task.base_sha.clone(),
        task.id.clone(),
        branch.clone(),
    );
    let carried = tokio::task::spawn_blocking(move || {
        // Commits the agent made itself go back to being uncommitted work,
        // so they are carried (and later committed) like the rest.
        if git::head_sha(&wt)? != base {
            git::uncommit_to(&wt, &base)?;
        }
        git::carry_onto_moved_base(&wt, &b, &base, &tid)
    })
    .await
    .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
    match carried {
        Ok(git::Rebase::Unchanged) => {}
        Ok(git::Rebase::Moved { new_sha }) => {
            task.decisions.push(format!(
                "Land: carried the work onto {branch} at {}",
                short_sha(&new_sha)
            ));
            task.base_sha = new_sha.clone();
            let (wt, b) = (worktree.to_path_buf(), new_sha.clone());
            let changed = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &b).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            task.attempts[idx].changed_files = changed.clone();
            if changed.is_empty() {
                task.decisions
                    .push(format!("Land: {branch} already contains this work"));
                return Ok(true);
            }
            let results = run_verify_cached(
                app,
                &task.id,
                worktree,
                run_dir,
                &new_sha,
                &task.verify,
                cancel,
            )
            .await;
            if cancel.is_cancelled() {
                return Err(Landing::Cancelled);
            }
            task.attempts[idx].verify = results.clone();
            if let Some(v) = results.iter().find(|v| v.code != Some(0)) {
                return failed(
                    FailureKind::Verify,
                    format!(
                        "{first} landed on {branch} first; carried onto it, {} exited {}.\n{}",
                        v.command,
                        v.code
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| "null".to_string()),
                        v.tail
                    ),
                );
            }
            if !extra_final.is_empty() {
                let sandbox = app.settings.read().unwrap().sandbox;
                let finals = run_verify_commands(
                    worktree,
                    &run_dir.join("final-land"),
                    extra_final,
                    sandbox,
                    &task.base_sha,
                    cancel,
                )
                .await;
                if cancel.is_cancelled() {
                    return Err(Landing::Cancelled);
                }
                task.attempts[idx].verify.extend(finals.iter().cloned());
                if let Some(v) = finals.iter().find(|v| v.code != Some(0)) {
                    return failed(
                        FailureKind::Verify,
                        format!(
                            "{first} landed on {branch} first; carried onto it, final check {} exited {}.\n{}",
                            v.command,
                            v.code
                                .map(|c| c.to_string())
                                .unwrap_or_else(|| "null".to_string()),
                            v.tail
                        ),
                    );
                }
            }
        }
        Ok(git::Rebase::Conflicts { new_sha, files }) => {
            task.base_sha = new_sha.clone();
            return failed(
                FailureKind::Verify,
                conflict_detail(&branch, &new_sha, &files, &task.id),
            );
        }
        Ok(git::Rebase::Skipped { reason }) => {
            return failed(
                FailureKind::Error,
                format!("Could not carry the work onto {branch} ({reason})."),
            );
        }
        Err(e) => {
            return failed(
                FailureKind::Error,
                format!("Could not carry the work onto {branch} ({e})."),
            );
        }
    }
    Ok(false)
}

impl App {
    pub(super) fn landing_lock(&self, parent_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.landing_locks
            .lock()
            .unwrap()
            .entry(parent_id.to_string())
            .or_default()
            .clone()
    }

    pub(super) fn repo_tasks(&self, repo: &str) -> Vec<Task> {
        let mut tasks = self.store.list_tasks().unwrap_or_default();
        tasks.retain(|t| t.repo == repo);
        tasks
    }

    /// Moves a repo's task graph along after something in it changed: a
    /// queued task (or a running parent) with no live loop starts once all
    /// it waits for is done, and waits for the owner when one of those ended
    /// `failed`/`stopped`. Tasks outside any graph are never touched.
    pub(super) fn advance_graph(&self, repo: &str) {
        if self.shutting_down.load(Ordering::SeqCst) {
            return;
        }
        let all = self.repo_tasks(repo);
        for task in all.iter().filter(|t| !t.archived) {
            let parent = is_parent(task, &all);
            let asking = task.status == TaskStatus::Waiting
                && task
                    .question
                    .as_ref()
                    .is_some_and(|q| q.text.starts_with(DEPENDENCY_QUESTION));
            let idle = task.status == TaskStatus::Queued
                || (parent && task.status == TaskStatus::Running)
                || asking;
            if !idle || self.controls.lock().unwrap().contains_key(&task.id) {
                continue;
            }
            // A subtask never starts while its parent still waits for the
            // parent's own dependencies.
            if let Some(p) = task
                .parent
                .as_deref()
                .and_then(|pid| all.iter().find(|t| t.id == pid))
            {
                if !matches!(own_waits_state(p, &all), Waits::Ready | Waits::Nothing) {
                    continue;
                }
            }
            let in_graph = parent || task.parent.is_some() || !task.depends_on.is_empty();
            let state = waits_state(task, &all);
            // A parent whose children were only just drafted has to start
            // them itself once its own dependencies are done.
            let unstarted_children = parent
                && matches!(own_waits_state(task, &all), Waits::Ready | Waits::Nothing)
                && all.iter().any(|t| {
                    t.parent.as_deref() == Some(task.id.as_str())
                        && !t.archived
                        && t.status == TaskStatus::Drafting
                });
            match state {
                // Also picks up a task restarted while its previous loop was
                // still winding down (that spawn was a no-op), and a
                // dependent whose question is moot because everything it
                // waited for finished meanwhile.
                Waits::Ready | Waits::Nothing if in_graph => {
                    if asking {
                        let mut task = task.clone();
                        task.question = None;
                        task.status = TaskStatus::Queued;
                        task.updated_at = now_ms();
                        let _ = self.store.save_task(&task);
                        self.broadcast_task(&task);
                    }
                    self.spawn_task_loop(task.id.clone(), true)
                }
                // Everything that had ended was restarted and still runs:
                // the question is moot, the task just waits again.
                Waits::Pending if asking => {
                    let mut task = task.clone();
                    task.question = None;
                    task.status = TaskStatus::Queued;
                    task.updated_at = now_ms();
                    let _ = self.store.save_task(&task);
                    self.broadcast_task(&task);
                    if unstarted_children {
                        self.spawn_task_loop(task.id.clone(), true)
                    }
                }
                Waits::Pending if unstarted_children => self.spawn_task_loop(task.id.clone(), true),
                Waits::Ended(ended) => {
                    if !asking {
                        self.ask_dependency_question(task, &all, &ended);
                    }
                }
                _ => {}
            }
        }
    }

    /// Parks `task` `waiting` on the question of what to do about the
    /// dependencies in `ended`.
    fn ask_dependency_question(&self, task: &Task, all: &[Task], ended: &[String]) {
        let names: Vec<String> = all
            .iter()
            .filter(|t| ended.contains(&t.id))
            .map(|t| format!("\"{}\" ({})", t.title, status_word(t.status)))
            .collect();
        let mut task = task.clone();
        task.question = Some(Question {
            text: format!(
                "{DEPENDENCY_QUESTION}: {}. Retry it, drop it from what this task waits for, or stop?",
                names.join(", ")
            ),
            options: vec![
                "retry the dependency".into(),
                "drop the dependency".into(),
                "stop".into(),
            ],
            kind: QuestionKind::DependencyEnded,
        });
        task.status = TaskStatus::Waiting;
        task.updated_at = now_ms();
        // The answer policy retries an ended dependency once before the owner
        // is asked.
        if let Some(question) = task.question.clone() {
            if policy_open(self, &task, question.kind)
                && !task
                    .assumptions
                    .iter()
                    .any(|a| a.kind == Some(QuestionKind::DependencyEnded))
            {
                let answer = "retry the dependency";
                record_policy_answer(
                    &mut task,
                    &question,
                    answer,
                    "policy",
                    "a dependency that ended is retried once",
                    0,
                );
                if self
                    .answer_dependency_question(task.clone(), answer, false)
                    .is_ok()
                {
                    return;
                }
                task.assumptions.pop();
                task.decisions.pop();
            }
        }
        let _ = self.store.save_task(&task);
        self.broadcast_task(&task);
    }

    /// Stops every child of `parent_id` that has not finished: a live loop
    /// is cancelled (it marks itself stopped), an idle one is marked here.
    pub(super) fn stop_children(&self, parent_id: &str, repo: &str) {
        for mut child in self.repo_tasks(repo) {
            if child.parent.as_deref() != Some(parent_id)
                || matches!(
                    child.status,
                    TaskStatus::Done | TaskStatus::Failed | TaskStatus::Stopped
                )
            {
                continue;
            }
            if let Some(ctrl) = self.controls.lock().unwrap().get(&child.id) {
                ctrl.cancel.cancel();
                continue;
            }
            child.question = None;
            child.status = TaskStatus::Stopped;
            child.updated_at = now_ms();
            let _ = self.store.save_task(&child);
            self.broadcast_task(&child);
        }
    }

    /// "retry" restarts every task this one waits for that ended
    /// `failed`/`stopped`; "drop" stops waiting for them (a dependency leaves
    /// `dependsOn`, a child is detached from this parent). Either way this
    /// task goes back to waiting for the rest.
    pub(super) fn answer_dependency_question(
        &self,
        mut task: Task,
        answer: &str,
        by_owner: bool,
    ) -> Result<serde_json::Value, String> {
        let all = self.repo_tasks(&task.repo);
        let was_parent = is_parent(&task, &all);
        let ended: Vec<Task> = waits_for(&task, &all)
            .into_iter()
            .filter(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Stopped))
            .cloned()
            .collect();
        let choice = answer.trim().to_ascii_lowercase();
        if choice.starts_with("retry") {
            for dep in ended.iter().filter(|t| !t.archived) {
                if self.controls.lock().unwrap().contains_key(&dep.id) {
                    continue;
                }
                // Queued before its loop runs, so nothing reads it as
                // ended again while it retries.
                let mut dep = dep.clone();
                dep.question = None;
                dep.status = TaskStatus::Queued;
                dep.updated_at = now_ms();
                self.store.save_task(&dep).map_err(|e| e.to_string())?;
                self.broadcast_task(&dep);
                self.spawn_task_loop(dep.id.clone(), true);
            }
        } else if choice.starts_with("drop") {
            task.depends_on
                .retain(|id| !ended.iter().any(|t| &t.id == id));
            for mut child in ended
                .into_iter()
                .filter(|t| t.parent.as_deref() == Some(task.id.as_str()))
            {
                child.parent = None;
                child
                    .decisions
                    .push(format!("Owner: dropped from \"{}\"", task.title));
                child.updated_at = now_ms();
                self.store.save_task(&child).map_err(|e| e.to_string())?;
                self.broadcast_task(&child);
            }
        } else {
            return Err("answer retry the dependency, drop the dependency, or stop".to_string());
        }
        if by_owner {
            task.decisions.push(format!("Owner: {}", answer.trim()));
        }
        task.question = None;
        let fresh = self.repo_tasks(&task.repo);
        if was_parent && !is_parent(&task, &fresh) {
            // Left without children it would implement the whole request.
            task.decisions.push(
                "Orchestrator: no subtasks are left, so this task was stopped instead of implementing the whole request itself"
                    .to_string(),
            );
            task.status = TaskStatus::Stopped;
            task.updated_at = now_ms();
            self.store.save_task(&task).map_err(|e| e.to_string())?;
            self.broadcast_task(&task);
            return serde_json::to_value(&task).map_err(|e| e.to_string());
        }
        task.status = if is_parent(&task, &fresh) {
            TaskStatus::Running
        } else {
            TaskStatus::Queued
        };
        task.updated_at = now_ms();
        self.store.save_task(&task).map_err(|e| e.to_string())?;
        self.broadcast_task(&task);
        // Its loop waits for whatever is still pending, or goes ahead.
        self.spawn_task_loop(task.id.clone(), true);
        let latest = self
            .store
            .load_task(&task.id)
            .map_err(|e| e.to_string())?
            .unwrap_or(task);
        serde_json::to_value(&latest).map_err(|e| e.to_string())
    }
}
