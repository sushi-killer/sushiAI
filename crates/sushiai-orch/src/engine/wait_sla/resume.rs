use crate::engine::state::App;
use crate::task::QuestionKind;
use crate::task::TaskStatus;
use serde_json::json;

impl App {
    /// A task waiting for a harness or for the daily budget goes on once the
    /// wait has cleared itself: a harness was installed, or the UTC day
    /// rolled over (or the budget was raised) so today's spend is under it.
    /// The answer only asks the loop to look again; it allows nothing.
    pub(in crate::engine) async fn resume_cleared_waits(&self, now: i64) {
        let waiting: Vec<(String, QuestionKind)> = self
            .store
            .list_tasks()
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.status == TaskStatus::Waiting && !t.archived)
            .filter_map(|t| Some((t.id.clone(), t.question.as_ref()?.kind)))
            .filter(|(id, kind)| {
                *kind != QuestionKind::HarnessMissing
                    || !self.harness_resumed.lock().unwrap().contains(id)
            })
            .filter(|(_, k)| matches!(k, QuestionKind::HarnessMissing | QuestionKind::DailyBudget))
            .collect();
        if waiting.is_empty() {
            return;
        }
        let harness_back = waiting
            .iter()
            .any(|(_, k)| *k == QuestionKind::HarnessMissing)
            && {
                self.refresh_availability();
                self.any_harness_available()
            };
        let budget_clear = waiting.iter().any(|(_, k)| *k == QuestionKind::DailyBudget) && {
            let budget = self.settings.read().unwrap().daily_budget_usd;
            let data = self.data_dir.clone();
            let records = tokio::task::spawn_blocking(move || crate::store::costs::read_all(&data))
                .await
                .unwrap_or_default();
            budget.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
                || crate::costs::spend_today(&records, now).1 < budget
        };
        for (id, kind) in waiting {
            let (clear, evidence) = match kind {
                QuestionKind::HarnessMissing => (harness_back, "an agent CLI is installed now"),
                _ => (budget_clear, "today's spend is under the daily budget"),
            };
            if clear {
                if kind == QuestionKind::HarnessMissing {
                    self.harness_resumed.lock().unwrap().insert(id.clone());
                }
                let params = json!({
                    "id": id,
                    "answer": "retry",
                    "source": "policy",
                    "evidence": evidence,
                });
                if let Err(e) = self.handle_task_answer(params).await {
                    eprintln!("orch: resuming task {id} failed: {e}");
                }
            }
        }
    }
}
