use crate::clock::now_ms;
use crate::engine::decision::record_failure;
use crate::engine::planning::questions::ask_plan_question;
use crate::engine::planning::records::end_plan_stage;
use crate::engine::slots::SlotPermit;
use crate::task::AnsweredBy;
use crate::task::FailureKind;
use crate::task::TaskStatus;

use crate::engine::planning::round::PlannerRound;
use crate::engine::planning::run::PlanOutcome;
impl PlannerRound {
    pub(in crate::engine::planning) async fn clarify(
        &mut self,
        clarify_rounds: &mut u32,
        signals: &crate::engine::run_control::TaskSignals,
        permit: &mut Option<SlotPermit>,
    ) -> Option<PlanOutcome> {
        let app = &self.app;
        let task_id = self.task.id.clone();
        let task_id = task_id.as_str();
        let idx = self.index;
        let _pending_answer = &signals.pending_answer;
        let _cancel = &signals.cancel;
        let task = &mut self.task;

        record_failure(
            task,
            idx,
            FailureKind::NoDeliverable,
            "planner did not return a parseable sushi-plan block".to_string(),
        );
        if *clarify_rounds >= 2 {
            // Already asked the owner twice for more detail; a third
            // unparseable round in a row means this isn't going anywhere.
            task.mark_failed();
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
            return Some(end_plan_stage(app, task_id, Some(idx)).await);
        }
        *clarify_rounds += 1;
        app.save_task_logged(task);
        app.broadcast_task(task);
        let question = "The planner could not draft this task: edit it or answer with more detail";
        match ask_plan_question(
            app,
            task_id,
            crate::task::Question {
                risk: Vec::new(),
                default_option: None,
                ..crate::task::Question::new(
                    question,
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
        {
            Some(answer) => {
                let who = match app.take_agent_answer(task_id) {
                    Some(AnsweredBy::Lead) => "Lead",
                    Some(AnsweredBy::Policy | AnsweredBy::Judge) => "Policy",
                    Some(_) => "Orchestrator",
                    None => "Owner",
                };
                if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                    let combined = format!(
                        "{}\n\n({who} clarification: {answer})",
                        t.request.clone().unwrap_or_default()
                    );
                    t.record_request(combined);
                    t.resume_phase(TaskStatus::Drafting, crate::clock::now_ms());
                    t.touch(now_ms());
                    app.save_task_logged(&t);
                    app.broadcast_task(&t);
                }
                None
            }
            None => return Some(end_plan_stage(app, task_id, Some(idx)).await),
        }
    }
}
