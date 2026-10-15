//! Append-only per-run spend ledger; partial/corrupt JSON lines are skipped.

use crate::policy::routing::Harness;
use crate::task::audit::Audit;
use crate::task::evolution::Proposal;
use crate::task::{Attempt, Stage, Task};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Every stage a record can name. `brief_check` and `eval_check` are for
/// runs that use a model; today those checks run shell commands only.
pub const STAGES: [&str; 18] = [
    "plan",
    "implement",
    "review",
    "advisor",
    "brief_check",
    "refresh",
    "pick",
    "final",
    "eval_check",
    "triage",
    "answer_judge",
    "finding_judge",
    "pr_draft",
    "audit",
    "evolution",
    "attribution",
    "chat",
    "other",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostRecord {
    pub ts: i64,
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The audit or evolution proposal a non-task run belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    pub route_id: String,
    pub harness: Harness,
    pub model: String,
    pub cost_usd: f64,
    #[serde(default)]
    pub estimated: bool,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub ms: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub backfilled: bool,
}

pub fn costs_path(data: &Path) -> PathBuf {
    data.join("costs.jsonl")
}

/// Appends one line. Best effort: a run's cost record must never fail the
/// run.
pub fn append(data: &Path, record: &CostRecord) {
    let mut record = record.clone();
    if !STAGES.contains(&record.stage.as_str()) {
        record.stage = "other".into();
    }
    let mut line = match serde_json::to_string(&record) {
        Ok(line) => line,
        Err(e) => {
            eprintln!("orch: serializing cost record failed: {e}");
            return;
        }
    };
    line.push('\n');
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(costs_path(data))
        .and_then(|mut file| file.write_all(line.as_bytes()));
    if let Err(e) = result {
        eprintln!("orch: appending cost record failed: {e}");
    }
}

/// Every record; a corrupt line is skipped.
pub fn read_all(data: &Path) -> Vec<CostRecord> {
    std::fs::read_to_string(costs_path(data))
        .unwrap_or_else(|e| {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("orch: reading cost records failed: {e}");
            }
            String::new()
        })
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Every stored task, for the lead-touch rate.
pub fn read_tasks(data: &Path) -> Vec<Task> {
    read_dir_json(&data.join("tasks"), Some("task.json"))
}

fn model_of(models: Option<&crate::task::Fingerprint>, fallback: &str) -> String {
    models
        .and_then(|f| f.models.first().cloned())
        .unwrap_or_else(|| fallback.to_string())
}

fn attempt_records(task: &Task) -> Vec<CostRecord> {
    let base = |a: &Attempt, stage: &str, cost: f64| CostRecord {
        ts: a.ended_at.unwrap_or(a.started_at),
        repo: task.repo.clone(),
        task_id: Some(task.id.clone()),
        run_id: None,
        stage: stage.into(),
        attempt: Some(a.n),
        route_id: a.route_id.clone(),
        harness: a.harness,
        model: a.model.clone(),
        cost_usd: cost,
        estimated: false,
        input_tokens: 0,
        cached_tokens: 0,
        output_tokens: 0,
        ms: a.ended_at.map_or(0, |e| (e - a.started_at).max(0) as u64),
        backfilled: true,
    };
    let mut out = Vec::new();
    for a in &task.attempts {
        if let Some(cost) = a.cost_usd {
            let stage = match a.stage {
                Stage::Plan => "plan",
                Stage::Implement => "implement",
                Stage::Review => "review",
            };
            let mut r = base(a, stage, cost);
            r.model = model_of(a.fingerprint.as_ref(), &a.model);
            r.estimated = a.cost_estimated;
            if let Some(u) = &a.usage {
                r.input_tokens = u.input;
                r.cached_tokens = u.cached;
                r.output_tokens = u.output;
            }
            out.push(r);
        }
        for (stage, cost, fp) in [
            ("review", a.review_cost_usd, &a.review_fingerprint),
            ("advisor", a.advisor_cost_usd, &a.advisor_fingerprint),
        ] {
            let Some(cost) = cost else { continue };
            let mut r = base(a, stage, cost);
            r.route_id = "unknown".into();
            r.ms = 0;
            if let Some(fp) = fp {
                r.harness = fp.harness;
                r.model = model_of(Some(fp), &a.model);
            }
            out.push(r);
        }
    }
    out
}

fn read_dir_json<T: serde::de::DeserializeOwned>(dir: &Path, file: Option<&str>) -> Vec<T> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "orch: listing {} for cost backfill failed: {e}",
                    dir.display()
                );
            }
            return Vec::new();
        }
    };
    entries
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(e) => {
                eprintln!(
                    "orch: reading entry in {} for cost backfill failed: {e}",
                    dir.display()
                );
                None
            }
        })
        .filter_map(|e| {
            let path = match file {
                Some(f) => e.path().join(f),
                None => e.path(),
            };
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(e) => {
                    eprintln!(
                        "orch: reading {} for cost backfill failed: {e}",
                        path.display()
                    );
                    return None;
                }
            };
            serde_json::from_str(&text).ok()
        })
        .collect()
}

