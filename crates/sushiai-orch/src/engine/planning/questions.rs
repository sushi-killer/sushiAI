use crate::clock::now_ms;
use crate::engine::answer_policy::record_answered_question;
use crate::engine::questions::answers::record_answer_for;
use crate::engine::questions::reply::Reply;
use crate::engine::questions::wake::wake_orchestrator_for_waiting;
use crate::engine::slots::SlotPermit;
use crate::engine::state::App;
use crate::task::AnsweredBy;
use crate::task::Question;
use crate::task::TaskStatus;
use std::sync::Arc;
use tokio::sync::oneshot;

/// The one owner question for a plan's blocking questions: each with its
/// options and the planner's recommendation, answered in one free-text reply.
pub(in crate::engine) fn batched_question_text(
    questions: &[&crate::engine::planning::draft::PlanQuestion],
) -> String {
    let mut out = format!(
        "The planner has {} blocking question(s); answer them all in one reply:\n",
        questions.len()
    );
    for (i, q) in questions.iter().enumerate() {
        out.push_str(&format!("\n{}. {}", i + 1, q.text));
        if !q.options.is_empty() {
            out.push_str(&format!(" (options: {})", q.options.join(" / ")));
        }
        if let Some(r) = q.usable_recommendation() {
            out.push_str(&format!(" -- recommended: {r}"));
        }
        if !q.evidence.trim().is_empty() {
            out.push_str(&format!(" ({})", q.evidence.trim()));
        }
    }
    out
}

/// Sets the question/waiting state, parks on a fresh one-shot the same way
/// [`wait_for_answer`] does, then records the decision as `"Owner: <question>
/// -> <answer>"` (spec step 5) instead of the generic attempt-failure
/// phrasing. `"stop"` is still handled centrally by `handle_task_answer`
/// before it ever reaches here.
pub(in crate::engine) async fn ask_plan_question(
    app: &Arc<App>,
    task_id: &str,
    question: Question,
    signals: &crate::engine::run_control::TaskSignals,
    permit: &mut Option<SlotPermit>,
) -> Option<String> {
    let question_text = question.text.clone();
    let pending_answer = &signals.pending_answer;
    let cancel = &signals.cancel;
    // Same as wait_for_answer: a task parked on a question holds no slot.
    permit.take();
    // Install the one-shot *before* the state that makes this externally
    // visible as "waiting" is even persisted: otherwise a `task.answer`
    // that arrives the instant the broadcast reaches a client could beat
    // this into existence and get rejected as "no live parked loop" even
    // though a loop genuinely is about to park.
    let (tx, rx) = oneshot::channel();
    *pending_answer.lock().unwrap() = Some(tx);
    if let Ok(Some(mut task)) = app.store.load_task(task_id) {
        task.ask(question, now_ms());
        task.resume_phase(TaskStatus::Waiting, crate::clock::now_ms());
        task.touch(now_ms());
        app.save_task_logged(&task);
        // The wake may hold the question or hand it to the owner: the
        // broadcast that shows the waiting task carries either.
        wake_orchestrator_for_waiting(app, &task);
        let shown = app.store.load_task(task_id).ok().flatten().unwrap_or(task);
        app.broadcast_task(&shown);
    }

    let reply = tokio::select! {
        _ = cancel.cancelled() => None,
        answer = rx => answer.ok(),
    };
    *pending_answer.lock().unwrap() = None;
    let result = reply.as_ref().map(|r| r.text.clone());

    record_plan_reply(app, task_id, &question_text, reply.as_ref());
    result
}

fn record_plan_reply(app: &Arc<App>, task_id: &str, question_text: &str, reply: Option<&Reply>) {
    match reply {
        Some(reply) => {
            let answer = &reply.text;
            let evidence = &reply.default_evidence;
            let like = &reply.like;
            if let Ok(Some(mut task)) = app.store.load_task(task_id) {
                // Peeked: the caller reads the mark again for its own wording.
                let caller = app
                    .agent_answers
                    .lock()
                    .unwrap()
                    .get(task_id)
                    .copied()
                    .unwrap_or(AnsweredBy::Owner);
                if caller != AnsweredBy::Owner || evidence.is_some() {
                    let question = task.take_question();
                    record_answer_for(
                        &mut task,
                        question.as_ref(),
                        &crate::engine::questions::reply::Reply {
                            text: (answer).to_string(),
                            default_evidence: (evidence.as_deref()).map(str::to_string),
                            like: (like.as_deref()).map(str::to_string),
                        },
                        caller,
                    );
                } else {
                    task.record_decision(format!("Owner: {question_text} -> {answer}"));
                    if let Some(question) = task.take_question() {
                        record_answered_question(&mut task, &question, answer, AnsweredBy::Owner);
                    }
                }
                task.resume_phase(TaskStatus::Drafting, crate::clock::now_ms());
                task.touch(now_ms());
                app.save_task_logged(&task);
                app.broadcast_task(&task);
            }
        }
        None => {
            if let Ok(Some(mut task)) = app.store.load_task(task_id) {
                app.apply_cancellation(&mut task);
                if task.status == TaskStatus::Stopped {
                    app.close_stopped_task(&mut task);
                }
                task.touch(now_ms());
                app.save_task_logged(&task);
                app.broadcast_task(&task);
                app.advance_queue(&task.repo);
            }
        }
    }
}
