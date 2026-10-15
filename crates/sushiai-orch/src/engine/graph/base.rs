use crate::clock::now_ms;
use crate::engine::answer_policy::origin_of_last_answer;
use crate::engine::answer_policy::remove_check_command;
use crate::engine::baseline::judge_against_base;
use crate::engine::baseline::BaseVerdict;
use crate::engine::lifecycle::base_wait::carry_after_base_moves;
use crate::engine::lifecycle::base_wait::BaseRetry;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::questions::parking::wait_for_answer;
use crate::engine::questions::reply::Reply;
use crate::engine::state::App;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::engine::verification::base::answers::answerer_label;
use crate::engine::verification::base::answers::base_check_answered;
use crate::engine::verification::base::answers::base_retry_requested;
use crate::engine::verification::base::answers::command_named_by_task;
use crate::engine::verification::base::answers::is_kept;
use crate::engine::verification::base::answers::keep_check;
use crate::engine::verification::base::answers::keep_planned_checks;
use crate::engine::verification::base::answers::record_owner_base_check;
use crate::engine::verification::base::answers::KEEP_ALL;
use crate::engine::verification::base::answers::KEPT_BY_TASK_TARGET;
use crate::engine::verification::base::answers::PRE_EXISTING_DROP;
use crate::engine::verification::base::checks::failing_on_base;
use crate::engine::verification::base::checks::failing_twice_on_base;
use crate::engine::verification::base::checks::start_final_base_jobs;
use crate::engine::verification::base::questions::parent_checks_question;
use crate::engine::verification::base::questions::pre_existing_question;
use crate::harness::cancel::CancelToken;
use crate::policy::settings::Settings;
use crate::task::amendment::PendingAmendment;
use crate::task::checks::authority::Origin;
use crate::task::is_option;
use crate::task::Question;
use crate::task::QuestionKind;
use crate::task::Task;
use crate::task::TaskStatus;
use crate::task::VerifyOutcome;
use crate::task::PRE_EXISTING_RETRY;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use tokio::sync::oneshot;

/// Applies the owner's (or the policy's) answer to a base-check question of
/// any task: `drop all` removes the commands, `keep all` records that they
/// stay. Shared by the live loops and the post-restart branch of
/// `task.answer`. `true` when at least one command was removed. A question
/// without `commands` (persisted before they existed) applies nothing.
pub(in crate::engine) fn apply_base_check_answer(
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
                    task.record_decision(format!("Orchestrator: dropped check {command}"));
                    dropped = true;
                }
                // Refused: the check stays, the answer counts as a retry.
                Err(why) => {
                    task.record_decision(format!("Orchestrator: check {command} stays: {why}"))
                }
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
pub(in crate::engine) async fn parent_base_preflight(
    app: &Arc<App>,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    pending_amend: &Arc<StdMutex<Option<PendingAmendment>>>,
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
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
        }
        failed.retain(|f| !kept.contains(&f.command));
        if failed.is_empty() {
            return true;
        }
        let question = parent_checks_question(&failed, &task.base_sha);
        task.ask(question.clone(), now_ms());
        task.resume_phase(TaskStatus::Waiting, crate::clock::now_ms());
        task.touch(now_ms());
        let task_id = task.id.clone();
        let assumed = task.assumptions.len();
        let Some(answer) = wait_for_answer(app, task, pending_answer, cancel, &mut None).await
        else {
            return false;
        };
        let policy_answered = task.assumptions.len() > assumed;
        if is_option(&answer, "stop") {
            if let Some(who) = answerer_label(task) {
                task.record_decision(format!("{who}: stop"));
            }
            task.clear_question();
            task.resume_phase(TaskStatus::Stopped, crate::clock::now_ms());
            app.close_stopped_task(task);
            task.touch(now_ms());
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
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
}

/// What the parent's gate does after its checks ran.
pub(in crate::engine) enum ParentGate {
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
pub(in crate::engine) async fn parent_finals_on_base(
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
    let (on_base, judged) = match classify_parent_finals(app, task, &failing_finals, cancel).await {
        Ok(classified) => classified,
        Err(end) => return end,
    };
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
        task.ask(question.clone(), now_ms());
        task.resume_phase(TaskStatus::Waiting, crate::clock::now_ms());
        task.touch(now_ms());
        let Some(answer) = wait_for_answer(app, task, pending_answer, cancel, &mut None).await
        else {
            return ParentGate::End;
        };
        answer
    };
    let policy_answered = task.assumptions.len() > assumed;
    if is_option(&answer, "stop") {
        stop_parent_after_answer(app, task);
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
    task.resume_phase(TaskStatus::Running, crate::clock::now_ms());
    task.touch(now_ms());
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
        return retry_parent_base(app, task, pending_answer, cancel).await;
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

async fn classify_parent_finals(
    app: &Arc<App>,
    task: &mut Task,
    failing_finals: &[VerifyOutcome],
    cancel: &CancelToken,
) -> Result<(Vec<VerifyOutcome>, Vec<String>), ParentGate> {
    let dir = app
        .store
        .task_dir(&task.id)
        .join("runs")
        .join("parent")
        .join("final-base");
    let mut on_base: Vec<VerifyOutcome> = Vec::new();
    for failed in failing_finals {
        if let Some(base_run) = failing_on_base(app, task, failed, &dir, cancel).await {
            on_base.push(base_run);
        }
    }
    if cancel.is_cancelled() {
        return Err(ParentGate::End);
    }
    let names: Vec<String> = on_base.iter().map(|b| b.command.clone()).collect();
    let kept = keep_planned_checks(app, task, &names);
    if !kept.is_empty() {
        task.touch(now_ms());
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
    Ok((unclear, judged))
}

async fn retry_parent_base(
    app: &Arc<App>,
    task: &mut Task,
    pending_answer: &Arc<StdMutex<Option<oneshot::Sender<Reply>>>>,
    cancel: &CancelToken,
) -> ParentGate {
    let wt = PathBuf::from(&task.worktree);
    match carry_after_base_moves(app, task, &wt, pending_answer, cancel, &mut None).await {
        BaseRetry::Ready => {}
        BaseRetry::Conflict(detail) => {
            task.resume_phase(TaskStatus::Running, crate::clock::now_ms());
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
            return ParentGate::Conflict(detail);
        }
        BaseRetry::Stopped => return ParentGate::End,
    }
    app.save_task_logged(task);
    app.broadcast_task(task);
    ParentGate::Again
}

fn stop_parent_after_answer(app: &Arc<App>, task: &mut Task) {
    if let Some(who) = answerer_label(task) {
        task.record_decision(format!("{who}: stop"));
    }
    task.clear_question();
    task.resume_phase(TaskStatus::Stopped, crate::clock::now_ms());
    app.close_stopped_task(task);
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    app.stop_children(&task.id, &task.repo);
}
