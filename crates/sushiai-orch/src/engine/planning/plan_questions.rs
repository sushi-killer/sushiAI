use crate::clock::now_ms;
use crate::engine::answer_policy::{policy_open, record_policy_answer};
use crate::engine::planning::draft::{PlanDraft, PlanQuestion};
use crate::engine::planning::questions::{ask_plan_question, batched_question_text};
use crate::engine::planning::records::end_plan_stage;
use crate::engine::planning::round::PlannerRound;
use crate::engine::planning::run::PlanOutcome;
use crate::engine::run_control::TaskSignals;
use crate::engine::slots::SlotPermit;
use crate::task::{AnsweredBy, AskedBy, Question, QuestionKind};

impl PlannerRound {
    pub(in crate::engine::planning) async fn answer_questions(
        &mut self,
        draft: &PlanDraft,
        signals: &TaskSignals,
        permit: &mut Option<SlotPermit>,
    ) -> Option<PlanOutcome> {
        let asked = draft.questions.iter().take(3).collect::<Vec<_>>();
        if self.task.variant().batch_questions {
            let (routine, mut blocking): (Vec<_>, Vec<_>) =
                asked.into_iter().partition(|question| {
                    !question.blocking && question.usable_recommendation().is_some()
                });
            match self.routine_questions(routine, signals, permit).await {
                Ok(mut remaining) => blocking.append(&mut remaining),
                Err(outcome) => return Some(outcome),
            }
            self.task.touch(now_ms());
            self.app.save_task_logged(&self.task);
            self.app.broadcast_task(&self.task);
            if !blocking.is_empty() {
                let text = batched_question_text(&blocking);
                self.reload();
                let risk = blocking
                    .iter()
                    .flat_map(|question| question.risk.clone())
                    .collect();
                if ask_plan_question(
                    &self.app,
                    &self.task.id,
                    crate::task::Question {
                        risk,
                        default_option: None,
                        ..crate::task::Question::new(
                            &text,
                            vec![],
                            crate::task::QuestionKind::PlanQuestion,
                            crate::task::AskedBy::Plan,
                            crate::clock::now_ms(),
                        )
                    },
                    signals,
                    permit,
                )
                .await
                .is_none()
                {
                    return Some(end_plan_stage(&self.app, &self.task.id, Some(self.index)).await);
                }
            }
        } else {
            for question in asked {
                self.reload();
                if ask_plan_question(
                    &self.app,
                    &self.task.id,
                    crate::task::Question {
                        risk: question.risk.clone(),
                        default_option: question.default_option,
                        ..crate::task::Question::new(
                            &question.text,
                            question.options.clone(),
                            crate::task::QuestionKind::PlanQuestion,
                            crate::task::AskedBy::Plan,
                            crate::clock::now_ms(),
                        )
                    },
                    signals,
                    permit,
                )
                .await
                .is_none()
                {
                    return Some(end_plan_stage(&self.app, &self.task.id, Some(self.index)).await);
                }
            }
        }
        None
    }

    async fn routine_questions<'a>(
        &mut self,
        routine: Vec<&'a PlanQuestion>,
        signals: &TaskSignals,
        permit: &mut Option<SlotPermit>,
    ) -> Result<Vec<&'a PlanQuestion>, PlanOutcome> {
        self.reload();
        let app = &self.app;
        let task_id = self.task.id.clone();
        let task_id = task_id.as_str();
        let idx = self.index;
        let attempt_n = self.task.attempts[idx].n;
        let _pending_answer = &signals.pending_answer;
        let _cancel = &signals.cancel;
        let task = &mut self.task;
        let mut blocking = Vec::new();
        for q in routine {
            // Planning reruns until a plan attempt passes, so the same
            // question can come back after an interruption: record it once.
            if task.assumptions.iter().any(|a| a.question == q.text) {
                continue;
            }
            let asked_q = Question::new(
                q.text.clone(),
                q.options.clone(),
                QuestionKind::PlanQuestion,
                AskedBy::Plan,
                crate::clock::now_ms(),
            );
            // The overseer settles it only while the answer policy and
            // auto-answer allow; otherwise it joins the owner's batch.
            let auto_answer = app.settings.read().unwrap().auto_answer;
            if !auto_answer || !policy_open(app, task, &asked_q) {
                blocking.push(q);
                continue;
            }
            let Some(answer) = ask_plan_question(
                app,
                task_id,
                crate::task::Question {
                    risk: q.risk.clone(),
                    default_option: q.default_option,
                    ..crate::task::Question::new(
                        &q.text,
                        q.options.clone(),
                        crate::task::QuestionKind::PlanQuestion,
                        crate::task::AskedBy::Plan,
                        crate::clock::now_ms(),
                    )
                },
                signals,
                permit,
            )
            .await
            else {
                return Err(end_plan_stage(app, task_id, Some(idx)).await);
            };
            let overseer = app.take_agent_answer(task_id) == Some(AnsweredBy::Overseer);
            if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                *task = reloaded;
            }
            if !overseer {
                continue;
            }
            record_policy_answer(
                task,
                &asked_q,
                &crate::engine::answer_policy::AutomaticAnswer::Policy {
                    text: answer.to_string(),
                    evidence: q.evidence.to_string(),
                },
                attempt_n,
            );
            app.broadcast_log(
                task_id,
                attempt_n,
                task.decisions.last().cloned().unwrap_or_default(),
            );
            // Saved before the next question reloads the task from the store.
            app.save_task_logged(task);
            app.broadcast_task(task);
        }

        Ok(blocking)
    }

    pub(in crate::engine::planning) fn reload(&mut self) {
        if let Ok(Some(task)) = self.app.store.load_task(&self.task.id) {
            self.task = task;
        }
    }
}
