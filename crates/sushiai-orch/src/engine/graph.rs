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

/// Titles of what a queued task waits for, for its `dependency` wait: what
/// it depends on that is not done, else what its parent waits for.
pub(super) fn unmet_titles(task: &Task, all: &[Task]) -> Vec<String> {
    let unmet = |of: &Task| -> Vec<String> {
        waits_for(of, all)
            .iter()
            .filter(|t| t.status != TaskStatus::Done)
            .map(|t| t.title.clone())
            .collect()
    };
    let mut titles = unmet(task);
    if titles.is_empty() {
        if let Some(parent) = task
            .parent
            .as_deref()
            .and_then(|pid| all.iter().find(|t| t.id == pid))
        {
            titles = unmet(parent);
            if titles.is_empty() {
                titles.push(parent.title.clone());
            }
        }
    }
    titles
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

/// Applies the owner's (or the policy's) answer to a base-check question of
/// any task: `drop all` removes the commands, `keep all` records that they
/// stay. Shared by the live loops and the post-restart branch of
/// `task.answer`. `true` when at least one command was removed. A question
/// without `commands` (persisted before they existed) applies nothing.
pub(super) fn apply_base_check_answer(
    settings: &Settings,
    task: &mut Task,
    question: &Question,
    answer: &str,
    origin: Origin,
) -> bool {
    if question.kind != QuestionKind::PreexistingFailure {
        return false;
    }
    if is_option(answer, PRE_EXISTING_DROP) {
        let mut dropped = false;
        for command in &question.commands {
            match remove_check_command(settings, task, command, origin) {
                Ok(()) => {
                    task.decisions
                        .push(format!("Orchestrator: dropped check {command}"));
                    dropped = true;
                }
                // Refused: the check stays, the answer counts as a retry.
                Err(why) => task
                    .decisions
                    .push(format!("Orchestrator: check {command} stays: {why}")),
            }
        }
        return dropped;
    }
    if is_option(answer, KEEP_ALL) {
        for command in &question.commands {
            keep_check(task, command, KEPT_BY_TASK_TARGET);
        }
    }
    false
}

/// Before any subtask starts: the parent's unkept `verify` commands run on
/// its base. One that still fails there on a second run is asked about, and
/// the subtasks start only after the answer. The parent's `finalVerify` base
/// runs go to background jobs the subtasks do not wait for; they are looked
/// up at the parent's own gate. `false`: the loop has to end (cancelled, or
/// stopped by the answer).
async fn parent_base_preflight(
    app: &Arc<App>,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    pending_amend: &Arc<StdMutex<Option<Amendment>>>,
    cancel: &CancelToken,
) -> bool {
    loop {
        // An amendment made while this loop was live (or parked on the
        // question) reaches every pass.
        apply_pending_amendment(app, task, pending_amend, cancel).await;
        start_final_base_jobs(app, task);
        let unkept: Vec<String> = task
            .verify
            .iter()
            .filter(|c| !is_kept(task, c))
            .filter(|c| !base_check_answered(task, c, &task.base_sha))
            .cloned()
            .collect();
        let mut failed = failing_twice_on_base(app, task, &unkept, cancel).await;
        if cancel.is_cancelled() {
            mark_stopped_if_not_already(app, &task.id).await;
            return false;
        }
        // The plan creates the files a check names: kept by rule, no question.
        let names: Vec<String> = failed.iter().map(|f| f.command.clone()).collect();
        let kept = keep_planned_checks(app, task, &names);
        if !kept.is_empty() {
            task.updated_at = now_ms();
            app.save_task_logged(task);
            app.broadcast_task(task);
        }
        failed.retain(|f| !kept.contains(&f.command));
        if failed.is_empty() {
            return true;
        }
        let question = parent_checks_question(&failed, &task.base_sha);
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
            if let Some(who) = answerer_label(task) {
                task.decisions.push(format!("{who}: stop"));
            }
            task.question = None;
            task.status = TaskStatus::Stopped;
            app.close_stopped_task(task);
            task.updated_at = now_ms();
            app.save_task_logged(task);
            app.broadcast_task(task);
            app.stop_children(&task_id, &task.repo);
            return false;
        }
        record_owner_base_check(
            task,
            &question.commands,
            &task.base_sha.clone(),
            &answer,
            policy_answered,
        );
        let settings = app.settings.read().unwrap().clone();
        let origin = origin_of_last_answer(task);
        let dropped = apply_base_check_answer(&settings, task, &question, &answer, origin);
        if dropped {
            app.verify_cache.lock().unwrap().remove(&task_id);
        }
        task.updated_at = now_ms();
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
}

/// What the parent's gate does after its checks ran.
enum ParentGate {
    /// Go on with these results (a dropped final no longer counts).
    Go(Vec<VerifyOutcome>),
    /// The work was carried onto the newest base: run the checks again.
    Again,
    Conflict(String),
    /// The loop has to end (cancelled, or stopped by the answer).
    End,
}

/// A final check of the parent that fails on its branch is looked up on the
/// base (the cache, else the in-flight background job). One that also fails
/// there and that the keep rule does not explain goes to ONE question
/// instead of an agent attempt; the others stay ordinary failures.
async fn parent_finals_on_base(
    app: &Arc<App>,
    task: &mut Task,
    results: Vec<VerifyOutcome>,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
) -> ParentGate {
    let failing_finals: Vec<VerifyOutcome> = results
        .iter()
        .filter(|v| v.code != Some(0))
        .filter(|v| task.final_verify.contains(&v.command) && !task.verify.contains(&v.command))
        .cloned()
        .collect();
    if failing_finals.is_empty() {
        return ParentGate::Go(results);
    }
    let dir = app
        .store
        .task_dir(&task.id)
        .join("runs")
        .join("parent")
        .join("final-base");
    let mut on_base: Vec<VerifyOutcome> = Vec::new();
    for failed in &failing_finals {
        if let Some(base_run) = failing_on_base(app, task, failed, &dir, cancel).await {
            on_base.push(base_run);
        }
    }
    if cancel.is_cancelled() {
        return ParentGate::End;
    }
    let names: Vec<String> = on_base.iter().map(|b| b.command.clone()).collect();
    let kept = keep_planned_checks(app, task, &names);
    if !kept.is_empty() {
        task.updated_at = now_ms();
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
    on_base.retain(|b| !kept.contains(&b.command) && !is_kept(task, &b.command));
    // The base is red too: only a failure the base lacks is the parent's.
    let wt = PathBuf::from(&task.worktree);
    let mut unclear: Vec<VerifyOutcome> = Vec::new();
    let mut judged: Vec<String> = Vec::new();
    for base_run in on_base {
        let Some(failed) = failing_finals
            .iter()
            .find(|f| f.command == base_run.command)
        else {
            continue;
        };
        // A check the parent's work is meant to turn green is never waived;
        // it stays an ordinary failure.
        if command_named_by_task(task, &base_run.command) {
            keep_check(task, &base_run.command, KEPT_BY_TASK_TARGET);
            continue;
        }
        match judge_against_base(app, task, failed, &base_run, &wt, &dir, cancel).await {
            BaseVerdict::Waived => judged.push(base_run.command.clone()),
            BaseVerdict::NewFailures(_) => {}
            BaseVerdict::Unclear => unclear.push(base_run),
        }
    }
    let on_base = unclear;
    let results: Vec<VerifyOutcome> = results
        .into_iter()
        .filter(|v| !judged.contains(&v.command))
        .collect();
    if on_base.is_empty() {
        return ParentGate::Go(results);
    }
    let question = pre_existing_question(&on_base, &task.base_sha);
    let task_id = task.id.clone();
    let assumed = task.assumptions.len();
    let already_retried = base_retry_requested(task, &question.commands, &task.base_sha);
    let answer = if already_retried {
        PRE_EXISTING_RETRY.to_string()
    } else {
        task.question = Some(question.clone());
        task.status = TaskStatus::Waiting;
        task.updated_at = now_ms();
        let Some(answer) =
            wait_for_answer(app, &task_id, task, pending_answer, cancel, &mut None).await
        else {
            return ParentGate::End;
        };
        answer
    };
    let policy_answered = task.assumptions.len() > assumed;
    if is_option(&answer, "stop") {
        if let Some(who) = answerer_label(task) {
            task.decisions.push(format!("{who}: stop"));
        }
        task.question = None;
        task.status = TaskStatus::Stopped;
        app.close_stopped_task(task);
        task.updated_at = now_ms();
        app.save_task_logged(task);
        app.broadcast_task(task);
        app.stop_children(&task_id, &task.repo);
        return ParentGate::End;
    }
    if !already_retried {
        let base_sha = task.base_sha.clone();
        record_owner_base_check(
            task,
            &question.commands,
            &base_sha,
            &answer,
            policy_answered,
        );
    }
    let settings = app.settings.read().unwrap().clone();
    let origin = origin_of_last_answer(task);
    // Every removal goes through `remove_check_command`; a refused command
    // stays and the answer counts as a retry.
    if apply_base_check_answer(&settings, task, &question, &answer, origin) {
        app.verify_cache.lock().unwrap().remove(&task_id);
    }
    task.status = TaskStatus::Running;
    task.updated_at = now_ms();
    // A retry on an unchanged base would meet the same failure: every retry
    // waits for the base to move and carries onto it.
    let kept_all =
        !question.commands.is_empty() && question.commands.iter().all(|c| is_kept(task, c));
    if !kept_all
        && (is_option(&answer, PRE_EXISTING_RETRY)
            || question
                .commands
                .iter()
                .any(|c| task.final_verify.contains(c)))
    {
        let wt = PathBuf::from(&task.worktree);
        match carry_after_base_moves(app, task, &wt, pending_answer, cancel, &mut None).await {
            BaseRetry::Ready => {}
            BaseRetry::Conflict(detail) => {
                task.status = TaskStatus::Running;
                task.updated_at = now_ms();
                app.save_task_logged(task);
                app.broadcast_task(task);
                return ParentGate::Conflict(detail);
            }
            BaseRetry::Stopped => return ParentGate::End,
        }
        app.save_task_logged(task);
        app.broadcast_task(task);
        return ParentGate::Again;
    }
    if kept_all {
        app.save_task_logged(task);
        app.broadcast_task(task);
        return ParentGate::Go(results);
    }
    app.save_task_logged(task);
    app.broadcast_task(task);
    ParentGate::Go(
        results
            .into_iter()
            .filter(|v| !question.commands.contains(&v.command))
            .collect(),
    )
}

/// A parent whose checks passed (or that has none) is done; with
/// `variant.land` it first lands on its base branch as one commit titled
/// with its title, exactly like a single task. A landing that fails goes to
/// an agent attempt.
async fn land_parent(app: &Arc<App>, task: &mut Task, cancel: &CancelToken) -> ParentEnd {
    let task_id = task.id.clone();
    let worktree = PathBuf::from(&task.worktree);
    let mut landed = false;
    // A subtask no review read already landed on this branch: the parent
    // is unreviewed too, is not landed by itself, and the owner gets a PR.
    ladder::inherit_unreviewed(app, task);
    if !task.unreviewed && task.variant().land && task.parent.is_none() {
        let run_dir = app.store.task_dir(&task_id).join("runs").join("parent");
        let _ = std::fs::create_dir_all(&run_dir);
        match land_on_base(app, task, None, 0, &worktree, &run_dir, cancel).await {
            Landing::Landed => {
                git::delete_wip_ref(&worktree, &task_id);
                landed = true;
            }
            Landing::Waiting => {
                task.status = TaskStatus::Landing;
                task.updated_at = now_ms();
                app.save_task_logged(task);
                app.broadcast_task(task);
                return ParentEnd::Finished;
            }
            Landing::Failed { detail, .. } => {
                task.status = TaskStatus::Running;
                task.updated_at = now_ms();
                app.save_task_logged(task);
                app.broadcast_task(task);
                return ParentEnd::Attempt {
                    failure: detail,
                    budget: None,
                };
            }
            Landing::Cancelled => {
                app.save_task_logged(task);
                mark_stopped_if_not_already(app, &task_id).await;
                return ParentEnd::Finished;
            }
            // A parent has no review to run again.
            Landing::Rereview => {}
            // Refused: done on its own branch, which gets a pull request.
            Landing::NoParent | Landing::DefaultBranchRefused => {}
        }
    }
    if !landed {
        // A clean carry turned the landed child commits back into a diff.
        // Keep that work on the parent's branch before removing its worktree.
        let (wt, title, tid) = (worktree.clone(), task.title.clone(), task_id.clone());
        let committed = tokio::task::spawn_blocking(move || {
            if git::status_short(&wt)?.trim().is_empty() {
                Ok(())
            } else {
                git::commit(&wt, &title, &tid, 0)
            }
        })
        .await
        .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
        if let Err(e) = committed {
            let failure = format!("Orchestrator: could not commit the parent's carried work: {e}");
            task.decisions.push(failure.clone());
            task.updated_at = now_ms();
            app.save_task_logged(task);
            app.broadcast_task(task);
            return ParentEnd::Attempt {
                failure,
                budget: Some(1),
            };
        }
        git::delete_wip_ref(&worktree, &task_id);
    }
    task.status = TaskStatus::Done;
    super::pr_draft::mark_pr_pending(app, task);
    task.updated_at = now_ms();
    app.write_report(task).await;
    app.release_worktree(
        task,
        if landed {
            "landed"
        } else {
            "every subtask landed"
        },
    )
    .await;
    task.updated_at = now_ms();
    app.save_task_logged(task);
    app.broadcast_task(task);
    app.notify_task_unreviewed(task);
    wake_orchestrator_for_done(app, task);
    super::pr_draft::spawn_pr_on_done(app, task);
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
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    pending_amend: &Arc<StdMutex<Option<Amendment>>>,
    cancel: &CancelToken,
) -> ParentEnd {
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return ParentEnd::Finished;
    };
    apply_pending_amendment(app, &mut task, pending_amend, cancel).await;
    // The parent's own checks run after the children landed: fix a grep that
    // cannot run now (the children's loops fix theirs).
    if heal_grep_dashes(&mut task) {
        app.verify_cache.lock().unwrap().remove(task_id);
        task.updated_at = now_ms();
        app.save_task_logged(&task);
        app.broadcast_task(&task);
    }
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
                app.save_task_logged(&task);
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
    if needs_prepare(&task) {
        let wt = PathBuf::from(&task.worktree);
        if !prepare_tests(app, task_id, &mut task, &wt, cancel).await {
            mark_stopped_if_not_already(app, task_id).await;
            return ParentEnd::Finished;
        }
        app.save_task_logged(&task);
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
    let dependencies_before = task.depends_on.clone();
    if none_started
        && !parent_base_preflight(app, &mut task, pending_answer, pending_amend, cancel).await
    {
        return ParentEnd::Finished;
    }
    apply_pending_amendment(app, &mut task, pending_amend, cancel).await;
    if task.depends_on != dependencies_before {
        task.status = TaskStatus::Queued;
        task.updated_at = now_ms();
        app.save_task_logged(&task);
        app.broadcast_task(&task);
        return ParentEnd::Finished;
    }
    // `advance_graph` only relaunches a queued or running parent, so a
    // stopped or failed one here is the owner's own restart: its unfinished
    // children restart with it.
    let restarted = task.queue.restart_children;
    task.queue.restart_children = false;
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
            TaskStatus::Drafting | TaskStatus::Queued if child.queue.park.is_none() => {
                if matches!(waits_state(child, &all), Waits::Ready | Waits::Nothing) {
                    app.spawn_child_loop(&all, child);
                } else {
                    app.note_dependency_wait(child, &all);
                }
            }
            TaskStatus::Stopped | TaskStatus::Failed if restarted => {
                // Queued before its loop runs, so this parent never reads
                // it as ended.
                let mut child = child.clone();
                let mut cancellations = app.cancellations.lock().unwrap();
                if let Some(end) = cancellations.get_mut(&child.id) {
                    end.restart = true;
                }
                child.queue.stopped_explicitly = false;
                child.queue.park = None;
                child.question = None;
                child.status = TaskStatus::Queued;
                child.updated_at = now_ms();
                app.save_task_logged(&child);
                app.broadcast_task(&child);
                drop(cancellations);
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
            app.save_task_logged(&task);
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
    app.save_task_logged(&task);
    app.broadcast_task(&task);
    let (mut task, results, skipped) = loop {
        // An amendment made at any point (also while this loop waited for a
        // slot or an answer) reaches this check.
        apply_pending_amendment(app, &mut task, pending_amend, cancel).await;
        let Ok(Some(mut task)) = app.store.load_task(task_id) else {
            return ParentEnd::Finished;
        };
        let checks: Vec<String> = task
            .verify
            .iter()
            .chain(&task.final_verify)
            .cloned()
            .collect();
        let (checks, skipped) = scope_task_checks(
            app,
            &task,
            Path::new(&task.worktree),
            &task.base_sha,
            &checks,
        )
        .await;
        let sandbox = app.task_sandbox(
            &task.repo,
            Path::new(&task.worktree),
            &app.store.task_dir(task_id).join("runs/parent"),
        );
        let results = run_verify_commands(
            CheckContext::new(app, Some(&task.id)).with_gate("parent gate"),
            Path::new(&task.worktree),
            &app.store.task_dir(task_id).join("runs").join("parent"),
            &checks,
            sandbox.as_ref(),
            &task.base_sha,
            cancel,
        )
        .await;
        record_check_outcomes(app, &mut task, &results);
        if cancel.is_cancelled() {
            mark_stopped_if_not_already(app, task_id).await;
            return ParentEnd::Finished;
        }
        match parent_finals_on_base(app, &mut task, results, pending_answer, cancel).await {
            ParentGate::Go(results) => break (task, results, skipped),
            ParentGate::Again => continue,
            ParentGate::Conflict(failure) => {
                return ParentEnd::Attempt {
                    failure,
                    budget: Some(1),
                };
            }
            ParentGate::End => {
                mark_stopped_if_not_already(app, task_id).await;
                return ParentEnd::Finished;
            }
        }
    };
    let sandbox = app.task_sandbox(
        &task.repo,
        Path::new(&task.worktree),
        &app.store.task_dir(task_id).join("runs/parent"),
    );
    // Once, after verify and finalVerify: the gated checks, then the held-out
    // one. The line for a failure names its kind and criterion, and never the
    // held-out command.
    let mut grounded_failure: Option<String> = None;
    if results.iter().all(|v| v.code == Some(0)) {
        let dir = app.store.task_dir(task_id).join("runs").join("parent");
        let worktree = task.worktree.clone();
        let wt = Path::new(&worktree);
        let gated = gated_checks(&task);
        let commands: Vec<String> = gated.iter().map(|c| c.run.clone()).collect();
        let gated_results = run_verify_commands(
            CheckContext::new(app, Some(&task.id)),
            wt,
            &dir,
            &commands,
            sandbox.as_ref(),
            &task.base_sha,
            cancel,
        )
        .await;
        record_check_outcomes(app, &mut task, &gated_results);
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
            let (result, detail) = run_held_out(
                CheckContext::new(app, Some(&task.id)),
                &task,
                &held,
                wt,
                &dir,
                sandbox.as_ref(),
                cancel,
            )
            .await;
            record_check_outcomes(app, &mut task, std::slice::from_ref(&result));
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
    let decisions = task.decisions;
    let Ok(Some(mut task)) = app.store.load_task(task_id) else {
        return ParentEnd::Finished;
    };
    record_check_decisions(&mut task, decisions);
    task.cost_usd = parent_cost(&task, &app.repo_tasks(&task.repo));
    record_check_decisions(&mut task, skipped);
    let failure = match results.iter().find(|v| v.code != Some(0)) {
        Some(failed) => {
            let code = failed
                .code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "null".to_string());
            if !task
                .loop_state
                .final_check_failures
                .contains(&failed.command)
            {
                task.loop_state
                    .final_check_failures
                    .push(failed.command.clone());
            }
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
        app.save_task_logged(&task);
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
    remove_chain_branches(app, &mut task).await;
    land_parent(app, &mut task, cancel).await
}

/// Once a parent holds every subtask, the subtasks' own branches are only
/// clutter: removes, locally, those fully contained in the parent branch.
async fn remove_chain_branches(app: &Arc<App>, task: &mut Task) {
    let candidates: Vec<(String, String)> = app
        .repo_tasks(&task.repo)
        .into_iter()
        .filter(|t| t.parent.as_deref() == Some(task.id.as_str()) && t.branch != task.branch)
        .map(|t| (t.id, t.branch))
        .collect();
    if candidates.is_empty() {
        return;
    }
    let (repo, into) = (PathBuf::from(&task.repo), task.branch.clone());
    let removed =
        tokio::task::spawn_blocking(move || git::delete_merged_branches(&repo, &into, &candidates))
            .await
            .unwrap_or_default();
    if !removed.is_empty() {
        task.decisions
            .push(format!("Branches: removed {}", removed.join(", ")));
    }
}

/// The carry-onto-moved-base failure detail the agent gets: which files
/// conflict and how to resolve them.
pub(super) fn conflict_detail(base_ref: &str, new_sha: &str, files: &[String]) -> String {
    format!(
        "{base_ref} moved ahead to {}, and your changes were carried onto it. \
         These files conflict: {}.",
        short_sha(new_sha),
        files.join(", ")
    )
}

/// Records the files a carry left conflicted: the next implement attempt's
/// brief names them (`brief::carry_conflict_block`).
pub(super) fn set_carry_conflicts(task: &mut Task, files: Vec<String>) {
    task.queue.carry_conflicts = files;
    task.queue.carry_conflict_step = None;
}

/// Clears the conflict note once the attempt that carried it ended with a
/// recorded outcome; an interrupted attempt keeps it for the next one.
pub(super) fn settle_carry_conflicts(task: &mut Task) {
    let Some(n) = task.queue.carry_conflict_step else {
        return;
    };
    let ended = task.attempts.iter().any(|a| {
        a.n == n
            && !matches!(
                a.status,
                AttemptStatus::Running | AttemptStatus::Interrupted
            )
    });
    if ended {
        task.queue.carry_conflicts.clear();
        task.queue.carry_conflict_step = None;
    }
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
    /// Landing was refused because the base is the default branch and landing
    /// there is not allowed: like `NoParent`, and the work may get a pull
    /// request.
    DefaultBranchRefused,
    /// The base branch's checkout has uncommitted changes: the task waits
    /// `landing` and is retried.
    Waiting,
    /// The carry changed a file the review saw: the attempt's verify and
    /// review run again on the carried tree, with no new implement attempt.
    Rereview,
}

/// Start of the decision line of a landing that sent the attempt back to
/// verify and the review.
const REREVIEW_LINE: &str = "Land: the carry";
/// How often one task goes back to verify and the review after a landing
/// carry before the carried tree lands on the checks alone.
const MAX_REREVIEWS: u32 = 3;

/// The files the review saw that the carry onto `new_base` rewrote: files
/// the task changed, in the reviewed tree or the carried one, that differ
/// between the two. Empty without a review.
async fn carry_changed_reviewed(worktree: &Path, attempt: &Attempt, new_base: &str) -> Vec<String> {
    let (Some(tree), Some(old_base)) = (attempt.review_tree.clone(), attempt.review_base.clone())
    else {
        return Vec::new();
    };
    let (wt, new_base) = (worktree.to_path_buf(), new_base.to_string());
    tokio::task::spawn_blocking(move || {
        let now = git::worktree_tree(&wt)?;
        git::own_changes_between(&wt, &tree, &old_base, &now, &new_base)
    })
    .await
    .unwrap_or_else(|e| Err(git::GitError(e.to_string())))
    .unwrap_or_default()
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
    let (wt, mut snapshot, b) = (worktree.to_path_buf(), task.clone(), branch.clone());
    let carried = tokio::task::spawn_blocking(move || {
        // Commits the agent made itself go back to being uncommitted work,
        // so they are carried (and later committed) like the rest.
        if git::head_sha(&wt)? != snapshot.base_sha {
            git::uncommit_to(&wt, &snapshot.base_sha)?;
        }
        let carried = carry_with_planner_tests(&mut snapshot, &wt, &b)?;
        Ok((carried, snapshot.planner_tests))
    })
    .await
    .unwrap_or_else(|e| Err(git::GitError(e.to_string())));
    let carried = carried.map(|(carried, tests)| {
        adopt_carried_tests(task, tests, &carried);
        carried
    });
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
            // A carry that rewrote a file the review saw leaves that review
            // judging another tree: verify and the review run again.
            if let Some(idx) = idx {
                let files = carry_changed_reviewed(worktree, &task.attempts[idx], &new_sha).await;
                if !files.is_empty() && task.loop_state.rereviews < MAX_REREVIEWS {
                    task.loop_state.rereviews += 1;
                    task.decisions.push(format!(
                        "{REREVIEW_LINE} onto {branch} changed {} which the review saw; verify and the review run again on the carried tree",
                        files.join(", ")
                    ));
                    return Err(Landing::Rereview);
                }
            }
            let (verify_commands, skipped) =
                scope_task_checks(app, task, worktree, &new_sha, &task.verify).await;
            record_check_decisions(task, skipped);
            let (results, cache_line) = run_verify_cached(
                app,
                &task.id,
                &task.repo,
                worktree,
                run_dir,
                &new_sha,
                &verify_commands,
                cancel,
            )
            .await;
            record_check_decisions(task, cache_line);
            if cancel.is_cancelled() {
                return Err(Landing::Cancelled);
            }
            record_check_outcomes(app, task, &results);
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
                scope_task_checks(app, task, worktree, &task.base_sha, extra_final).await;
            record_check_decisions(task, skipped);
            if !extra_final.is_empty() {
                let sandbox = app.task_sandbox(&task.repo, worktree, run_dir);
                let finals = run_verify_commands(
                    CheckContext::new(app, Some(&task.id)).with_gate("final checks"),
                    worktree,
                    &run_dir.join("final-land"),
                    &extra_final,
                    sandbox.as_ref(),
                    &task.base_sha,
                    cancel,
                )
                .await;
                record_check_outcomes(app, task, &finals);
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
            let detail = conflict_detail(&branch, &new_sha, &files);
            set_carry_conflicts(task, files);
            return failed(FailureKind::Conflict, detail);
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
        let mut tasks = self.store.list_tasks().unwrap_or_else(|e| {
            eprintln!("orch: listing graph tasks failed: {e}");
            Vec::new()
        });
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
        let mut all = self.repo_tasks(repo);
        for task in &mut all {
            let ready = task
                .queue
                .park
                .as_ref()
                .and_then(|p| p.until.as_ref())
                .and_then(|id| self.store.load_task(id).ok().flatten())
                .is_some_and(|target| target.status == TaskStatus::Done);
            if ready && !task.archived && !self.controls.lock().unwrap().contains_key(&task.id) {
                task.queue.park = None;
                task.decisions
                    .push("Park: the until task is done, resuming".into());
                task.updated_at = now_ms();
                let _ = self.store.save_task(task);
                self.broadcast_task(task);
                self.spawn_task_loop(task.id.clone(), true);
            }
        }
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
                || self.is_suspended(task)
                || self.controls.lock().unwrap().contains_key(&task.id)
            {
                continue;
            }
            // Waiting for a file: `advance_queue` starts it when it is free.
            if self.file_blocked(task) {
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
                    || self.is_suspended(p)
                    || matches!(p.status, TaskStatus::Stopped | TaskStatus::Failed)
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
                        if let Err(e) = self.update_task(&task.id, |task| {
                            task.question = None;
                            task.status = TaskStatus::Queued;
                        }) {
                            eprintln!("orch: updating task {} failed: {e}", task.id);
                        }
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
                    if let Err(e) = self.update_task(&task.id, |task| {
                        task.question = None;
                        task.status = TaskStatus::Queued;
                    }) {
                        eprintln!("orch: updating task {} failed: {e}", task.id);
                    }
                    if unstarted_children {
                        self.spawn_task_loop(task.id.clone(), true)
                    }
                }
                Waits::Pending if unstarted_children => self.spawn_task_loop(task.id.clone(), true),
                Waits::Ended(ended) if !asking => {
                    self.ask_dependency_question(task, &all, &ended);
                }
                Waits::Pending if !parent && !asking => self.note_dependency_wait(task, &all),
                _ => {}
            }
        }
        self.advance_queue(repo);
        self.advance_autopilot();
    }

    /// A task with no loop that waits for other tasks shows which ones.
    fn note_dependency_wait(&self, task: &Task, all: &[Task]) {
        let wait = Wait::Dependency {
            titles: unmet_titles(task, all),
        };
        if task.queue.wait.as_ref() != Some(&wait) {
            if let Err(e) = self.update_task(&task.id, |t| t.queue.wait = Some(wait)) {
                eprintln!("orch: updating task {} failed: {e}", task.id);
            }
        }
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
            let wait = Wait::SubtaskSlot {
                running: running as u32,
                limit: cap as u32,
            };
            if child.queue.wait.as_ref() != Some(&wait) {
                if let Err(e) = self.update_task(&child.id, |t| t.queue.wait = Some(wait)) {
                    eprintln!("orch: updating task {} failed: {e}", child.id);
                }
            }
            return false;
        }
        self.spawn_task_loop(child.id.clone(), true);
        true
    }

    /// Parks `task` `waiting` on the question of what to do about the
    /// dependencies in `ended`.
    pub(super) fn ask_dependency_question(&self, task: &Task, all: &[Task], ended: &[String]) {
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
        if self.restart_environment_dependency(&task, all, ended) {
            return;
        }
        // The answer policy retries an ended dependency once before the owner
        // is asked.
        if let Some(question) = task.question.clone() {
            if !all
                .iter()
                .any(|t| ended.contains(&t.id) && t.queue.stopped_explicitly)
                && policy_open(self, &task, &question)
                && !task
                    .assumptions
                    .iter()
                    .any(|a| a.kind == Some(QuestionKind::DependencyEnded))
            {
                let answer = "retry the dependency";
                match self.answer_dependency_question(task.clone(), answer, None, None, None) {
                    Ok(_) => return,
                    Err(e) => eprintln!(
                        "orch: answering dependency for task {} failed: {e}",
                        task.id
                    ),
                }
            }
        }
        if let Err(e) = self.update_task(&task.id, |fresh| {
            fresh.question = task.question.clone();
            fresh.status = TaskStatus::Waiting;
        }) {
            eprintln!("orch: updating task {} failed: {e}", task.id);
        }
    }

    /// A dependency that failed on the environment (a timeout, a flaky or
    /// outside-the-diff check, an install error) is restarted once more by
    /// itself, after a pause; dropping a dependency stays the owner's.
    /// `true` when this call handled `task` (restarted it or is waiting out
    /// the pause); `task` carries the dependency question.
    pub(super) fn restart_environment_dependency(
        &self,
        task: &Task,
        all: &[Task],
        ended: &[String],
    ) -> bool {
        let deps: Vec<&Task> = all.iter().filter(|t| ended.contains(&t.id)).collect();
        if deps.is_empty()
            || task.loop_state.dependency_restarted
            || deps.iter().any(|d| {
                d.status != TaskStatus::Failed
                    || !ladder::agent_may_restart(d)
                    || d.queue.stopped_explicitly
                    || !ladder::failed_on_environment(d, &ladder::base_tails(self, &d.base_sha))
            })
        {
            return false;
        }
        let pause = ladder::dependency_restart_delay().as_millis() as i64;
        let left = pause - (now_ms() - deps.iter().map(|d| d.updated_at).max().unwrap_or(0));
        if left > 0 {
            // The graph comes back to this task when the pause is over.
            if tokio::runtime::Handle::try_current().is_ok() {
                let (app, repo) = (self.arc(), task.repo.clone());
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(left as u64 + 20)).await;
                    app.advance_graph(&repo);
                });
            }
            return true;
        }
        if let Err(e) = self.update_task(&task.id, |t| {
            t.loop_state.dependency_restarted = true;
            t.decisions
                .push(ladder::DEPENDENCY_RESTART_LINE.to_string())
        }) {
            eprintln!("orch: updating task {} failed: {e}", task.id);
            return false;
        }
        match self.answer_dependency_question(
            task.clone(),
            "retry the dependency",
            Some(AnsweredBy::Policy),
            None,
            None,
        ) {
            Ok(_) => true,
            Err(e) => {
                eprintln!(
                    "orch: restarting dependency of task {} failed: {e}",
                    task.id
                );
                false
            }
        }
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
            self.close_stopped_task(&mut child);
            let _ = self.interrupt_task(
                &mut child,
                None,
                true,
                "Orchestrator: stopped with its parent".into(),
            );
            self.stop_children(&child.id, repo);
        }
    }

    /// "retry" restarts every task this one waits for that ended
    /// `failed`/`stopped`; "drop" stops waiting for them (a dependency leaves
    /// `dependsOn`, a child is detached from this parent). Either way this
    /// task goes back to waiting for the rest.
    pub(super) fn answer_dependency_question(
        &self,
        task: Task,
        answer: &str,
        caller: Option<AnsweredBy>,
        default_evidence: Option<&str>,
        like: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let all = self.repo_tasks(&task.repo);
        let was_parent = is_parent(&task, &all);
        let ended: Vec<Task> = waits_for(&task, &all)
            .into_iter()
            .filter(|t| matches!(t.status, TaskStatus::Failed | TaskStatus::Stopped))
            .cloned()
            .collect();
        let by_owner = caller == Some(AnsweredBy::Owner);
        let choice = answer.trim().to_ascii_lowercase();
        if !choice.starts_with("retry") && !choice.starts_with("drop") {
            return Err("answer retry the dependency, drop the dependency, or stop".to_string());
        }
        if choice.starts_with("retry") {
            if !by_owner && ended.iter().any(|t| t.queue.stopped_explicitly) {
                return Err("an explicitly stopped dependency needs the owner's restart".into());
            }
            if !by_owner {
                let ids: Vec<String> = ended.iter().map(|t| t.id.clone()).collect();
                self.agent_restart_gate(&ids)?;
            }
            for dep in ended.iter().filter(|t| !t.archived) {
                let mut cancellations = self.cancellations.lock().unwrap();
                if let Some(end) = cancellations.get_mut(&dep.id) {
                    end.restart = true;
                    end.restart_children = is_parent(dep, &all);
                }
                // Queued before its loop runs, so nothing reads it as
                // ended again while it retries.
                // From the stored task: the restart gate just wrote its record there.
                let restart_children = is_parent(dep, &all);
                let Ok(dep) = self.update_task(&dep.id, |d| {
                    d.queue.restart_children = restart_children;
                    d.queue.stopped_explicitly = false;
                    d.queue.park = None;
                    d.question = None;
                    d.status = TaskStatus::Queued;
                }) else {
                    continue;
                };
                drop(cancellations);
                self.spawn_task_loop(dep.id.clone(), true);
            }
        } else if choice.starts_with("drop") {
            for child in ended
                .iter()
                .filter(|t| t.parent.as_deref() == Some(task.id.as_str()))
            {
                self.update_task(&child.id, |child| {
                    child.parent = None;
                    child.decisions.push(format!(
                        "{}: dropped from \"{}\"",
                        caller.filter(|_| default_evidence.is_none()).map_or(
                            "Policy",
                            |by| match by {
                                AnsweredBy::Owner => "Owner",
                                AnsweredBy::Lead => "Lead",
                                AnsweredBy::Overseer | AnsweredBy::Orchestrator => "Orchestrator",
                                _ => "Policy",
                            }
                        ),
                        task.title
                    ));
                })?;
            }
        }
        let fresh = self.repo_tasks(&task.repo);
        let task = self.update_task(&task.id, |current| {
            if choice.starts_with("drop") {
                current.depends_on.retain(|id| !ended.iter().any(|t| &t.id == id));
            }
            let question = current.question.clone();
            if let Some(caller) = caller {
                record_answer_for(current, question.as_ref(), answer.trim(), caller, default_evidence, like);
            } else if let Some(question) = task.question.as_ref() {
                record_policy_answer(
                    current, question, answer, "policy",
                    "a dependency that ended is retried once", 0,
                );
            }
            current.question = None;
            if was_parent && !is_parent(current, &fresh) {
                current.decisions.push(
                    "Orchestrator: no subtasks are left, so this task was stopped instead of implementing the whole request itself".to_string(),
                );
                current.status = TaskStatus::Stopped;
                self.close_stopped_task(current);
            } else {
                current.status = if is_parent(current, &fresh) {
                    TaskStatus::Running
                } else {
                    TaskStatus::Queued
                };
            }
        })?;
        if task.status == TaskStatus::Stopped {
            return serde_json::to_value(&task).map_err(|e| e.to_string());
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{init_git_repo, test_app};

    #[tokio::test]
    async fn a_pass_under_one_base_is_not_reused_under_another() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let counter = dir.path().join("counter");
        let commands = vec![format!(
            "echo \"$ORCHD_BASE_SHA\" >> '{}'",
            counter.display()
        )];
        let cancel = CancelToken::new();
        let run = |base| run_checks(&app, &repo, &commands, None, base, &cancel);
        let (outcomes, lines) = run("base-a").await;
        assert_eq!(outcomes[0].code, Some(0));
        assert!(lines.is_empty());
        let (outcomes, lines) = run("base-a").await;
        assert_eq!(outcomes[0].ms, 0);
        assert_eq!(lines.len(), 1);
        let (outcomes, lines) = run("base-b").await;
        assert_eq!(outcomes[0].code, Some(0));
        assert!(lines.is_empty());
        assert_eq!(
            std::fs::read_to_string(counter).unwrap(),
            "base-a\nbase-b\n"
        );
    }

    #[tokio::test]
    async fn a_timed_out_final_is_never_recorded_as_a_pass() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let commands = vec!["exit 137".into(), "false".into(), "true".into()];
        let cancel = CancelToken::new();
        let run = || run_checks(&app, &repo, &commands, None, "base", &cancel);
        let (first, lines) = run().await;
        assert_eq!(first[0].code, Some(137));
        assert_eq!(first[1].code, Some(1));
        assert_eq!(first[2].code, Some(0));
        assert_eq!(lines.len(), 1);
        let (second, lines) = run().await;
        assert_eq!(second[0].code, Some(137));
        assert_eq!(second[1].code, Some(1));
        assert_eq!(second[2].ms, 0);
        assert_eq!(lines.len(), 2);
    }
}
