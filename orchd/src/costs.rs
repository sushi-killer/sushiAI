//! Spend analytics: one record per model run in `<data>/costs.jsonl`
//! (append-only, one JSON line each), the `costs.summary` grouping over
//! them, the one-time `orchd costs backfill`, and the `orchd costs` CLI.
//! Every place orchd runs a harness writes its record when the run ends.

use crate::model::{now_ms, Attempt, Audit, Harness, Proposal, Stage, Task, TaskStatus};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Every stage a record can name. `brief_check` and `eval_check` are for
/// runs that use a model; today those checks run shell commands only.
pub const STAGES: [&str; 13] = [
    "plan",
    "implement",
    "review",
    "advisor",
    "brief_check",
    "pick",
    "final",
    "eval_check",
    "triage",
    "answer_judge",
    "audit",
    "evolution",
    "chat",
];

const DAY_MS: i64 = 86_400_000;

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
    debug_assert!(STAGES.contains(&record.stage.as_str()), "{}", record.stage);
    let Ok(mut line) = serde_json::to_string(record) else {
        return;
    };
    line.push('\n');
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(costs_path(data))
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Every record; a corrupt line is skipped.
pub fn read_all(data: &Path) -> Vec<CostRecord> {
    std::fs::read_to_string(costs_path(data))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

// -- summary -----------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Query {
    pub repo: Option<String>,
    pub task_id: Option<String>,
    pub since_days: Option<u32>,
    pub group_by: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct Acc {
    cost: f64,
    runs: u64,
    input: u64,
    cached: u64,
    output: u64,
}

impl Acc {
    fn add(&mut self, r: &CostRecord) {
        self.cost += r.cost_usd;
        self.runs += 1;
        self.input += r.input_tokens;
        self.cached += r.cached_tokens;
        self.output += r.output_tokens;
    }

    fn json(&self, extra: Value) -> Value {
        let seen = self.input + self.cached;
        let rate = if seen == 0 {
            0.0
        } else {
            (self.cached as f64 / seen as f64).min(1.0)
        };
        let mut v = json!({
            "costUsd": self.cost,
            "runs": self.runs,
            "tokens": {"input": self.input, "cached": self.cached, "output": self.output},
            "cacheHitRate": rate,
        });
        if let (Some(obj), Some(more)) = (v.as_object_mut(), extra.as_object()) {
            obj.extend(more.clone());
        }
        v
    }
}

fn day_of(ts: i64) -> String {
    // Days since the epoch -> civil date (proleptic Gregorian, UTC).
    let z = ts.div_euclid(DAY_MS) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn key_part(r: &CostRecord, by: &str) -> String {
    match by {
        "stage" => r.stage.clone(),
        "model" => r.model.clone(),
        "route" => r.route_id.clone(),
        "repo" => r.repo.clone(),
        "task" => r.task_id.clone().unwrap_or_else(|| "-".into()),
        "day" => day_of(r.ts),
        _ => String::new(),
    }
}

pub fn check_group_by(group_by: &[String]) -> Result<(), String> {
    for g in group_by {
        if !["stage", "model", "route", "repo", "task", "day"].contains(&g.as_str()) {
            return Err(format!(
                "unknown groupBy {g:?} (stage, model, route, repo, task, day)"
            ));
        }
    }
    Ok(())
}

/// `{rows: [{key, keys, costUsd, runs, tokens, cacheHitRate}], totals}`,
/// rows by cost, highest first (by key for `day`, oldest first).
pub fn summarize(records: &[CostRecord], q: &Query, now: i64) -> Value {
    let since = q.since_days.map(|d| now - i64::from(d) * DAY_MS);
    let mut total = Acc::default();
    let mut groups: BTreeMap<Vec<String>, Acc> = BTreeMap::new();
    for r in records {
        if q.repo.as_ref().is_some_and(|repo| *repo != r.repo)
            || q.task_id.is_some() && q.task_id != r.task_id
            || since.is_some_and(|s| r.ts < s)
        {
            continue;
        }
        total.add(r);
        let key: Vec<String> = q.group_by.iter().map(|g| key_part(r, g)).collect();
        groups.entry(key).or_default().add(r);
    }
    let mut rows: Vec<(Vec<String>, Acc)> = groups.into_iter().collect();
    if q.group_by.first().map(String::as_str) != Some("day") {
        rows.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost));
    }
    let rows: Vec<Value> = rows
        .iter()
        .map(|(keys, acc)| acc.json(json!({"key": keys.join(" / "), "keys": keys})))
        .collect();
    json!({"rows": rows, "totals": total.json(json!({}))})
}

