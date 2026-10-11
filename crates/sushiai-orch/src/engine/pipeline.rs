//! `pipeline.describe`: the stages a task goes through, with the routes and
//! switches in force and, for one task, where each stage stands. Everything
//! is read from settings, the repo's preset and the task's recorded facts;
//! nothing here decides how a stage runs.

use super::*;
use serde::Serialize;

/// The stages in run order, then the side stages.
const MAIN: [(&str, &str); 10] = [
    ("brief", "Brief"),
    ("plan", "Plan"),
    ("brief-check", "Brief check"),
    ("base-checks", "Base checks"),
    ("implement", "Implement"),
    ("checks", "Checks"),
    ("held-out", "Held-out check"),
    ("review", "Review"),
    ("judge", "Judge"),
    ("land", "Land"),
];
const SIDE: [(&str, &str); 2] = [("advisor", "Advisor"), ("question", "Question")];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StageOut {
    id: &'static str,
    label: &'static str,
    kind: &'static str,
    enabled: bool,
    optional: bool,
    owner_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    route_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
}

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

/// A route id and the model of the route it names, when it names one.
fn route_of(settings: &Settings, id: &str) -> (Option<String>, Option<String>) {
    if id.is_empty() {
        return (None, None);
    }
    let model = settings
        .routes
        .iter()
        .find(|r| r.id == id)
        .and_then(|r| r.model.clone());
    (Some(id.to_string()), model)
}

