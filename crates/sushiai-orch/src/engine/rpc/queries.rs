use crate::api::validation::validate_task_id;
use crate::clock::now_ms;
use crate::engine::state::App;
use crate::harness;
use crate::task::AttemptStatus;
use crate::task::Fingerprint;
use crate::task::Stage;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

/// The most lines `task.log` returns: the last ones.
pub(in crate::engine) const TASK_LOG_MAX_LINES: usize = 10_000;

impl App {
    pub(in crate::engine) async fn handle_task_list(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize, Default)]
        #[serde(rename_all = "camelCase")]
        struct P {
            repo: Option<String>,
            #[serde(default)]
            include_archived: bool,
        }
        let p: P = serde_json::from_value(params).unwrap_or_default();
        let mut tasks = self.store.list_tasks().map_err(|e| e.to_string())?;
        if let Some(repo) = p.repo {
            tasks.retain(|t| t.repo == repo);
        }
        if !p.include_archived {
            tasks.retain(|t| !t.archived);
        }
        serde_json::to_value(&tasks).map_err(|e| e.to_string())
    }
}

impl App {
    pub(in crate::engine) async fn handle_task_get(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let mut value = serde_json::to_value(&task).map_err(|e| e.to_string())?;
        // What each stage of this task cost, from the per-run cost records.
        let by_stage = crate::costs::summarize(
            &crate::store::costs::read_all(&self.data_dir),
            &crate::costs::Query {
                task_id: Some(task.id.clone()),
                group_by: vec!["stage".into()],
                ..Default::default()
            },
            now_ms(),
        );
        value["costByStage"] = by_stage["rows"].clone();
        Ok(value)
    }
}

impl App {
    /// One saved evidence image of a task as a data URL, for a renderer that
    /// cannot read the data directory. Only paths an attempt lists.
    pub(in crate::engine) async fn handle_task_evidence(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            path: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if !task.attempts.iter().any(|a| a.evidence.contains(&p.path)) {
            return Err("not an evidence image of this task".to_string());
        }
        let bytes = std::fs::read(&p.path).map_err(|e| e.to_string())?;
        let mime = crate::task::image_mime(Path::new(&p.path))
            .ok_or_else(|| "not an image".to_string())?;
        Ok(serde_json::json!({
            "dataUrl": format!("data:{mime};base64,{}", base64(&bytes)),
        }))
    }
}

impl App {
    /// `task.log {id, attempt, stage}` -> `{lines, truncated}`: the stored
    /// run of a finished attempt's `plan`, `implement`, `review` or `advisor`
    /// stage, replayed into the same lines the live `log` event carried.
    pub(in crate::engine) async fn handle_task_log(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        struct P {
            id: String,
            attempt: u32,
            stage: String,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        validate_task_id(&self.store, &p.id)?;
        let task = self
            .store
            .load_task(&p.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let wanted = if p.stage == "plan" {
            Stage::Plan
        } else {
            Stage::Implement
        };
        if !matches!(
            p.stage.as_str(),
            "plan" | "implement" | "review" | "advisor"
        ) {
            return Err(format!(
                "unknown stage {}: plan, implement, review or advisor",
                p.stage
            ));
        }
        let attempt = match task
            .attempts
            .iter()
            .find(|a| a.n == p.attempt && a.stage == wanted)
        {
            Some(a) => a,
            None if task.attempts.iter().any(|a| a.n == p.attempt) => {
                return Err(format!("attempt {} has no {} stage", p.attempt, p.stage));
            }
            None => return Err(format!("attempt {} not found", p.attempt)),
        };
        if attempt.status == AttemptStatus::Running {
            return Err("the attempt is still running".to_string());
        }
        let run_dir = self.store.run_dir(&p.id, p.attempt);
        let fingerprint_harness =
            |f: &Option<Fingerprint>| f.as_ref().map_or(attempt.harness, |f| f.harness);
        let (paths, harness) = match p.stage.as_str() {
            "plan" => (
                vec![run_dir.join("plan").join("events.jsonl")],
                attempt.harness,
            ),
            "implement" => (vec![run_dir.join("events.jsonl")], attempt.harness),
            "review" => (
                self.store
                    .helper_run_dirs(&p.id, "review", Some(p.attempt))
                    .into_iter()
                    .map(|dir| dir.join("events.jsonl"))
                    .collect(),
                fingerprint_harness(&attempt.review_fingerprint),
            ),
            _ => (
                self.store
                    .helper_run_dirs(&p.id, "advisor", Some(p.attempt))
                    .into_iter()
                    .map(|dir| dir.join("events.jsonl"))
                    .collect(),
                fingerprint_harness(&attempt.advisor_fingerprint),
            ),
        };
        let (lines, truncated) =
            tokio::task::spawn_blocking(move || replay_stage_logs(paths, harness))
                .await
                .map_err(|e| e.to_string())??;
        Ok(json!({"lines": lines, "truncated": truncated}))
    }
}

pub(in crate::engine) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn replay_stage_logs(
    paths: Vec<std::path::PathBuf>,
    harness: crate::policy::routing::Harness,
) -> Result<(Vec<String>, bool), String> {
    let mut lines = Vec::new();
    let mut found = false;
    for path in paths {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        found = true;
        let text = String::from_utf8_lossy(&bytes);
        let mut outcome = harness::RunOutcome::default();
        lines.extend(
            text.lines()
                .filter_map(|l| harness::feed_stream_line(harness, l, &mut outcome)),
        );
    }
    if !found {
        return Err("no stored log for this stage".to_string());
    }
    let truncated = lines.len() > TASK_LOG_MAX_LINES;
    if truncated {
        lines.drain(..lines.len() - TASK_LOG_MAX_LINES);
    }
    Ok::<_, String>((lines, truncated))
}
