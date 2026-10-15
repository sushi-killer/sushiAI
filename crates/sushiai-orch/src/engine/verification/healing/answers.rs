use crate::engine::brief_check::judge::JudgeRun;
use crate::engine::state::App;
use crate::engine::verification::healing::contract::amend_criterion;
use crate::engine::verification::healing::contract::judge_route;
use crate::engine::verification::healing::contract::overturnable;
use crate::harness::cancel::CancelToken;
use crate::harness::native::error::RunError;
use crate::prompts::clip::truncate_chars;
use crate::task::state::Task;
use crate::verify::healing::reply::answer_prompt;
use crate::verify::healing::reply::new_answers;
use crate::verify::healing::reply::parse_amendments;
use crate::verify::healing::reply::MAX_REASON_CHARS;
use std::sync::Arc;

use crate::clock::now_ms;
use crate::policy::settings::Settings;
use crate::task::checks::admission::CheckAdmission;
use crate::task::state::TaskStatus;

pub(in crate::engine) enum AnswerHealing {
    Ready,
    Retry,
    Stopped,
}

struct AnswerInput {
    contract: CheckAdmission,
    phase: TaskStatus,
    answers: Vec<String>,
    seen: usize,
    stored_cost: f64,
}

struct AnswerRuling {
    amendments: Vec<(usize, String)>,
    cost: f64,
    timeout_note: Option<String>,
}

/// Publishes an answer ruling only against the exact contract and answers it read.
pub(in crate::engine) async fn heal_answers(
    app: &Arc<App>,
    task: &mut Task,
    attempt_n: u32,
    cancel: &CancelToken,
) -> Result<AnswerHealing, RunError> {
    let input = AnswerInput {
        contract: CheckAdmission::capture(task),
        phase: task.status,
        answers: new_answers(task),
        seen: task.brief_check.answers_seen,
        stored_cost: app
            .store
            .load_task(&task.id)
            .map_err(|e| RunError::Io(e.to_string()))?
            .ok_or_else(|| RunError::Io("task not found".into()))?
            .cost_usd,
    };
    let ruling = if input.answers.is_empty() || task.criteria.is_empty() {
        AnswerRuling {
            amendments: Vec::new(),
            cost: 0.0,
            timeout_note: None,
        }
    } else {
        let Some(route) = judge_route(app, &task.variant()) else {
            return Ok(AnswerHealing::Ready);
        };
        let prompt = answer_prompt(task, &input.answers);
        let judged = crate::engine::brief_check::judge::Judge::new(
            &route,
            crate::engine::brief_check::judge::JudgePurpose::AnswerCheck(attempt_n),
        )
        .run(app, task, &prompt, cancel)
        .await;
        let JudgeRun {
            reply,
            cost,
            timeout_note,
        } = match judged {
            Ok(reply) => reply,
            Err(error) => {
                if let Ok(Some(fresh)) = app.store.load_task(&task.id) {
                    *task = fresh;
                }
                return Err(error);
            }
        };
        let amendments = reply.ok().map_or_else(Vec::new, |text| {
            parse_amendments(&text, task.criteria.len())
        });
        AnswerRuling {
            amendments,
            cost,
            timeout_note,
        }
    };
    publish(app, task, input, ruling, cancel).map_err(RunError::Io)
}

fn publish(
    app: &App,
    task: &mut Task,
    input: AnswerInput,
    ruling: AnswerRuling,
    cancel: &CancelToken,
) -> Result<AnswerHealing, String> {
    let lock = app.task_lock(&task.id);
    let _guard = lock.lock().unwrap();
    let mut fresh = app
        .store
        .load_task(&task.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    fresh.charge(ruling.cost);
    fresh.record_decisions(ruling.timeout_note.clone());
    let controls = app.controls.lock().unwrap();
    let pending = controls
        .get(&task.id)
        .map(|control| control.signals.pending_amend.lock().unwrap());
    let stopped = cancel.is_cancelled()
        || fresh.archived
        || fresh.queue.park.is_some()
        || matches!(
            fresh.status,
            TaskStatus::Stopped | TaskStatus::Done | TaskStatus::Failed | TaskStatus::Landing
        );
    let stale = !matches!(fresh.status, TaskStatus::Queued | TaskStatus::Running)
        || fresh.status != input.phase
        || !input.contract.matches(&fresh)
        || fresh.brief_check.answers_seen != input.seen
        || fresh.pending_amendment.is_some()
        || new_answers(&fresh) != input.answers
        || pending.as_deref().is_some_and(|edit| edit.is_some());
    let settings = app.settings.read().unwrap().clone();
    let outcome = if stopped {
        AnswerHealing::Stopped
    } else if stale {
        AnswerHealing::Retry
    } else {
        apply_ruling(&settings, &mut fresh, &ruling.amendments, &input.answers);
        fresh.record_answer_progress(fresh.decisions.len());
        AnswerHealing::Ready
    };
    fresh.touch(now_ms());
    app.store.save_task(&fresh).map_err(|e| e.to_string())?;
    match outcome {
        AnswerHealing::Ready | AnswerHealing::Retry => {
            if matches!(outcome, AnswerHealing::Ready) {
                apply_ruling(&settings, task, &ruling.amendments, &input.answers);
            }
            task.charge(fresh.cost_usd - input.stored_cost);
            for line in &fresh.decisions {
                if !task.decisions.contains(line) {
                    task.record_decision(line.clone());
                }
            }
            if matches!(outcome, AnswerHealing::Ready) {
                task.record_answer_progress(task.decisions.len());
            }
        }
        AnswerHealing::Stopped => *task = fresh.clone(),
    }
    drop(pending);
    drop(controls);
    drop(_guard);
    app.broadcast_task(&fresh);
    Ok(outcome)
}

fn apply_ruling(
    settings: &Settings,
    task: &mut Task,
    amendments: &[(usize, String)],
    answers: &[String],
) {
    for (i, text) in amendments.iter().cloned() {
        let old = task.criteria[i].clone();
        if old == text {
            continue;
        }
        if let Err(why) = amend_criterion(settings, task, i, &text) {
            task.record_decision(format!(
                "Orchestrator: criterion \"{old}\" stays as written: {why}"
            ));
            continue;
        }
        task.record_decision(format!(
            "Orchestrator: criterion \"{old}\" contradicts the owner's answer; amended to \"{text}\""
        ));
        overturnable(
            task,
            format!("Does the criterion \"{old}\" still hold after the owner's answer?"),
            format!("No: amended to \"{text}\""),
            truncate_chars(&answers.join(" | "), MAX_REASON_CHARS),
        );
    }
}

#[cfg(test)]
mod tests;
