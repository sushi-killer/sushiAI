use super::{record_policy_answer, AutomaticAnswer};
use crate::clock::now_ms;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::policy::settings::Settings;
use crate::task::checks::admission::CheckAdmission;
use crate::task::checks::rewrite::CheckRewrite;
use crate::task::checks::snapshot::CriterionEdit;
use crate::task::{Question, Task, TaskStatus};

pub(super) struct PolicyInput {
    contract: CheckAdmission,
    phase: TaskStatus,
    stored_question: Option<serde_json::Value>,
    stored_cost: f64,
    pub(super) question: Question,
    pub(super) attempt: u32,
}

pub(super) struct AnswerRuling {
    pub(super) answer: Option<AutomaticAnswer>,
    pub(super) rewrite: Option<(CriterionEdit, CheckRewrite)>,
    pub(super) cost: f64,
    pub(super) notes: Vec<String>,
}

pub(in crate::engine) enum PolicyResult {
    Answered(String),
    Open,
    Stale,
    Stopped,
}

impl PolicyInput {
    pub(super) fn capture(app: &App, task: &Task) -> Option<Self> {
        let stored = app.store.load_task(&task.id).ok()??;
        Some(Self {
            contract: CheckAdmission::capture(task),
            phase: stored.status,
            stored_question: stored
                .question
                .as_ref()
                .and_then(|question| serde_json::to_value(question).ok()),
            stored_cost: stored.cost_usd,
            question: task.question.clone()?,
            attempt: task.attempts.last().map_or(0, |attempt| attempt.n),
        })
    }
}

impl AnswerRuling {
    pub(super) fn rule(answer: AutomaticAnswer) -> Self {
        Self {
            answer: Some(answer),
            rewrite: None,
            cost: 0.0,
            notes: Vec::new(),
        }
    }

    fn apply(&mut self, settings: &Settings, task: &mut Task, input: &PolicyInput) {
        if let Some((edit, rewrite)) = &self.rewrite {
            if let Err(why) = super::planner::rewrite_criterion_check(settings, task, edit, rewrite)
            {
                if input.question.criterion.is_some_and(|index| {
                    super::planner::planner_check_criterion(settings, task, index)
                }) {
                    self.answer = Some(AutomaticAnswer::Policy {
                        text: "drop criterion".into(),
                        evidence: format!("the judge's rewrite to {} was not applied: {why}; every check of this criterion was written by the planner, so it is dropped without the owner", rewrite.command),
                    });
                } else {
                    self.answer = None;
                    task.record_decision(format!("Answer judge: {why}"));
                }
                self.rewrite = None;
            }
        }
        if let Some(answer) = &self.answer {
            record_policy_answer(task, &input.question, answer, input.attempt);
            task.clear_question();
            task.resume_phase(TaskStatus::Queued, now_ms());
        }
    }
}

pub(super) fn publish(
    app: &App,
    task: &mut Task,
    input: PolicyInput,
    mut ruling: AnswerRuling,
    cancel: &CancelToken,
) -> Result<PolicyResult, String> {
    let lock = app.task_lock(&task.id);
    let guard = lock.lock().unwrap();
    let mut fresh = app
        .store
        .load_task(&task.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    fresh.charge(ruling.cost);
    fresh.record_decisions(ruling.notes.clone());
    let controls = app.controls.lock().unwrap();
    let pending = controls
        .get(&task.id)
        .map(|control| control.signals.pending_amend.lock().unwrap());
    let stopped = cancel.is_cancelled()
        || fresh.archived
        || fresh.queue.park.is_some()
        || matches!(
            fresh.status,
            TaskStatus::Stopped | TaskStatus::Done | TaskStatus::Failed
        );
    let stale = fresh.pending_amendment.is_some()
        || fresh.status != input.phase
        || fresh
            .question
            .as_ref()
            .and_then(|question| serde_json::to_value(question).ok())
            != input.stored_question
        || !input.contract.matches(&fresh)
        || pending.as_deref().is_some_and(|edit| edit.is_some());
    let settings = app.settings.read().unwrap().clone();
    let result = if stopped {
        PolicyResult::Stopped
    } else if stale {
        PolicyResult::Stale
    } else {
        ruling.apply(&settings, &mut fresh, &input);
        match &ruling.answer {
            Some(answer) => PolicyResult::Answered(answer.parts().0.to_string()),
            None => PolicyResult::Open,
        }
    };
    fresh.touch(now_ms());
    app.store
        .save_task(&fresh)
        .map_err(|error| error.to_string())?;
    if !matches!(result, PolicyResult::Stopped) {
        if matches!(result, PolicyResult::Answered(_)) {
            if let Some((edit, rewrite)) = &ruling.rewrite {
                super::planner::rewrite_criterion_check(&settings, task, edit, rewrite)?;
            }
            task.adopt_answer_receipt(&fresh);
        }
        task.charge(fresh.cost_usd - input.stored_cost);
        for line in &fresh.decisions {
            if !task.decisions.contains(line) {
                task.record_decision(line.clone());
            }
        }
    } else {
        *task = fresh.clone();
    }
    drop(pending);
    drop(controls);
    drop(guard);
    app.broadcast_task(&fresh);
    if matches!(result, PolicyResult::Answered(_)) {
        if let Some(answer) = &ruling.answer {
            let (text, by, evidence) = answer.parts();
            app.broadcast_log(
                &task.id,
                input.attempt,
                crate::engine::lines::policy_answer_line(&input.question.text, text, by, evidence),
            );
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
