use crate::api::dispatch::UNKNOWN_METHOD;
use crate::api::validation::validate_task_id;
use crate::engine::chat_tools;
use crate::engine::messages;
use crate::engine::state::App;
impl App {
    /// Runs one `orch.<method>` call. An unknown method answers with
    /// [`UNKNOWN_METHOD`] as the start of its message.
    pub async fn dispatch(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match method {
            "settings.get" => self.handle_settings_get().await,
            "settings.set" => self.handle_settings_set(params).await,
            "settings.defaults" => self.handle_settings_defaults().await,
            "secrets.set" => self.handle_secrets_set(params).await,
            "task.list" => self.handle_task_list(params).await,
            "task.get" => self.handle_task_get(params).await,
            "task.create" => self.handle_task_create(params).await,
            "task.start" => self.handle_task_start(params).await,
            "task.backlog" => self.handle_task_backlog(params).await,
            "task.land" => self.handle_task_land(params).await,
            "task.pr" => self.handle_task_pr(params).await,
            "task.stop" => self.handle_task_stop(params).await,
            "task.park" => self.handle_task_park(params).await,
            "task.sync" => self.handle_task_sync(params).await,
            "task.answer" => self.handle_task_answer(params).await,
            "task.report" => self.handle_task_report(params).await,
            "task.leadTouch" => self.handle_task_lead_touch(params).await,
            "task.overturn" => self.handle_task_overturn(params).await,
            "task.amend" => self.handle_task_amend(params).await,
            "task.preset" => self.handle_task_preset(params).await,
            "pipeline.describe" => self.handle_pipeline_describe(params).await,
            "task.delete" => self.handle_task_delete(params).await,
            "costs.summary" => self.handle_costs_summary(params).await,
            "overseer.log" => self.handle_overseer_log(params).await,
            "worktrees.gc" => self.handle_worktrees_gc(params).await,
            "task.archive" => self.handle_task_archive(params).await,
            "task.unarchive" => self.handle_task_unarchive(params).await,
            "task.timeline" => {
                let id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");
                validate_task_id(&self.store, id)?;
                crate::timeline::timeline_json(&self.store, id)
            }
            "task.evidence" => self.handle_task_evidence(params).await,
            "task.openDeliverable" => self.handle_task_open_deliverable(params).await,
            "task.log" => self.handle_task_log(params).await,
            "failures.catalogue" => crate::timeline::catalogue_json(&self.store, &params),
            "chat.get" => crate::engine::chat::api::handle_get(self, params).await,
            "chat.send" => crate::engine::chat::api::handle_send(self, params).await,
            "chat.edit" => crate::engine::chat::api::handle_edit(self, params).await,
            "chat.cancel" => crate::engine::chat::api::handle_cancel(self, params).await,
            "chat.list" => crate::engine::chat::api::handle_list(self, params).await,
            "chat.new" => crate::engine::chat::api::handle_new(self, params).await,
            "chat.switch" => crate::engine::chat::api::handle_switch(self, params).await,
            "chat.clear" => crate::engine::chat::api::handle_clear(self, params).await,
            "chat.createProposal" => {
                crate::engine::chat::proposals::handle_create_proposal(self, params).await
            }
            "chat.tools" => chat_tools::status::handle_tools(self, params).await,
            "chat.toolServers" => chat_tools::status::handle_tool_servers(self, params).await,
            "chat.actionSend" => {
                crate::engine::chat::actions::handle_action_send(self, params).await
            }
            "chat.actionDecline" => {
                crate::engine::chat::actions::handle_action_decline(self, params).await
            }
            "peer.list" => messages::handle_peers(self, params).await,
            "message.send" => messages::handle_send(self, params).await,
            "message.inbox" => messages::handle_inbox(self, params).await,
            "message.list" => messages::handle_list(self, params).await,
            "repo.audit" => crate::engine::audit::run::handle_start(self, params).await,
            "repo.audit.get" => crate::engine::audit::run::handle_get(self, params).await,
            "repo.audit.list" => crate::engine::audit::run::handle_list(self, params).await,
            "evolution.run" => self.handle_evolution_run().await,
            "evolution.list" => self.handle_evolution_list(params).await,
            "evolution.approve" => self.handle_evolution_approve(params).await,
            "evolution.reject" => self.handle_evolution_reject(params).await,
            "evolution.adopt" => self.handle_evolution_adopt(params).await,
            "repo.notes.list" => self.handle_repo_notes_list(params).await,
            "repo.notes.add" => self.handle_repo_notes_add(params).await,
            "repo.notes.remove" => self.handle_repo_notes_remove(params).await,
            "precedents.list" => self.handle_precedents_list(params).await,
            "precedents.remove" => self.handle_precedents_remove(params).await,
            "precedents.rule.accept" => self.handle_precedents_rule_accept(params).await,
            "precedents.rule.dismiss" => self.handle_precedents_rule_dismiss(params).await,
            "hook.stop" => self.handle_hook_stop(params).await,
            "hook.edit" => self.handle_hook_edit(params).await,
            other => Err(format!("{UNKNOWN_METHOD}{other}")),
        }
    }
}
