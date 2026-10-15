//! `pipeline.describe`: the stages a task goes through, with the routes and
//! switches in force and, for one task, where each stage stands. Everything
//! is read from settings, the repo's preset and the task's recorded facts;
//! nothing here decides how a stage runs.

use crate::api::validation::validate_task_id;
use crate::clock::now_ms;
use crate::engine::state::App;
use crate::task::AskedBy;
mod routes;
mod stages;
mod state;

use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

/// Time and money of one stage, summed from the timeline segments.
#[derive(Default, Clone, Copy)]
struct Spent {
    ms: i64,
    usd: f64,
    seen: bool,
}

fn asked_by_stage(by: Option<AskedBy>) -> &'static str {
    match by {
        Some(AskedBy::Brief) => "brief",
        Some(AskedBy::Plan) => "plan",
        Some(AskedBy::Implement) => "implement",
        Some(AskedBy::Verify) => "checks",
        Some(AskedBy::Review) => "review",
        Some(AskedBy::Advisor) => "advisor",
        Some(AskedBy::Land) => "land",
        None => "question",
    }
}

impl App {
    pub(super) async fn handle_pipeline_describe(
        &self,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct P {
            #[serde(default)]
            repo: Option<String>,
            #[serde(default)]
            task_id: Option<String>,
        }
        let p: P = serde_json::from_value(params).map_err(|e| e.to_string())?;
        let settings = self.settings.read().unwrap().clone();
        let task = match p.task_id.as_deref().filter(|s| !s.is_empty()) {
            Some(id) => {
                validate_task_id(&self.store, id)?;
                Some(
                    self.store
                        .load_task(id)
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "task not found".to_string())?,
                )
            }
            None => None,
        };
        let repo = task
            .as_ref()
            .map(|t| t.repo.clone())
            .or(p.repo.filter(|r| !r.trim().is_empty()));

        let (variant, preset_id) = match &task {
            Some(t) => {
                let variant = t.variant();
                let matching = settings.all_presets().into_iter().find(|preset| {
                    let mut v = settings.experiments.clone();
                    preset.values.apply(&mut v);
                    v == variant
                });
                (variant, matching.map(|p| p.id))
            }
            None => {
                let mut variant = settings.experiments.clone();
                let preset = repo.as_deref().and_then(|r| settings.repo_preset(r));
                if let Some(preset) = &preset {
                    preset.values.apply(&mut variant);
                }
                (variant, preset.map(|p| p.id))
            }
        };

        let mut spent: HashMap<&'static str, Spent> = HashMap::new();
        if let Some(t) = &task {
            let segments =
                crate::timeline::task_timeline(&self.store, t, &settings.work_buckets, now_ms());
            for s in segments {
                let key = match s.stage {
                    "plan" => "plan",
                    "implement" => "implement",
                    "verify" | "final" => "checks",
                    "review" => "review",
                    "advisor" => "advisor",
                    _ => continue,
                };
                let e = spent.entry(key).or_default();
                e.seen = true;
                e.ms += (s.ended_at - s.started_at).max(0);
                e.usd += s.cost_usd;
            }
        }

        let stages = stages::describe(self, &settings, &variant, task.as_ref(), &spent);

        let branch = task.as_ref().and_then(|t| t.question.as_ref()).map(|q| {
            json!({
                "from": asked_by_stage(q.asked_by),
                "to": if q.held_for_overseer { "overseer" } else { "owner" },
            })
        });
        let mut out = json!({
            "presetId": preset_id,
            "presets": settings.all_presets(),
            "stages": stages,
        });
        if let Some(branch) = branch {
            out["branch"] = branch;
        }
        Ok(out)
    }
}