/// Derives records from `task.json`, `audit.json` and proposal files, once:
/// a task, audit or proposal that already has a record is skipped. Returns
/// how many records it appended.
pub fn backfill(data: &Path) -> usize {
    let existing = read_all(data);
    let tasks_done: HashSet<&str> = existing
        .iter()
        .filter_map(|r| r.task_id.as_deref())
        .collect();
    let runs_done: HashSet<&str> = existing
        .iter()
        .filter_map(|r| r.run_id.as_deref())
        .collect();
    let mut fresh: Vec<CostRecord> = Vec::new();

    let tasks: Vec<Task> = read_dir_json(&data.join("tasks"), Some("task.json"));
    for task in tasks.iter().filter(|t| !tasks_done.contains(t.id.as_str())) {
        fresh.extend(attempt_records(task));
    }
    let audits: Vec<Audit> = read_dir_json(&data.join("audits"), Some("audit.json"));
    for a in audits
        .iter()
        .filter(|a| a.cost_usd > 0.0 && !runs_done.contains(a.id.as_str()))
    {
        fresh.push(CostRecord {
            ts: a.ended_at.unwrap_or(a.started_at),
            repo: a.repo.clone(),
            task_id: None,
            run_id: Some(a.id.clone()),
            stage: "audit".into(),
            attempt: None,
            route_id: a.route_id.clone(),
            harness: a.harness,
            model: model_of(a.fingerprint.as_ref(), &a.model),
            cost_usd: a.cost_usd,
            estimated: false,
            input_tokens: a.usage.as_ref().map_or(0, |u| u.input),
            cached_tokens: a.usage.as_ref().map_or(0, |u| u.cached),
            output_tokens: a.usage.as_ref().map_or(0, |u| u.output),
            ms: a.ended_at.map_or(0, |e| (e - a.started_at).max(0) as u64),
            backfilled: true,
        });
    }
    let proposals: Vec<Proposal> = read_dir_json(&data.join("evolution").join("proposals"), None);
    for p in proposals
        .iter()
        .filter(|p| p.cost_usd > 0.0 && !runs_done.contains(p.id.as_str()))
    {
        let fp = p.fingerprint.as_ref();
        fresh.push(CostRecord {
            ts: p.created_at,
            repo: p.repo.clone(),
            task_id: None,
            run_id: Some(p.id.clone()),
            stage: "evolution".into(),
            attempt: None,
            route_id: "unknown".into(),
            harness: fp.map_or(Harness::Claude, |f| f.harness),
            model: model_of(fp, "unknown"),
            cost_usd: p.cost_usd,
            estimated: false,
            input_tokens: 0,
            cached_tokens: 0,
            output_tokens: 0,
            ms: 0,
            backfilled: true,
        });
    }
    for r in &fresh {
        append(data, r);
    }
    fresh.len()
}
