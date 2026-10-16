use super::create_request::CreateRequest;
use crate::engine::state::App;

impl App {
    pub(in crate::engine) async fn handle_task_create(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let request: CreateRequest = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let task = self
            .create_task_record(request.prepare(self).await?)
            .await?;
        let task = request.activate(self, task)?;
        serde_json::to_value(task).map_err(|e| e.to_string())
    }
}
