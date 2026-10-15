use crate::api::validation::validate_task_id;
use crate::clock::now_ms;
use crate::engine::answer_policy::check_authority;
use crate::engine::answer_policy::shared_check_amend_refusal;
use crate::engine::decision::implement_attempt_count;
use crate::engine::graph::dependencies::DEPENDENCY_QUESTION;
use crate::engine::state::App;
use crate::engine::verification::base::checks::baseline_on_base;
use crate::harness::cancel::CancelToken;
use crate::task::amendment::Amendment;
use crate::task::amendment::PendingAmendment;
use crate::task::checks::authority::Origin;
use crate::task::TaskStatus;
use serde_json::json;

impl App {
    pub(in crate::engine) async fn handle_task_amend(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let super::amend_request::AmendRequest {
            id,
            mut amendment,
            origin,
        } = super::amend_request::AmendRequest::parse(params)?;
        validate_task_id(&self.store, &id)?;
        let mut task = self
            .store
            .load_task(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        self.check_amend_authority(&task, &amendment, origin)?;
        crate::task::amendment::ensure_amendable(&task)?;
        self.resolve_amend_dependencies(&task, &mut amendment)?;
        amendment.validate_indices(&task)?;
        let fields = amendment.fields();
        let dependencies_amended = amendment.depends_on.is_some();
        if amendment.verify.is_some() {
            self.verify_cache.lock().unwrap().remove(&id);
        }
        if self.queue_amendment(&mut task, &amendment, origin)? {
            return Ok(json!({"id": id, "amended": fields, "pending": true}));
        }
        let (updated, pending) = self.apply_idle_amendment(task, amendment, origin).await?;
        if pending {
            return Ok(json!({"id": id, "amended": fields, "pending": true}));
        }
        if dependencies_amended {
            if updated.status == TaskStatus::Queued && updated.queue.backlog.is_none() {
                self.start_task_loop(id.clone());
            }
            self.advance_graph(&updated.repo);
        }
        Ok(json!({"id": id, "amended": fields, "pending": false}))
    }
    fn check_amend_authority(
        &self,
        task: &crate::task::Task,
        amendment: &Amendment,
        origin: Origin,
    ) -> Result<(), String> {
        let id = task.id.clone();
        if origin == Origin::Other {
            let mut effective = task.clone();
            if let Some(ctrl) = self.controls.lock().unwrap().get(&id) {
                if let Some(pending) = ctrl.signals.pending_amend.lock().unwrap().as_ref() {
                    let settings = self.settings.read().unwrap().clone();
                    effective =
                        pending.preview(task, &check_authority(&settings, task), now_ms())?;
                }
            }
            let settings = self.settings.read().unwrap();
            if let Some(why) = shared_check_amend_refusal(&settings, &effective, amendment) {
                return Err(why);
            }
        }
        Ok(())
    }
    fn resolve_amend_dependencies(
        &self,
        task: &crate::task::Task,
        amendment: &mut Amendment,
    ) -> Result<(), String> {
        if let Some(deps) = &amendment.depends_on {
            {
                let controls = self.controls.lock().unwrap();
                self.ensure_dependency_amend_idle(task, &controls)?;
            }
            let (deps, _) = self.check_graph_params(&task.repo, deps, None, Some(task))?;
            amendment.dependency_titles = deps.iter().map(|t| t.title.clone()).collect();
            amendment.depends_on = Some(deps.into_iter().map(|t| t.id.clone()).collect());
        }
        Ok(())
    }
    fn ensure_dependency_amend_idle(
        &self,
        task: &crate::task::Task,
        controls: &std::collections::HashMap<String, crate::engine::run_control::TaskControl>,
    ) -> Result<(), String> {
        let live = |id: &str| {
            controls
                .get(id)
                .is_some_and(|c| !c.signals.cancel.is_cancelled())
        };
        if live(&task.id) && implement_attempt_count(task) > 0 {
            return Err("task is implementing".into());
        }
        if self.repo_tasks(&task.repo).iter().any(|child| {
            child.parent.as_deref() == Some(task.id.as_str()) && !child.archived && live(&child.id)
        }) {
            return Err("a child is implementing".into());
        }
        Ok(())
    }
    fn queue_amendment(
        &self,
        task: &mut crate::task::Task,
        amendment: &Amendment,
        origin: Origin,
    ) -> Result<bool, String> {
        let id = task.id.clone();
        let lock = self.task_lock(&id);
        let _guard = lock.lock().unwrap();
        *task = self
            .store
            .load_task(&id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        crate::task::amendment::ensure_amendable(task)?;
        self.queue_live_amendment_locked(task, amendment, origin)
    }
    fn queue_live_amendment_locked(
        &self,
        task: &crate::task::Task,
        amendment: &Amendment,
        origin: Origin,
    ) -> Result<bool, String> {
        let id = task.id.clone();
        let settings = self.settings.read().unwrap().clone();
        let controls = self.controls.lock().unwrap();
        if amendment.depends_on.is_some() {
            self.ensure_dependency_amend_idle(task, &controls)?;
        }
        if let Some(ctrl) = controls
            .get(&id)
            .filter(|c| !c.signals.cancel.is_cancelled())
        {
            let mut slot = ctrl.signals.pending_amend.lock().unwrap();
            let effective = match slot.as_ref() {
                Some(pending) => {
                    pending.preview(task, &check_authority(&settings, task), now_ms())?
                }
                None => task.clone(),
            };
            amendment.validate_indices(&effective)?;
            if origin == Origin::Other {
                if let Some(why) = shared_check_amend_refusal(&settings, &effective, amendment) {
                    return Err(why);
                }
            }
            match slot.as_mut() {
                Some(pending) => pending.merge(amendment.clone(), origin),
                None => *slot = Some(PendingAmendment::capture(task, amendment.clone(), origin)),
            }
            return Ok(true);
        }

        Ok(false)
    }
    async fn apply_idle_amendment(
        &self,
        mut task: crate::task::Task,
        amendment: Amendment,
        origin: Origin,
    ) -> Result<(crate::task::Task, bool), String> {
        let id = task.id.clone();
        let before = crate::task::checks::snapshot::CheckSnapshot::capture(&task);
        let dependencies_amended = amendment.depends_on.is_some();
        let has_dependencies = amendment.depends_on.as_ref().is_some_and(|d| !d.is_empty());
        let settings = self.settings.read().unwrap().clone();
        let authority = check_authority(&settings, &task);
        amendment
            .clone()
            .apply_authorized(&mut task, origin, &authority, now_ms())?;
        baseline_on_base(&self.arc(), &mut task, &CancelToken::new()).await;
        let mut pending = false;
        let updated = self.update_task_checked(&id, |fresh| {
            crate::task::amendment::ensure_amendable(fresh)?;
            if !before.matches(fresh) {
                return Err("the amendment is stale".to_string());
            }
            let mut amendment = amendment;
            self.resolve_amend_dependencies(fresh, &mut amendment)?;
            if self.queue_live_amendment_locked(fresh, &amendment, origin)? {
                pending = true;
                return Ok(());
            }
            if origin == Origin::Other {
                let settings = self.settings.read().unwrap().clone();
                if let Some(why) = shared_check_amend_refusal(&settings, fresh, &amendment) {
                    return Err(why);
                }
            }
            let authority = check_authority(&self.settings.read().unwrap(), fresh);
            amendment.apply_authorized(fresh, origin, &authority, now_ms())?;
            if fresh.status == TaskStatus::Failed && has_dependencies {
                fresh.resume_phase(TaskStatus::Queued, crate::clock::now_ms());
            }
            if dependencies_amended
                && fresh.status == TaskStatus::Waiting
                && fresh
                    .question
                    .as_ref()
                    .is_some_and(|q| q.text.starts_with(DEPENDENCY_QUESTION))
            {
                fresh.clear_question();
                fresh.resume_phase(TaskStatus::Queued, crate::clock::now_ms());
            }
            crate::task::checks::baseline::adopt(fresh, &task);
            for decision in &task.decisions {
                if !fresh.decisions.contains(decision) {
                    fresh.record_decision(decision.clone());
                }
            }
            Ok(())
        })?;
        Ok((updated, pending))
    }
}
