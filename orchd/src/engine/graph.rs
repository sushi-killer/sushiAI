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

/// How a parent's loop ended.
pub(super) enum ParentEnd {
    /// Nothing more for this loop to do: the parent is done, parked, waiting
    /// for its children or landing, or stopped.
    Finished,
    /// Every subtask landed but finishing the parent failed. The loop goes on
    /// as an ordinary implement attempt on the parent's own worktree, with
    /// `failure` in its brief. `budget` caps the attempts (`None`: the usual
    /// `maxAttempts`).
    Attempt {
        failure: String,
        budget: Option<u32>,
    },
}

/// The decision line that keeps a parent's `verify` command although it
/// fails on the base; it is what stops a relaunch from asking again.
fn kept_check_line(command: &str) -> String {
    format!("Orchestrator: kept check {command}, it fails on the base until the subtasks land")
}

/// Applies the owner's (or the policy's) answer to a base-check question of
/// any task: `drop this check` removes the command, `keep this check`
/// records that it stays. Shared by the live loops and the post-restart
/// branch of `task.answer`.
pub(super) fn apply_base_check_answer(task: &mut Task, question: &Question, answer: &str) {
    let Some(command) = pre_existing_command(question).map(str::to_string) else {
        return;
    };
    if is_option(answer, PRE_EXISTING_DROP) {
        task.verify.retain(|c| c != &command);
        task.final_verify.retain(|c| c != &command);
        task.decisions
            .push(format!("Orchestrator: dropped check {command}"));
    } else if question.options.iter().any(|o| o == PARENT_KEEP) {
        let line = kept_check_line(&command);
        if !task.decisions.contains(&line) {
            task.decisions.push(line);
        }
    }
}

/// Before any subtask starts: the parent's own checks run on its base. A
/// `finalVerify` and a `verify` that still fail there on a second run are
/// asked about, and the subtasks start only after the answer. `false`: the
/// loop has to end (cancelled, or stopped by the answer).
async fn parent_base_preflight(
    app: &Arc<App>,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    pending_amend: &Arc<StdMutex<Option<Amendment>>>,
    cancel: &CancelToken,
) -> bool {
    loop {
        // An amendment made while this loop was live (or parked on the
        // question) reaches every pass.
        apply_pending_amendment(app, task, pending_amend, cancel).await;
        let finals: Vec<String> = task
            .final_verify
            .iter()
            .filter(|c| !owner_answered_base_check(task, c, &task.base_sha))
            .cloned()
            .collect();
        let mut failed = failing_twice_on_base(app, task, &finals, cancel).await;
        let mut is_final = failed.is_some();
        if failed.is_none() && !cancel.is_cancelled() {
            let unkept: Vec<String> = task
                .verify
                .iter()
                .filter(|c| !task.decisions.contains(&kept_check_line(c)))
                .filter(|c| !owner_answered_base_check(task, c, &task.base_sha))
                .cloned()
                .collect();
            failed = failing_twice_on_base(app, task, &unkept, cancel).await;
            is_final = false;
        }
        if cancel.is_cancelled() {
            mark_stopped_if_not_already(app, &task.id).await;
            return false;
        }
        let Some(base_run) = failed else {
            return true;
        };
        let question = if is_final {
            pre_existing_question(&base_run.command, &task.base_sha, &base_run.tail)
        } else {
            parent_check_question(&base_run.command, &task.base_sha, &base_run.tail)
        };
        task.question = Some(question.clone());
        task.status = TaskStatus::Waiting;
        task.updated_at = now_ms();
        let task_id = task.id.clone();
        let assumed = task.assumptions.len();
        let Some(answer) =
            wait_for_answer(app, &task_id, task, pending_answer, cancel, &mut None).await
        else {
            return false;
        };
        let policy_answered = task.assumptions.len() > assumed;
        if is_option(&answer, "stop") {
            task.decisions.push("Owner: stop".to_string());
            task.question = None;
            task.status = TaskStatus::Stopped;
            task.updated_at = now_ms();
            let _ = app.store.save_task(task);
            app.broadcast_task(task);
            app.stop_children(&task_id, &task.repo);
            return false;
        }
        record_owner_base_check(
            task,
            &base_run.command,
            &task.base_sha.clone(),
            &answer,
            policy_answered,
        );
        apply_base_check_answer(task, &question, &answer);
        if is_option(&answer, PRE_EXISTING_DROP) {
            app.verify_cache.lock().unwrap().remove(&task_id);
        } else if is_final {
            // Retry after the base is fixed: on the newest base.
            let wt = PathBuf::from(&task.worktree);
            carry_onto_newest_base(task, &wt).await;
        }
        task.updated_at = now_ms();
        let _ = app.store.save_task(task);
        app.broadcast_task(task);
    }
}

