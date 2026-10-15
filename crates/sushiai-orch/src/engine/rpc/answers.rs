use crate::api::validation::validate_task_id;
use crate::clock::now_ms;
use crate::engine::answer_policy::origin_of_last_answer;
use crate::engine::cost::is_budget_raise;
use crate::engine::cost::is_daily_budget_run_anyway;
use crate::engine::decision::drop_criterion;
use crate::engine::decision::impossible_drop_target;
use crate::engine::decision_memory::checked_like;
use crate::engine::graph::base::apply_base_check_answer;
use crate::engine::graph::dependencies::DEPENDENCY_QUESTION;
use crate::engine::messages;
use crate::engine::questions::answers::non_owner_refusal;
use crate::engine::questions::answers::record_answer_for;
use crate::engine::questions::reply::Reply;
use crate::engine::state::App;
use crate::engine::verification::base::answers::record_owner_base_check;
use crate::engine::verification::base::answers::PRE_EXISTING_DROP;
use crate::overseer;
use crate::task::is_option;
use crate::task::messages::ORCHESTRATOR;
use crate::task::AnsweredBy;
use crate::task::QuestionKind;
use crate::task::TaskStatus;
use serde::Deserialize;

impl App {
    /// `task.overturn {id, index, answer}`: the owner disagrees with one of
    /// the planner's assumptions. It is marked overturned and, unless the
    /// task is done, the answer reaches the task's next attempt as a message.
    pub(in crate::engine) async fn handle_task_overturn(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            index: usize,
            answer: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let answer = p.answer.trim();
        if answer.is_empty() {
            return Err("answer is required".to_string());
        }
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let assumption = task
            .assumptions
            .get(p.index)
            .ok_or_else(|| format!("task has no assumption {}", p.index))?;
        let text = format!(
            "The owner overturned an assumption made for them ({}). Question: {}\nThe assumption: {}\nThe owner's answer: {answer}\nUse the owner's answer.",
            assumption.by, assumption.question, assumption.answer
        );
        if task.status != TaskStatus::Done {
            messages::send(self, ORCHESTRATOR, Some(&task.id), &text, None)?;
        }
        let task = self.update_task(&p.id, |task| {
            task.overturn_assumption(p.index, answer.to_string());
        })?;
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }
}

impl App {
    pub(in crate::engine) async fn handle_task_answer(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let p: AnswerRequest = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let (caller, default_evidence) = p.authority()?;
        let mut task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if task.status != TaskStatus::Waiting || task.question.is_none() {
            return Err("task is not waiting for an answer".to_string());
        }
        let like = checked_like(p.like.as_deref(), p.source.as_deref(), &task, &p.answer)?;
        self.verify_like(like.as_deref(), &task, &p.answer)?;
        let by_agent = caller != AnsweredBy::Owner;
        if by_agent {
            if let Some(q) = task.question.as_ref() {
                let settings = self.settings.read().unwrap();
                if let Some(why) = non_owner_refusal(&settings, &task, q, &p.answer, caller) {
                    return Err(why);
                }
            }
            self.agent_answers
                .lock()
                .unwrap()
                .insert(p.id.clone(), caller);
            // Only a turn orchd woke is the overseer; the owner's chat is not.
            if p.source.as_deref() == Some("overseer") {
                self.record_overseer(overseer::Entry::answered(
                    &task.repo,
                    &task.id,
                    task.question.as_ref(),
                    &p.answer,
                ));
            }
        } else {
            self.agent_answers.lock().unwrap().remove(&p.id);
        }

        let reply = Reply {
            text: p.answer.clone(),
            default_evidence: default_evidence.clone(),
            like: like.clone(),
        };
        if p.answer.trim().eq_ignore_ascii_case("stop") {
            return self.stop_answer(&mut task, &reply, caller);
        }

        if task
            .question
            .as_ref()
            .is_some_and(|q| q.text.starts_with(DEPENDENCY_QUESTION))
        {
            self.agent_answers.lock().unwrap().remove(&p.id);
            return self.answer_dependency_question(
                task,
                &crate::engine::questions::reply::Reply {
                    text: p.answer.to_string(),
                    default_evidence: (default_evidence.as_deref()).map(str::to_string),
                    like: (like.as_deref()).map(str::to_string),
                },
                Some(caller),
            );
        }

        let delivered = match self.deliver_parked_answer(&task, &reply)? {
            AnswerDelivery::Permission => {
                return serde_json::to_value(&task).map_err(|e| e.to_string())
            }
            AnswerDelivery::Loop => true,
            AnswerDelivery::Idle => false,
        };
        if !delivered {
            self.apply_unparked_answer(&p.id, &reply, caller)?;
        }
        let latest = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        serde_json::to_value(&latest).map_err(|e| e.to_string())
    }
}

#[derive(Deserialize)]
struct AnswerRequest {
    id: String,
    answer: String,
    /// The bridge sets chat/mcp/overseer; raw RPC may set lead.
    /// Omitted means the owner in the app.
    #[serde(default)]
    source: Option<String>,
    /// With source `policy` (the waiting SLA): why this answer was given.
    #[serde(default)]
    evidence: Option<String>,
    /// With source `overseer`: the precedent or repo note this answer
    /// follows (see `decision_memory.rs`).
    #[serde(default)]
    like: Option<String>,
}

