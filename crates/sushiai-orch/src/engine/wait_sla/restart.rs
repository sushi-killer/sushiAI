use crate::engine::state::App;
use crate::policy::questions::owner_only;
use crate::task::Task;
use crate::task::TaskStatus;

impl App {
    /// After a restart: wakes the overseer once per repo that has a waiting
    /// question the owner is not the only one to answer.
    ///
    /// A question nobody will take after the restart is the owner's: an
    /// owner-only kind, or any with auto-answer off. A restart wake that
    /// cannot start hands its questions over itself (`crate::engine::chat::wake::wake_overseer`).
    pub(in crate::engine) fn wake_overseer_for_waiting_after_restart(&self) {
        let auto_answer = self.settings.read().unwrap().auto_answer;
        for task in self.store.list_tasks().unwrap_or_default() {
            if task.archived || task.status != TaskStatus::Waiting {
                continue;
            }
            if let Some(q) = task
                .question
                .as_ref()
                .filter(|q| !q.for_owner && (!auto_answer || owner_only(q.kind)))
            {
                self.hand_to_owner(&task.id, q.asked_at);
            }
        }
        if !auto_answer {
            return;
        }
        let mut repos: Vec<String> = self
            .store
            .list_tasks()
            .unwrap_or_else(|e| {
                eprintln!("orch: listing waiting tasks after restart failed: {e}");
                Vec::new()
            })
            .into_iter()
            .filter(|t| {
                !t.archived
                    && t.status == TaskStatus::Waiting
                    && t.question.as_ref().is_some_and(|q| !owner_only(q.kind))
            })
            .map(|t| t.repo.clone())
            .collect();
        repos.sort();
        repos.dedup();
        let tasks = self.store.list_tasks().unwrap_or_default();
        for repo in repos {
            let waiting: Vec<&Task> = tasks
                .iter()
                .filter(|t| t.repo == repo && t.status == TaskStatus::Waiting && !t.archived)
                .collect();
            crate::engine::chat::wake::wake_overseer(
                self,
                &repo,
                crate::engine::chat::wake::WakeRequest::new(
                    self.with_precedents(crate::prompts::get("overseer_restart"), &waiting),
                    false,
                    "restart",
                    None,
                ),
            );
        }
    }
}
