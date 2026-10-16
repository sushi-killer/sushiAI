use crate::clock::now_ms;
use crate::engine::answer_policy::origin_of_last_answer;
use crate::engine::planning::graph::check_subtasks;
use crate::engine::planning::questions::ask_plan_question;
use crate::engine::planning::records::end_plan_stage;
use crate::engine::planning::subtasks::merge_into_checklist;
use crate::engine::planning::subtasks::prepare_subtasks;
use crate::engine::planning::subtasks::split_into_subtasks;
use crate::engine::slots::SlotPermit;
use crate::task::checks::authority::Origin;
use crate::task::Assumption;
use crate::task::CheckSource;
use crate::task::TaskStatus;
use crate::verify::options::verify_options;

use crate::engine::planning::round::PlannerRound;
use crate::engine::planning::run::PlanOutcome;
impl PlannerRound {
    pub(in crate::engine::planning) async fn split(
        &mut self,
        draft: &crate::engine::planning::draft::PlanDraft,
        auto_start_after_plan: bool,
    ) -> Option<PlanOutcome> {
        let app = &self.app;
        let task_id = self.task.id.clone();
        let task_id = task_id.as_str();
        let _idx = self.index;
        let task = &mut self.task;
        // The planner split the request: the task becomes a parent of one
        // child per part and never implements anything itself.
        if !draft.subtasks.is_empty() && task.parent.is_none() {
            let failed = match check_subtasks(&draft.subtasks) {
                Err(reason) => Some(format!("subtasks ignored: {reason}")),
                Ok(()) => {
                    let plan = prepare_subtasks(&draft.subtasks);
                    // One merged group and nothing else: the task itself
                    // keeps its draft and gains the ordered checklist.
                    if plan.merge.separate.is_empty() {
                        merge_into_checklist(app, task_id, &plan).err()
                    } else {
                        match split_into_subtasks(app, task_id, &plan, auto_start_after_plan).await
                        {
                            Ok(()) => {
                                return Some(if auto_start_after_plan {
                                    PlanOutcome::Proceed
                                } else {
                                    PlanOutcome::StoppedForReview
                                });
                            }
                            Err(e) => Some(format!("could not create the subtasks: {e}")),
                        }
                    }
                }
            };
            if let Some(why) = failed {
                app.append_decision(task_id, format!("Planner: {why}; running as one task"));
            }
            if let Ok(Some(reloaded)) = app.store.load_task(task_id) {
                *task = reloaded;
            }
        }

        None
    }
}
impl PlannerRound {
    pub(in crate::engine::planning) async fn verify_command(
        &mut self,
        signals: &crate::engine::run_control::TaskSignals,
        permit: &mut Option<SlotPermit>,
    ) -> Option<PlanOutcome> {
        let worktree = std::path::PathBuf::from(&self.task.worktree);
        let attempt_n = self.task.attempts[self.index].n;
        let app = &self.app;
        let task_id = self.task.id.clone();
        let task_id = task_id.as_str();
        let idx = self.index;
        let _pending_answer = &signals.pending_answer;
        let _cancel = &signals.cancel;
        let task = &mut self.task;
        // Unconditional on an empty `verify` (review item P2e): the first
        // command the repo offers becomes an assumption; the question is
        // asked only when the repo offers none.
        if task.verify.is_empty() {
            let mut options = verify_options(&worktree);
            let known = options
                .iter()
                .position(|o| crate::task::checks::commands::runs_ci_entry(o));
            if known.is_none() {
                match ask_plan_question(
                    app,
                    task_id,
                    crate::task::Question {
                        risk: Vec::new(),
                        default_option: None,
                        ..crate::task::Question::new(
                            "No verification command was found. Which command proves this task?",
                            options,
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
                        let _ = app.update_task(task_id, |task| {
                            let source = if origin_of_last_answer(task) == Origin::Owner {
                                CheckSource::Owner
                            } else {
                                CheckSource::Repo
                            };
                            crate::task::checks::edits::supply_verification(task, answer, source);
                        });
                    }
                    None => return Some(end_plan_stage(app, task_id, Some(idx)).await),
                }
            } else if let Ok(Some(mut t)) = app.store.load_task(task_id) {
                let command = options.swap_remove(known.unwrap_or_default());
                let question = "Which command proves this task?".to_string();
                app.broadcast_log(
                    task_id,
                    attempt_n,
                    format!("Planner assumed: {question} -> {command}"),
                );
                t.record_assumption(Assumption {
                    question,
                    answer: command.clone(),
                    evidence:
                        "the first verification command found in the repo that runs the whole suite"
                            .into(),
                    by: "planner".to_string(),
                    kind: None,
                    attempt: None,
                    commands: Vec::new(),
                    overturned: false,
                    owner_answer: None,
                });
                crate::task::checks::edits::supply_verification(&mut t, command, CheckSource::Repo);
                t.touch(now_ms());
                app.save_task_logged(&t);
                app.broadcast_task(&t);
            }
        }

        None
    }
}
impl PlannerRound {
    pub(in crate::engine::planning) fn finish(
        &self,
        auto_start_after_plan: bool,
        signals: &crate::engine::run_control::TaskSignals,
    ) -> PlanOutcome {
        let app = &self.app;
        let task_id = &self.task.id;
        let lock = app.task_lock(task_id);
        let _guard = lock.lock().unwrap();
        if let Ok(Some(mut t)) = app.store.load_task(task_id) {
            if signals.cancel.is_cancelled()
                || t.status != TaskStatus::Drafting
                || t.archived
                || t.queue.park.is_some()
            {
                return PlanOutcome::Ended;
            }
            t.resume_phase(
                if auto_start_after_plan {
                    TaskStatus::Queued
                } else {
                    TaskStatus::Stopped
                },
                crate::clock::now_ms(),
            );
            if t.status == TaskStatus::Stopped {
                app.close_stopped_task(&mut t);
            }
            t.touch(now_ms());
            if let Err(error) = app.store.save_task(&t) {
                eprintln!("orch: saving planned task {task_id} failed: {error}");
                return PlanOutcome::Ended;
            }
            app.broadcast_task(&t);
        }
        if auto_start_after_plan {
            PlanOutcome::Proceed
        } else {
            PlanOutcome::StoppedForReview
        }
    }
}