/// A parent whose checks passed (or that has none) is done; with
/// `variant.land` it first lands on its base branch as one commit titled
/// with its title, exactly like a single task. A landing that fails goes to
/// an agent attempt.
async fn land_parent(app: &Arc<App>, task: &mut Task, cancel: &CancelToken) -> ParentEnd {
    let task_id = task.id.clone();
    let worktree = PathBuf::from(&task.worktree);
    let mut reason = "every subtask landed";
    if task.variant().land && task.parent.is_none() {
        let run_dir = app.store.task_dir(&task_id).join("runs").join("parent");
        let _ = std::fs::create_dir_all(&run_dir);
        match land_on_base(app, task, None, 0, &worktree, &run_dir, cancel).await {
            Landing::Landed => {
                git::delete_wip_ref(&worktree, &task_id);
                reason = "landed";
            }
            Landing::Waiting => {
                task.status = TaskStatus::Landing;
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
                app.broadcast_task(task);
                return ParentEnd::Finished;
            }
            Landing::Failed { detail, .. } => {
                task.status = TaskStatus::Running;
                task.updated_at = now_ms();
                let _ = app.store.save_task(task);
                app.broadcast_task(task);
                return ParentEnd::Attempt {
                    failure: detail,
                    budget: None,
                };
            }
            Landing::Cancelled => {
                let _ = app.store.save_task(task);
                mark_stopped_if_not_already(app, &task_id).await;
                return ParentEnd::Finished;
            }
            // Refused: done on its own branch, as with land off.
            Landing::NoParent => {}
        }
    }
    task.status = TaskStatus::Done;
    task.updated_at = now_ms();
    app.write_report(task).await;
    app.release_worktree(task, reason).await;
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    ParentEnd::Finished
}