/// Where one stage of `task` stands, from recorded facts only.
fn task_state(
    id: &str,
    enabled: bool,
    task: &Task,
    spent: &HashMap<&'static str, Spent>,
) -> &'static str {
    let seen = |s: &str| spent.get(s).is_some_and(|x| x.seen);
    let of = |stage: Stage| task.attempts.iter().filter(move |a| a.stage == stage);
    let last_verify = || {
        task.attempts
            .iter()
            .rev()
            .find(|a| a.verify.iter().any(|v| !brief::is_held_out(v)))
    };
    let stopped = matches!(task.status, TaskStatus::Failed | TaskStatus::Stopped);
    if !enabled && id != "brief" && id != "question" {
        return "skipped";
    }
    match id {
        "brief" => {
            if task.status == TaskStatus::Drafting {
                "pending"
            } else {
                "done"
            }
        }
        "plan" => {
            if of(Stage::Plan).any(|a| a.status == AttemptStatus::Running) {
                "running"
            } else if of(Stage::Plan).any(|a| a.status == AttemptStatus::Passed) {
                "done"
            } else if of(Stage::Plan).next().is_some() {
                if stopped {
                    "failed"
                } else {
                    "running"
                }
            } else if task.request.is_none() {
                "skipped"
            } else {
                "pending"
            }
        }
        "brief-check" => {
            if task.brief_check.done {
                "done"
            } else {
                "pending"
            }
        }
        "base-checks" => {
            if task.brief_check.feasibility_done {
                "done"
            } else {
                "pending"
            }
        }
        "implement" => {
            if of(Stage::Implement).any(|a| a.status == AttemptStatus::Running) {
                "running"
            } else if of(Stage::Implement).any(|a| a.status == AttemptStatus::Passed) {
                "done"
            } else if of(Stage::Implement).next().is_some() {
                if stopped {
                    "failed"
                } else {
                    "running"
                }
            } else {
                "pending"
            }
        }
        "checks" => match last_verify() {
            None => "pending",
            Some(a) if a.status == AttemptStatus::Running => "running",
            Some(a) => {
                let failed = a
                    .verify
                    .iter()
                    .any(|v| !brief::is_held_out(v) && v.code != Some(0));
                if failed {
                    "failed"
                } else {
                    "done"
                }
            }
        },
        "held-out" => {
            let held = task.attempts.iter().rev().find_map(|a| {
                a.verify
                    .iter()
                    .rev()
                    .find(|v| brief::is_held_out(v))
                    .map(|v| v.code == Some(0))
            });
            match held {
                Some(true) => "done",
                Some(false) => "failed",
                None => "pending",
            }
        }
        "review" => {
            let last = task.attempts.iter().rev().find_map(|a| a.review.as_ref());
            match last.map(|r| r.verdict) {
                Some(Verdict::Pass) => "done",
                Some(Verdict::Fail) => "failed",
                None if seen("review") => "running",
                None => "pending",
            }
        }
        "judge" => {
            if !task.brief_check.judged.is_empty() || !task.judged_findings.is_empty() {
                "done"
            } else {
                "pending"
            }
        }
        "land" => {
            if task.landed_sha.is_some() {
                "done"
            } else if task.status == TaskStatus::Landing {
                "running"
            } else {
                "pending"
            }
        }
        "advisor" => {
            if seen("advisor") {
                "done"
            } else {
                "pending"
            }
        }
        _ => {
            if task.question.is_some() {
                "running"
            } else if !task.question_history.is_empty() {
                "done"
            } else {
                "pending"
            }
        }
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

        let tier = task.as_ref().map_or(Tier::Standard, |t| t.tier);
        let (implement_id, _) = variant.implement_route_id(&settings, tier);
        let plan_id = variant.plan_route_id(&settings).to_string();
        let review_id = variant.review_route_id(&settings).to_string();
        let check_id = variant.brief_check_route_id(&settings).to_string();

        let mut stages = Vec::new();
        for (kind, list) in [("main", &MAIN[..]), ("side", &SIDE[..])] {
            for &(id, label) in list {
                let (enabled, optional, route) = match id {
                    "plan" => (!plan_id.is_empty(), false, plan_id.as_str()),
                    "brief-check" => (!check_id.is_empty(), true, check_id.as_str()),
                    "implement" => (true, false, implement_id.as_str()),
                    "held-out" => (variant.grounded_checks, true, ""),
                    "review" => (!review_id.is_empty(), false, review_id.as_str()),
                    // Judge keeps the global route, even when a preset or
                    // task variant overrides the brief check.
                    "judge" => (
                        !settings.brief_check_route.is_empty(),
                        false,
                        settings.brief_check_route.as_str(),
                    ),
                    "land" => (variant.land, true, ""),
                    "advisor" => (variant.advisor, true, plan_id.as_str()),
                    _ => (true, false, ""),
                };
                let (route_id, mut model) = route_of(&settings, route);
                if enabled && matches!(id, "brief-check" | "judge") {
                    // The route the engine resolves now: `auto` and a named
                    // route without its harness both fall to the cheapest.
                    let resolved = if id == "judge" {
                        super::brief_check::check_route_for(self, &Variant::default())
                    } else {
                        super::brief_check::check_route_for(self, &variant)
                    };
                    model = resolved
                        .and_then(|(_, r, _)| r)
                        .map(|r| r.model.unwrap_or(r.label));
                }
                if id == "review" && route == "auto" {
                    // `auto` picks at run time; show the route it would pick now.
                    let implementer = settings.routes.iter().find(|r| r.id == implement_id);
                    model = implementer
                        .and_then(|imp| {
                            super::review::select_review_route_with(&settings, route, imp, tier)
                        })
                        .and_then(|(r, _)| r.model.clone());
                }
                let mut out = StageOut {
                    id,
                    label,
                    kind,
                    enabled,
                    optional,
                    owner_only: matches!(id, "checks" | "base-checks"),
                    route_id,
                    model,
                    max_attempts: (id == "implement").then(|| variant.max_attempts(&settings)),
                    state: None,
                    duration_ms: None,
                    cost_usd: None,
                };
                if let Some(t) = &task {
                    out.state = Some(task_state(id, enabled, t, &spent));
                    if let Some(s) = spent.get(id).filter(|s| s.seen) {
                        out.duration_ms = Some(s.ms);
                        out.cost_usd = Some(s.usd);
                    }
                }
                stages.push(out);
            }
        }

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