impl App {
    fn stop_answer(
        &self,
        task: &mut crate::task::Task,
        reply: &Reply,
        caller: AnsweredBy,
    ) -> Result<serde_json::Value, String> {
        self.agent_answers.lock().unwrap().remove(&task.id);
        let question = task.take_question();
        record_answer_for(
            task,
            question.as_ref(),
            &crate::engine::questions::reply::Reply {
                text: ("stop").to_string(),
                default_evidence: (reply.default_evidence.as_deref()).map(str::to_string),
                like: (reply.like.as_deref()).map(str::to_string),
            },
            caller,
        );
        let line = match caller {
            AnsweredBy::Owner => "Owner: stop (UI)".to_string(),
            AnsweredBy::Lead => "Lead: stop (RPC)".to_string(),
            _ => task.decisions.last().cloned().unwrap_or_default(),
        };
        if matches!(caller, AnsweredBy::Owner | AnsweredBy::Lead) {
            task.replace_last_decision(line.clone());
        }
        self.close_stopped_task(task);
        self.interrupt_task(task, None, true, line)?;
        self.stop_children(&task.id, &task.repo);
        self.advance_graph(&task.repo);
        serde_json::to_value(&task).map_err(|e| e.to_string())
    }
    fn apply_unparked_answer(
        &self,
        id: &str,
        reply: &Reply,
        caller: AnsweredBy,
    ) -> Result<(), String> {
        // No live parked loop (e.g. after a daemon restart): apply the
        // decision synchronously and relaunch (spec step 9: "status
        // queued, loop continues").
        self.update_task(id, |task| {
            let question = task.question.clone();
            record_answer_for(
                task,
                question.as_ref(),
                &crate::engine::questions::reply::Reply {
                    text: reply.text.to_string(),
                    default_evidence: (reply.default_evidence.as_deref()).map(str::to_string),
                    like: (reply.like.as_deref()).map(str::to_string),
                },
                caller,
            );
            self.agent_answers.lock().unwrap().remove(id);
            if let Some(n) = task
                .question
                .as_ref()
                .and_then(|q| impossible_drop_target(q, &reply.text))
            {
                let settings = self.settings.read().unwrap().clone();
                let origin = origin_of_last_answer(task);
                let _ = drop_criterion(&settings, task, n, origin);
            }
            if let Some(q) = task.question.clone() {
                // A base-check question: the task starts again only with the
                // answer applied, and the owner's answer is remembered.
                if q.kind == QuestionKind::PreexistingFailure && !q.commands.is_empty() {
                    let base_sha = task.base_sha.clone();
                    record_owner_base_check(
                        task,
                        &q.commands,
                        &base_sha,
                        &reply.text,
                        reply.default_evidence.is_some(),
                    );
                    let settings = self.settings.read().unwrap().clone();
                    let origin = origin_of_last_answer(task);
                    apply_base_check_answer(&settings, task, &q, &reply.text, origin);
                    if is_option(&reply.text, PRE_EXISTING_DROP) {
                        self.verify_cache.lock().unwrap().remove(&task.id);
                    }
                }
            }
            if task
                .question
                .as_ref()
                .is_some_and(|q| is_budget_raise(q, &reply.text))
            {
                task.raise_budget();
            }
            if task
                .question
                .as_ref()
                .is_some_and(|q| is_daily_budget_run_anyway(q, &reply.text))
            {
                task.allow_daily_budget(crate::costs::today(now_ms()));
            }
            task.clear_question();
            task.resume_phase(TaskStatus::Queued, crate::clock::now_ms());
        })?;
        self.start_task_loop(id.to_string());
        Ok(())
    }
    fn deliver_parked_answer(
        &self,
        task: &crate::task::Task,
        reply: &Reply,
    ) -> Result<AnswerDelivery, String> {
        // A tool call held for this question: the run goes on with the answer.
        if let Some(tx) = self.permission_waits.lock().unwrap().remove(&task.id) {
            if tx
                .send(Reply {
                    text: reply.text.clone(),
                    default_evidence: reply.default_evidence.clone(),
                    like: None,
                })
                .is_ok()
            {
                return Ok(AnswerDelivery::Permission);
            }
        }

        let (delivered, has_control) = {
            let controls = self.controls.lock().unwrap();
            let control = controls.get(&task.id);
            let delivered = control
                .and_then(|c| c.signals.pending_answer.lock().unwrap().take())
                .map(|tx| {
                    tx.send(Reply {
                        text: reply.text.clone(),
                        default_evidence: reply.default_evidence.clone(),
                        like: reply.like.clone(),
                    })
                    .is_ok()
                })
                .unwrap_or(false);
            (delivered, control.is_some())
        };
        // A default must not relaunch a loop whose question is being answered.
        if !delivered && reply.default_evidence.is_some() && has_control {
            self.agent_answers.lock().unwrap().remove(&task.id);
            return Err("the question was answered meanwhile".to_string());
        }
        Ok(if delivered {
            AnswerDelivery::Loop
        } else {
            AnswerDelivery::Idle
        })
    }
}

impl AnswerRequest {
    fn authority(&self) -> Result<(AnsweredBy, Option<String>), String> {
        let caller = match self.source.as_deref() {
            None | Some("") => AnsweredBy::Owner,
            Some("lead" | "mcp") => AnsweredBy::Lead,
            Some("overseer") => AnsweredBy::Overseer,
            Some("chat") => AnsweredBy::Orchestrator,
            Some("policy") => AnsweredBy::Policy,
            Some(source) => return Err(format!("unknown answer source: {source}")),
        };
        let default_evidence = match (self.source.as_deref(), &self.evidence) {
            (Some("policy"), Some(e)) => Some(e.clone()),
            (Some("policy"), None) => return Err("a policy answer needs its evidence".to_string()),
            _ => None,
        };
        Ok((caller, default_evidence))
    }
}

enum AnswerDelivery {
    Permission,
    Loop,
    Idle,
}