/// A parent's loop: starts its children that have not started yet, and once
/// every child has landed runs its own checks on its branch, lands (with
/// `variant.land`) and ends `done`. Until then it leaves the task `running`
/// with no loop; `advance_graph` comes back to it. A failing check or
/// landing is not the end of the graph: the parent gets an agent attempt
/// (`ParentEnd::Attempt`).
pub(super) async fn run_parent(
    app: &Arc<App>,
    task_id: &str,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<String>>>>,
    pending_amend: &Arc<StdMutex<Option<Amendment>>>,
    cancel: &CancelToken,
) -> ParentEnd {
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return ParentEnd::Finished;
    };
    let all = app.repo_tasks(&task.repo);
    // No child starts before the parent's own dependencies are done.
    match own_waits_state(&task, &all) {
        Waits::Ready | Waits::Nothing => {}
        Waits::Ended(ended) => {
            app.ask_dependency_question(&task, &all, &ended);
            return ParentEnd::Finished;
        }
        Waits::Pending => {
            if task.status != TaskStatus::Queued {
                task.status = TaskStatus::Queued;
                task.updated_at = now_ms();
                let _ = app.store.save_task(&task);
                app.broadcast_task(&task);
            }
            return ParentEnd::Finished;
        }
    }
    // A parent that passed everything and waited to land: only the landing
    // is tried again, no check re-runs and no child is touched.
    if task.status == TaskStatus::Landing {
        let permit = tokio::select! {
            _ = cancel.cancelled() => None,
            p = app.slots.clone().acquire_owned() => p.ok(),
        };
        if permit.is_none() {
            mark_stopped_if_not_already(app, task_id).await;
            return ParentEnd::Finished;
        }
        return land_parent(app, &mut task, cancel).await;
    }
    // Before the first child starts: the parent's checks judge the base, not
    // the children's work. Idempotent across relaunches.
    if task.variant().grounded_checks && needs_baseline(&task) {
        let wt = PathBuf::from(&task.worktree);
        if !baseline_checks(app, task_id, &mut task, &wt, cancel).await {
            mark_stopped_if_not_already(app, task_id).await;
            return ParentEnd::Finished;
        }
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);
    }
    // The parent's own commands, run on the base while no child has started.
    let none_started = all
        .iter()
        .filter(|t| t.parent.as_deref() == Some(task_id) && !t.archived)
        .all(|t| {
            matches!(t.status, TaskStatus::Drafting | TaskStatus::Queued)
                && implement_attempt_count(t) == 0
        });
    if none_started
        && !parent_base_preflight(app, &mut task, pending_answer, pending_amend, cancel).await
    {
        return ParentEnd::Finished;
    }
    // `advance_graph` only relaunches a queued or running parent, so a
    // stopped or failed one here is the owner's own restart: its unfinished
    // children restart with it.
    let restarted = matches!(task.status, TaskStatus::Stopped | TaskStatus::Failed);
    for child in all
        .iter()
        .filter(|t| t.parent.as_deref() == Some(task_id) && !t.archived)
    {
        if cancel.is_cancelled() {
            break;
        }
        match child.status {
            // A part whose dependencies have not landed is not started (so
            // not planned) yet; the graph comes back to it when they do.
            TaskStatus::Drafting | TaskStatus::Queued => {
                if matches!(waits_state(child, &all), Waits::Ready | Waits::Nothing) {
                    app.spawn_child_loop(&all, child);
                }
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
                app.spawn_child_loop(&all, &child);
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
        return ParentEnd::Finished;
    }

    let permit = tokio::select! {
        _ = cancel.cancelled() => None,
        p = app.slots.clone().acquire_owned() => p.ok(),
    };
    if permit.is_none() {
        mark_stopped_if_not_already(app, task_id).await;
        return ParentEnd::Finished;
    }
    task.status = TaskStatus::Running;
    task.updated_at = now_ms();
    let _ = app.store.save_task(&task);
    app.broadcast_task(&task);
    // An amendment made at any point (also while this loop waited for a
    // slot) reaches this check.
    apply_pending_amendment(app, &mut task, pending_amend, cancel).await;
    let Ok(Some(task)) = app.store.load_task(task_id) else {
        return ParentEnd::Finished;
    };

    let checks: Vec<String> = task
        .verify
        .iter()
        .chain(&task.final_verify)
        .cloned()
        .collect();
    let sandbox = app.settings.read().unwrap().sandbox;
    let (checks, skipped) = scope_to_diff(
        app,
        &task.repo,
        Path::new(&task.worktree),
        &task.base_sha,
        &checks,
    )
    .await;
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
        return ParentEnd::Finished;
    }
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return ParentEnd::Finished;
    };
    task.cost_usd = parent_cost(&task, &app.repo_tasks(&task.repo));
    record_skips(&mut task, skipped);
    let failure = match results.iter().find(|v| v.code != Some(0)) {
        Some(failed) => {
            let code = failed
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "null".to_string());
            task.decisions.push(format!(
                "Orchestrator: every subtask landed, but `{}` exited {code} on {}: {}",
                failed.command,
                task.branch,
                tail_chars(failed.tail.trim(), 600)
            ));
            Some(format!(
                "`{}` exited {code} on {}:\n{}",
                failed.command,
                task.branch,
                failed.tail.trim()
            ))
        }
        None => grounded_failure.inspect(|line| task.decisions.push(line.clone())),
    };
    if let Some(failure) = failure {
        // A check that fails after the subtasks landed gets one agent
        // attempt on the parent's branch, not the end of the graph.
        task.updated_at = now_ms();
        let _ = app.store.save_task(&task);
        app.broadcast_task(&task);
        return ParentEnd::Attempt {
            failure,
            budget: Some(1),
        };
    }
    task.decisions.push(format!(
        "Orchestrator: every subtask landed on {}",
        task.branch
    ));
    land_parent(app, &mut task, cancel).await
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

