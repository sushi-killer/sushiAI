use super::evidence::read_signals;
use super::measurement::measure;
use crate::api::events::Event;
use crate::clock::now_ms;
use crate::engine::state::App;
use crate::task::evolution::{Proposal, ProposalForm, ProposalStatus, Track};
use serde_json::json;

fn proposal_id_param(app: &App, params: &serde_json::Value) -> Result<String, String> {
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("id is required")?;
    uuid::Uuid::parse_str(id).map_err(|_| "invalid proposal id".to_string())?;
    if !app
        .store
        .proposal_path(id)
        .starts_with(app.store.proposals_dir())
    {
        return Err("invalid proposal id".to_string());
    }
    Ok(id.to_string())
}

impl App {
    pub(super) fn broadcast_proposal(&self, p: &Proposal) {
        let _ = self.events_tx.send(Event::Proposal {
            proposal: Box::new(serde_json::to_value(p).unwrap_or_else(|_| json!({}))),
        });
    }

    pub(super) fn store_proposal(&self, p: &Proposal) -> Result<(), String> {
        self.store.save_proposal(p).map_err(|e| e.to_string())?;
        self.broadcast_proposal(p);
        Ok(())
    }

    /// Adoption and measurement of every stored proposal; the ones that
    /// changed.
    pub(super) async fn measure_proposals(&self) -> Vec<Proposal> {
        let _guard = self.proposal_lock.lock().await;
        let settings = self.settings.read().unwrap().evolution.clone();
        let proposals = self.store.list_proposals().unwrap_or_default();
        if proposals.is_empty() {
            return Vec::new();
        }
        let tasks = self.store.list_tasks().unwrap_or_default();
        let signals = read_signals(&self.data_dir);
        let mut changed = Vec::new();
        for mut p in proposals {
            if measure(&mut p, &tasks, &signals, &settings) && self.store_proposal(&p).is_ok() {
                changed.push(p);
            }
        }
        changed
    }

    /// `evolution.list {repo?}`: every proposal, newest first.
    pub(crate) async fn handle_evolution_list(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let repo = params.get("repo").and_then(|v| v.as_str());
        let list = self.store.list_proposals().map_err(|e| e.to_string())?;
        Ok(serde_json::Value::Array(
            list.iter()
                .filter(|p| repo.is_none_or(|r| p.repo == r))
                .map(|p| serde_json::to_value(p).unwrap_or_default())
                .collect(),
        ))
    }

    fn load_proposal_param(&self, params: &serde_json::Value) -> Result<Proposal, String> {
        let id = proposal_id_param(self, params)?;
        self.store
            .load_proposal(&id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no such proposal".to_string())
    }

    /// `evolution.approve {id}`: a repo-track proposal becomes an ordinary
    /// task, started at once.
    pub(crate) async fn handle_evolution_approve(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let _guard = self.proposal_lock.lock().await;
        let mut p = self.load_proposal_param(&params)?;
        if p.track != Track::Repo {
            return Err("only a repo-track proposal can be approved; adopt a harness-track one after its eval".to_string());
        }
        if p.status != ProposalStatus::Proposed {
            return Err(format!("proposal is already {:?}", p.status).to_lowercase());
        }
        let request = format!(
            "Evolution proposal: {}\n\nEvidence: {}\nMetric: {}\nTest: {}",
            p.change, p.evidence, p.metric, p.test
        );
        let task = self
            .handle_task_create(json!({"repo": p.repo, "request": request, "start": true}))
            .await?;
        p.task_id = task.get("id").and_then(|v| v.as_str()).map(String::from);
        if matches!(p.form, ProposalForm::Doc | ProposalForm::Command) {
            self.add_repo_note(&p.repo, &p.change, format!("proposal:{}", p.id))
                .await?;
        }
        p.status = ProposalStatus::Approved;
        self.store_proposal(&p)?;
        serde_json::to_value(&p).map_err(|e| e.to_string())
    }

    /// `evolution.reject {id, reason?}`.
    pub(crate) async fn handle_evolution_reject(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let _guard = self.proposal_lock.lock().await;
        let mut p = self.load_proposal_param(&params)?;
        p.status = ProposalStatus::Rejected;
        p.reason = params
            .get("reason")
            .and_then(|v| v.as_str())
            .filter(|r| !r.trim().is_empty())
            .map(String::from)
            .or_else(|| Some("rejected by the owner".to_string()));
        self.store_proposal(&p)?;
        serde_json::to_value(&p).map_err(|e| e.to_string())
    }

    /// `evolution.adopt {id}`: the change is in; measurement starts now.
    pub(crate) async fn handle_evolution_adopt(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let _guard = self.proposal_lock.lock().await;
        let mut p = self.load_proposal_param(&params)?;
        if p.status == ProposalStatus::Rejected {
            return Err("a rejected proposal cannot be adopted".to_string());
        }
        p.status = ProposalStatus::Adopted;
        p.adopted_at = Some(now_ms());
        p.reason = None;
        self.store_proposal(&p)?;
        serde_json::to_value(&p).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;