/// Monday (UTC) of the week `ts` falls in, as `YYYY-MM-DD`.
fn week_of(ts: i64) -> String {
    // 1970-01-01 was a Thursday: three days after that week's Monday.
    let days = ts.div_euclid(DAY_MS);
    let monday = days - (days + 3).rem_euclid(7);
    day_of(monday * DAY_MS)
}

#[derive(Default, Clone, Copy)]
struct Touch {
    touched: u64,
    marked: u64,
}

impl Touch {
    fn json(&self, extra: Value) -> Value {
        let rate = if self.marked == 0 {
            0.0
        } else {
            self.touched as f64 / self.marked as f64
        };
        let mut v = json!({"touched": self.touched, "marked": self.marked, "rate": rate});
        if let (Some(obj), Some(more)) = (v.as_object_mut(), extra.as_object()) {
            obj.extend(more.clone());
        }
        v
    }
}

/// The lead-touch rate: done tasks whose work needed a fix after orchd said
/// done, over done tasks carrying a mark, overall, per repo and per week
/// (the week the task finished). Honors the query's repo, task and window.
pub fn lead_touch_summary(tasks: &[Task], q: &Query, now: i64) -> Value {
    let since = q.since_days.map(|d| now - i64::from(d) * DAY_MS);
    let mut total = Touch::default();
    let mut repos: BTreeMap<&str, Touch> = BTreeMap::new();
    let mut weeks: BTreeMap<String, Touch> = BTreeMap::new();
    for t in tasks {
        let Some(mark) = t
            .lead_touch
            .as_ref()
            .filter(|_| t.status == TaskStatus::Done)
        else {
            continue;
        };
        if q.repo.as_ref().is_some_and(|r| *r != t.repo)
            || q.task_id.as_ref().is_some_and(|id| *id != t.id)
            || since.is_some_and(|s| t.updated_at < s)
        {
            continue;
        }
        for acc in [
            &mut total,
            repos.entry(t.repo.as_str()).or_default(),
            weeks.entry(week_of(t.updated_at)).or_default(),
        ] {
            acc.marked += 1;
            acc.touched += u64::from(mark.touched);
        }
    }
    let mut out = total.json(json!({}));
    out["byRepo"] = repos
        .iter()
        .map(|(repo, t)| t.json(json!({"repo": repo})))
        .collect();
    out["byWeek"] = weeks
        .iter()
        .map(|(week, t)| t.json(json!({"week": week})))
        .collect();
    out
}

/// Every stored task, for the lead-touch rate.
pub fn read_tasks(data: &Path) -> Vec<Task> {
    read_dir_json(&data.join("tasks"), Some("task.json"))
}

/// The text `orchd costs` prints for a summary.
pub fn render(summary: &Value) -> String {
    let money = |v: &Value| format!("${:.2}", v["costUsd"].as_f64().unwrap_or(0.0));
    let mut out = String::new();
    let rows = summary["rows"].as_array().cloned().unwrap_or_default();
    let width = rows
        .iter()
        .map(|r| r["key"].as_str().unwrap_or("").chars().count())
        .max()
        .unwrap_or(0)
        .max("total".len());
    let line = |key: &str, v: &Value| {
        let t = &v["tokens"];
        format!(
            "{key:<width$}  {:>9}  {:>5} runs  in {} / cached {} / out {}  cache {:.0}%\n",
            money(v),
            v["runs"].as_u64().unwrap_or(0),
            t["input"].as_u64().unwrap_or(0),
            t["cached"].as_u64().unwrap_or(0),
            t["output"].as_u64().unwrap_or(0),
            v["cacheHitRate"].as_f64().unwrap_or(0.0) * 100.0,
        )
    };
    for r in &rows {
        out.push_str(&line(r["key"].as_str().unwrap_or(""), r));
    }
    out.push_str(&line("total", &summary["totals"]));
    let touch = &summary["leadTouch"];
    if touch["marked"].as_u64().unwrap_or(0) > 0 {
        let rate = |v: &Value| {
            format!(
                "{}/{} touched ({:.0}%)",
                v["touched"].as_u64().unwrap_or(0),
                v["marked"].as_u64().unwrap_or(0),
                v["rate"].as_f64().unwrap_or(0.0) * 100.0
            )
        };
        out.push_str(&format!("\nlead touch  {}\n", rate(touch)));
        for r in touch["byRepo"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "  {}  {}\n",
                r["repo"].as_str().unwrap_or(""),
                rate(r)
            ));
        }
        for w in touch["byWeek"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "  week {}  {}\n",
                w["week"].as_str().unwrap_or(""),
                rate(w)
            ));
        }
    }
    out
}