/// A relay link's start: its worktree continues the previous link's branch
/// (the earlier subtask's commit), so it edits on top of that work. When the
/// previous link cannot be continued the subtask starts from the parent's
/// head like any other.
pub(super) async fn start_relay_link(app: &Arc<App>, task: &mut Task, prev_id: &str) {
    let prev = app
        .store
        .load_task(prev_id)
        .ok()
        .flatten()
        .filter(|p| p.status == TaskStatus::Done);
    let Some(prev) = prev else {
        task.queue.relay_of = None;
        task.decisions.push(
            "Relay: the previous subtask did not finish; starting from the parent's head instead"
                .to_string(),
        );
        return;
    };
    let (wt, from, tid, branch) = (
        PathBuf::from(&task.worktree),
        task.base_sha.clone(),
        task.id.clone(),
        prev.branch.clone(),
    );
    let synced =
        tokio::task::spawn_blocking(move || git::carry_onto_moved_base(&wt, &branch, &from, &tid))
            .await;
    let head = match synced {
        Ok(Ok(git::Rebase::Moved { new_sha, .. })) => new_sha,
        Ok(Ok(git::Rebase::Unchanged)) => task.base_sha.clone(),
        _ => {
            task.queue.relay_of = None;
            task.decisions.push(format!(
                "Relay: could not continue {}; starting from the parent's head instead",
                prev.branch
            ));
            return;
        }
    };
    task.decisions.push(format!(
        "Relay: continues \"{}\" on {} at {}",
        prev.title,
        prev.branch,
        short_sha(&head)
    ));
    task.queue.relay_base = Some(
        prev.queue
            .relay_base
            .clone()
            .unwrap_or_else(|| prev.base_sha.clone()),
    );
    task.base_sha = head;
    task.base_ref = Some(prev.branch);
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
    // A link of a relay that a later sibling continues commits on its own
    // branch; the last link lands the whole chain on the parent.
    let next = app
        .repo_tasks(&task.repo)
        .into_iter()
        .find(|t| t.queue.relay_of.as_deref() == Some(task.id.as_str()) && !t.archived);
    if let Some(next) = next {
        let (wt, title, tid) = (worktree.to_path_buf(), task.title.clone(), task.id.clone());
        let committed =
            tokio::task::spawn_blocking(move || git::commit(&wt, &title, &tid, attempt_n))
                .await
                .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
        return match committed {
            Ok(()) => {
                task.decisions.push(format!(
                    "Land: committed on {} for \"{}\" to continue from",
                    task.branch, next.title
                ));
                record_landing(task).await;
                Landing::Landed
            }
            Err(e) => Landing::Failed {
                kind: FailureKind::Error,
                detail: format!("Could not commit for the relay: {e}"),
            },
        };
    }
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

    // The last link of a relay: everything since the chain began is this
    // task's work now, carried onto the parent like any other.
    if let Some(chain_base) = task.queue.relay_base.clone() {
        let (wt, base) = (worktree.to_path_buf(), chain_base.clone());
        let reset = tokio::task::spawn_blocking(move || git::uncommit_to(&wt, &base))
            .await
            .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
        if let Err(e) = reset {
            return failed(
                FailureKind::Error,
                format!("Could not gather the relay's work: {e}"),
            );
        }
        task.decisions.push(format!(
            "Land: lands the relay's chain from {} onto {branch}",
            short_sha(&chain_base)
        ));
        task.base_sha = chain_base;
        task.base_ref = Some(branch.clone());
        task.queue.relay_base = None;
    }

    match carry_and_check(
        app,
        task,
        Some(idx),
        worktree,
        run_dir,
        &branch,
        "Another subtask",
        &[],
        cancel,
    )
    .await
    {
        Ok(true) => {
            record_landing(task).await;
            return Landing::Landed;
        }
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
            record_landing(task).await;
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
/// returns: a failed attempt, or a cancel. `idx` is the attempt record that
/// takes the changed files and verify results; a parent landing without an
/// attempt has none.
#[allow(clippy::too_many_arguments)]
pub(super) async fn carry_and_check(
    app: &Arc<App>,
    task: &mut Task,
    idx: Option<usize>,
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
        Ok(git::Rebase::Moved { new_sha, rewritten }) => {
            task.decisions.push(if rewritten {
                format!(
                    "Land: {branch} was rewritten; carried the work onto it at {}",
                    short_sha(&new_sha)
                )
            } else {
                format!(
                    "Land: carried the work onto {branch} at {}",
                    short_sha(&new_sha)
                )
            });
            task.base_sha = new_sha.clone();
            let (wt, b) = (worktree.to_path_buf(), new_sha.clone());
            let changed = tokio::task::spawn_blocking(move || {
                git::changed_files(&wt, &b).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            if let Some(idx) = idx {
                task.attempts[idx].changed_files = changed.clone();
            }
            if changed.is_empty() {
                task.decisions
                    .push(format!("Land: {branch} already contains this work"));
                return Ok(true);
            }
            let (verify_commands, skipped) =
                scope_to_diff(app, &task.repo, worktree, &new_sha, &task.verify).await;
            record_skips(task, skipped);
            let results = run_verify_cached(
                app,
                &task.id,
                worktree,
                run_dir,
                &new_sha,
                &verify_commands,
                cancel,
            )
            .await;
            if cancel.is_cancelled() {
                return Err(Landing::Cancelled);
            }
            if let Some(idx) = idx {
                task.attempts[idx].verify = results.clone();
            }
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
            let (extra_final, skipped) =
                scope_to_diff(app, &task.repo, worktree, &task.base_sha, extra_final).await;
            record_skips(task, skipped);
            if !extra_final.is_empty() {
                let sandbox = app.settings.read().unwrap().sandbox;
                let finals = run_verify_commands(
                    worktree,
                    &run_dir.join("final-land"),
                    &extra_final,
                    sandbox,
                    &task.base_sha,
                    cancel,
                )
                .await;
                if cancel.is_cancelled() {
                    return Err(Landing::Cancelled);
                }
                if let Some(idx) = idx {
                    task.attempts[idx].verify.extend(finals.iter().cloned());
                }
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
        Ok(git::Rebase::Conflicts {
            new_sha,
            files,
            rewritten,
        }) => {
            if rewritten {
                task.decisions.push(format!(
                    "Land: {branch} was rewritten; carried the work onto it at {} with conflicts in {}",
                    short_sha(&new_sha),
                    files.join(", ")
                ));
            }
            task.base_sha = new_sha.clone();
            return failed(
                FailureKind::Conflict,
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
            if !idle
                || task.queue.backlog.is_some()
                || self.controls.lock().unwrap().contains_key(&task.id)
            {
                continue;
            }
            // Waiting for a lease: `advance_queue` starts it when it is free.
            if task.queue.queue_reason.is_some() && self.lease_blocker(task).is_some() {
                continue;
            }
            // A subtask never starts while its parent still waits for the
            // parent's own dependencies or sits in the backlog.
            if let Some(p) = task
                .parent
                .as_deref()
                .and_then(|pid| all.iter().find(|t| t.id == pid))
            {
                if !matches!(own_waits_state(p, &all), Waits::Ready | Waits::Nothing)
                    || p.queue.backlog.is_some()
                    || (p.status == TaskStatus::Waiting && implement_attempt_count(task) == 0)
                {
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
                    if task.parent.is_some() {
                        self.spawn_child_loop(&all, task);
                    } else {
                        self.spawn_task_loop(task.id.clone(), true)
                    }
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
        self.advance_queue(repo);
        self.advance_autopilot();
    }

    /// Starts `child`'s loop unless its parent already has `childParallel`
    /// subtasks running. `false`: not started; the graph comes back to it
    /// when a sibling ends.
    pub(super) fn spawn_child_loop(&self, all: &[Task], child: &Task) -> bool {
        let cap = self.settings.read().unwrap().child_parallel.max(1) as usize;
        let _admit = self.child_admission.lock().unwrap();
        let running = {
            let controls = self.controls.lock().unwrap();
            all.iter()
                .filter(|t| {
                    t.parent == child.parent && t.id != child.id && controls.contains_key(&t.id)
                })
                .count()
        };
        if running >= cap {
            return false;
        }
        self.spawn_task_loop(child.id.clone(), true);
        true
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
        task.question = Some(Question::new(
            format!(
                "{DEPENDENCY_QUESTION}: {}. Retry it, drop it from what this task waits for, or stop?",
                names.join(", ")
            ),
            vec![
                "retry the dependency".into(),
                "drop the dependency".into(),
                "stop".into(),
            ],
            QuestionKind::DependencyEnded,
            AskedBy::Implement,
        ));
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
                task.question_history.pop();
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
            if let Some(question) = task.question.clone() {
                record_answered_question(&mut task, &question, answer.trim(), AnsweredBy::Owner);
            }
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