// -- backfill ------------------------------------------------------------------

fn model_of(models: Option<&crate::model::Fingerprint>, fallback: &str) -> String {
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
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let path = match file {
                Some(f) => e.path().join(f),
                None => e.path(),
            };
            serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
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

// -- CLI -----------------------------------------------------------------------

/// `orchd costs backfill --data <dir>` and
/// `orchd costs --data <dir> [--repo <r>] [--since <days>] [--by stage,model] [--json]`.
pub fn run(args: &[String]) -> i32 {
    let backfill_mode = args.first().map(String::as_str) == Some("backfill");
    let (mut data, mut repo, mut since, mut by, mut as_json) =
        (None, None, None, vec!["stage".to_string()], false);
    let mut i = usize::from(backfill_mode);
    while i < args.len() {
        match args[i].as_str() {
            "--data" if i + 1 < args.len() => {
                data = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--repo" if i + 1 < args.len() => {
                repo = Some(args[i + 1].clone());
                i += 2;
            }
            "--since" if i + 1 < args.len() => {
                since = args[i + 1].parse::<u32>().ok();
                i += 2;
            }
            "--by" if i + 1 < args.len() => {
                by = args[i + 1]
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                i += 2;
            }
            "--json" => {
                as_json = true;
                i += 1;
            }
            _ => i += 1,
        }
    }
    let Some(data) = data else {
        eprintln!(
            "orchd costs: usage: orchd costs [backfill] --data <dir> [--repo <r>] [--since <days>] [--by stage,model] [--json]"
        );
        return 2;
    };
    if backfill_mode {
        println!("backfilled {} records", backfill(&data));
        return 0;
    }
    if let Err(e) = check_group_by(&by) {
        eprintln!("orchd costs: {e}");
        return 2;
    }
    let q = Query {
        repo,
        task_id: None,
        since_days: since,
        group_by: by,
    };
    let mut summary = summarize(&read_all(&data), &q, now_ms());
    summary["leadTouch"] = lead_touch_summary(&read_tasks(&data), &q, now_ms());
    if as_json {
        println!("{summary}");
    } else {
        print!("{}", render(&summary));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(stage: &str, model: &str, cost: f64, ts: i64) -> CostRecord {
        CostRecord {
            ts,
            repo: "/r".into(),
            task_id: Some("t1".into()),
            run_id: None,
            stage: stage.into(),
            attempt: Some(1),
            route_id: "claude-sonnet".into(),
            harness: Harness::Claude,
            model: model.into(),
            cost_usd: cost,
            estimated: false,
            input_tokens: 100,
            cached_tokens: 300,
            output_tokens: 10,
            ms: 5,
            backfilled: false,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orchd-costs-{name}-{}", now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn summary_groups_by_stage_and_model_with_totals() {
        let now = 40 * DAY_MS;
        let records = vec![
            rec("implement", "sonnet", 1.0, now - DAY_MS),
            rec("review", "opus", 2.0, now - DAY_MS),
            rec("review", "opus", 0.5, now - 2 * DAY_MS),
            rec("review", "opus", 9.0, now - 20 * DAY_MS),
        ];
        let by = |g: &[&str], since| Query {
            repo: None,
            task_id: None,
            since_days: since,
            group_by: g.iter().map(|s| s.to_string()).collect(),
        };
        let s = summarize(&records, &by(&["stage"], Some(7)), now);
        assert_eq!(s["rows"][0]["key"], "review");
        assert_eq!(s["rows"][0]["costUsd"], 2.5);
        assert_eq!(s["rows"][0]["runs"], 2);
        assert_eq!(s["rows"][1]["key"], "implement");
        assert_eq!(s["totals"]["costUsd"], 3.5);
        assert_eq!(s["totals"]["tokens"]["cached"], 900);
        assert_eq!(s["totals"]["cacheHitRate"], 0.75);
        let s = summarize(&records, &by(&["model"], None), now);
        assert_eq!(s["rows"][0]["key"], "opus");
        assert_eq!(s["rows"][0]["costUsd"], 11.5);
        assert_eq!(s["totals"]["runs"], 4);
        let s = summarize(&records, &by(&["stage", "model"], Some(7)), now);
        assert_eq!(s["rows"][0]["key"], "review / opus");
        let s = summarize(&records, &by(&["day"], Some(7)), now);
        assert_eq!(s["rows"][0]["key"], day_of(now - 2 * DAY_MS));
    }

    #[test]
    fn day_of_formats_utc_dates() {
        assert_eq!(day_of(0), "1970-01-01");
        assert_eq!(day_of(1_700_000_000_000), "2023-11-14");
    }

    #[test]
    fn append_then_read_round_trips_and_skips_bad_lines() {
        let dir = temp_dir("roundtrip");
        append(&dir, &rec("plan", "sonnet", 0.1, 5));
        std::fs::write(
            costs_path(&dir),
            format!(
                "{}not json\n",
                std::fs::read_to_string(costs_path(&dir)).unwrap()
            ),
        )
        .unwrap();
        append(&dir, &rec("review", "opus", 0.2, 6));
        let all = read_all(&dir);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].stage, "review");
    }

    fn done_task(id: &str, repo: &str, at: i64, touched: Option<bool>) -> Task {
        let mut t: Task = serde_json::from_value(json!({
            "id": id, "title": id, "goal": "", "criteria": [], "verify": [],
            "repo": repo, "worktree": "", "branch": "b", "baseSha": "s",
            "status": "done", "tier": "standard", "createdAt": at, "updatedAt": at,
        }))
        .unwrap();
        t.lead_touch = touched.map(|touched| crate::model::LeadTouch {
            touched,
            note: String::new(),
            at,
            by: "owner".into(),
        });
        t
    }

    #[test]
    fn lead_touch_rate_counts_marked_done_tasks_per_repo_and_week() {
        let now = 40 * DAY_MS;
        let mut running = done_task("r", "/a", now, Some(true));
        running.status = TaskStatus::Running;
        let tasks = vec![
            done_task("1", "/a", now - DAY_MS, Some(true)),
            done_task("2", "/a", now - DAY_MS, Some(false)),
            done_task("3", "/a", now - DAY_MS, None),
            done_task("4", "/b", now - 30 * DAY_MS, Some(true)),
            running,
        ];
        let q = Query::default();
        let s = lead_touch_summary(&tasks, &q, now);
        assert_eq!(
            (s["touched"].as_u64(), s["marked"].as_u64()),
            (Some(2), Some(3))
        );
        assert!((s["rate"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(s["byRepo"][0]["repo"], "/a");
        assert_eq!(s["byRepo"][0]["rate"], 0.5);
        assert_eq!(s["byRepo"][1]["rate"], 1.0);
        assert_eq!(s["byWeek"].as_array().unwrap().len(), 2);
        let recent = Query {
            repo: Some("/a".into()),
            since_days: Some(7),
            ..Query::default()
        };
        let s = lead_touch_summary(&tasks, &recent, now);
        assert_eq!(s["marked"], 2);
        assert_eq!(s["rate"], 0.5);
        assert_eq!(lead_touch_summary(&[], &q, now)["rate"], 0.0);
        let text = render(
            &json!({"rows": [], "totals": {}, "leadTouch": lead_touch_summary(&tasks, &q, now)}),
        );
        assert!(text.contains("lead touch  2/3 touched (67%)"), "{text}");
    }

    #[test]
    fn week_of_is_the_monday_of_the_week() {
        // 2023-11-14 was a Tuesday.
        assert_eq!(week_of(1_700_000_000_000), "2023-11-13");
        assert_eq!(week_of(0), "1969-12-29");
    }
}
