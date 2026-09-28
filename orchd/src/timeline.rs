//! Where a task's time and money went, and which failures recur across
//! tasks. Both are computed on demand from the task records and each run's
//! `events.jsonl`; nothing here is stored. `orchd failures --data <dir>`
//! prints the catalogue as a table.

use crate::ab::work_breakdown;
use crate::model::{
    now_ms, Attempt, AttemptStatus, FailureKind, Stage, Task, TaskStatus, Verdict, WorkBuckets,
};
use crate::store::Store;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// A gap between two pieces of work shorter than this is scheduling noise,
/// not a wait for the owner.
const MIN_WAIT_MS: i64 = 1000;
const DAY_MS: i64 = 86_400_000;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Buckets {
    pub process: u32,
    pub evidence: u32,
    pub verify: u32,
    pub task: u32,
    pub explore: u32,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    /// plan | implement | verify | review | advisor | final | wait
    pub stage: &'static str,
    pub attempt: u32,
    pub started_at: i64,
    pub ended_at: i64,
    pub cost_usd: f64,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<FailureKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buckets: Option<Buckets>,
}

fn seg(stage: &'static str, attempt: u32, started_at: i64, ended_at: i64) -> Segment {
    Segment {
        stage,
        attempt,
        started_at,
        ended_at: ended_at.max(started_at),
        cost_usd: 0.0,
        outcome: String::new(),
        failure_kind: None,
        buckets: None,
    }
}

fn mtime_ms(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64)
}

/// How long a side run (review, advisor) took: its brief is written just
/// before the harness spawns and `events.jsonl` last changes when it ends.
fn side_run_ms(dir: &Path) -> i64 {
    match (
        mtime_ms(&dir.join("brief.md")),
        mtime_ms(&dir.join("events.jsonl")),
    ) {
        (Some(a), Some(b)) => (b - a).max(0),
        _ => 0,
    }
}

fn read_events(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn status_word(a: &Attempt) -> &'static str {
    match a.status {
        AttemptStatus::Running => "running",
        AttemptStatus::Passed => "passed",
        AttemptStatus::Failed => "failed",
        AttemptStatus::Interrupted => "interrupted",
        AttemptStatus::Blocked => "blocked",
    }
}

/// The segments of one attempt, in the order the work happened. The
/// attempt's own window covers implement, verify, review and the final
/// checks; there are no per-stage timestamps, so verify and final are sized
/// from their recorded command durations and review from its run files, and
/// implement is what is left of the window.
fn attempt_segments(store: &Store, task: &Task, a: &Attempt, rules: &WorkBuckets) -> Vec<Segment> {
    let end = a.ended_at.unwrap_or(a.started_at);
    let run_dir = store.run_dir(&task.id, a.n);
    let failed = a.failure.as_ref().map(|f| f.kind);
    let mut out = Vec::new();

    if a.stage == Stage::Plan {
        let mut s = seg("plan", a.n, a.started_at, end);
        s.cost_usd = a.cost_usd.unwrap_or(0.0);
        s.outcome = status_word(a).into();
        s.failure_kind = failed;
        out.push(s);
    } else {
        let is_final = |c: &str| task.final_verify.iter().any(|f| f == c);
        let ms = |final_checks: bool| -> i64 {
            a.verify
                .iter()
                .filter(|v| is_final(&v.command) == final_checks)
                .map(|v| v.ms as i64)
                .sum()
        };
        let verify_ms = ms(false);
        let final_ms = ms(true);
        let has_review = a.review.is_some() || a.review_cost_usd.is_some();
        let review_ms = if has_review {
            side_run_ms(&run_dir.join("review"))
        } else {
            0
        };
        let implement_end = (end - verify_ms - review_ms - final_ms).max(a.started_at);

        let mut implement = seg("implement", a.n, a.started_at, implement_end);
        implement.cost_usd = a.cost_usd.unwrap_or(0.0);
        implement.outcome = status_word(a).into();
        if !matches!(
            failed,
            Some(FailureKind::Verify | FailureKind::Review) | None
        ) {
            implement.failure_kind = failed;
        }
        if a.stage == Stage::Implement {
            let changed_verify: Vec<String> = task
                .verify
                .iter()
                .chain(&task.final_verify)
                .cloned()
                .collect();
            let events = read_events(&run_dir.join("events.jsonl"));
            let b = work_breakdown(&events, &a.changed_files, &changed_verify, rules);
            if b.total() > 0 {
                implement.buckets = Some(Buckets {
                    process: b.process,
                    evidence: b.evidence,
                    verify: b.verify,
                    task: b.task,
                    explore: b.explore,
                });
            }
        }
        out.push(implement);

        let mut cursor = implement_end;
        if verify_ms > 0 || a.verify.iter().any(|v| !is_final(&v.command)) {
            let mut s = seg("verify", a.n, cursor, cursor + verify_ms);
            let bad = a
                .verify
                .iter()
                .any(|v| !is_final(&v.command) && v.code != Some(0));
            s.outcome = if bad { "fail" } else { "pass" }.into();
            if failed == Some(FailureKind::Verify) {
                s.failure_kind = failed;
            }
            cursor = s.ended_at;
            out.push(s);
        }
        if has_review {
            let mut s = seg("review", a.n, cursor, cursor + review_ms);
            s.cost_usd = a.review_cost_usd.unwrap_or(0.0);
            s.outcome = match a.review.as_ref().map(|r| r.verdict) {
                Some(Verdict::Pass) => "pass",
                Some(Verdict::Fail) => "fail",
                None => "error",
            }
            .into();
            if failed == Some(FailureKind::Review) {
                s.failure_kind = failed;
            }
            cursor = s.ended_at;
            out.push(s);
        }
        if a.verify.iter().any(|v| is_final(&v.command)) {
            let mut s = seg("final", a.n, cursor, cursor + final_ms);
            let bad = a
                .verify
                .iter()
                .any(|v| is_final(&v.command) && v.code != Some(0));
            s.outcome = if bad { "fail" } else { "pass" }.into();
            out.push(s);
        }
    }

    if a.advisor_cost_usd.is_some() {
        let dir = run_dir.join("advisor");
        let start = mtime_ms(&dir.join("brief.md")).unwrap_or(end);
        let mut s = seg("advisor", a.n, start, start + side_run_ms(&dir));
        s.cost_usd = a.advisor_cost_usd.unwrap_or(0.0);
        s.outcome = if a.advice.is_some() {
            "advised"
        } else {
            "none"
        }
        .into();
        out.push(s);
    }
    out
}

/// A task's timeline: its attempts' segments in time order, with a `wait`
/// segment for every gap where the task sat with nobody working on it
/// (an owner question, most often), and an open one while it is waiting now.
pub fn task_timeline(store: &Store, task: &Task, rules: &WorkBuckets, now: i64) -> Vec<Segment> {
    let mut attempts: Vec<&Attempt> = task.attempts.iter().collect();
    attempts.sort_by_key(|a| a.started_at);
    let mut out: Vec<Segment> = Vec::new();
    let mut covered_to: Option<i64> = None;
    for a in attempts {
        let segs = attempt_segments(store, task, a, rules);
        if let Some(prev) = covered_to {
            if a.started_at - prev >= MIN_WAIT_MS {
                let mut w = seg("wait", a.n, prev, a.started_at);
                w.outcome = "answered".into();
                out.push(w);
            }
        }
        let seg_end = segs
            .iter()
            .map(|s| s.ended_at)
            .max()
            .unwrap_or(a.started_at);
        covered_to = Some(covered_to.map_or(seg_end, |p| p.max(seg_end)));
        out.extend(segs);
    }
    if task.status == TaskStatus::Waiting {
        let from = covered_to.unwrap_or(task.created_at);
        let n = task.attempts.last().map_or(0, |a| a.n);
        let mut w = seg("wait", n, from, now.max(from));
        w.outcome = "open".into();
        out.push(w);
    }
    out
}

pub fn timeline_json(store: &Store, id: &str) -> Result<Value, String> {
    let task = store
        .load_task(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    let rules = store
        .load_settings()
        .map(|s| s.work_buckets)
        .unwrap_or_default();
    serde_json::to_value(task_timeline(store, &task, &rules, now_ms())).map_err(|e| e.to_string())
}

#[derive(Debug, Serialize, PartialEq)]
pub struct TaskRef {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FailureRow {
    pub signature: String,
    pub kind: FailureKind,
    pub count: u32,
    pub tasks: Vec<TaskRef>,
    pub last_seen: i64,
    pub example_detail: String,
    pub example_task_id: String,
}

/// Failures grouped by signature across `tasks`, most frequent first. A
/// failure is one failed attempt; `since_days` keeps those that ended within
/// that many days of `now`.
pub fn failure_catalogue(
    tasks: &[Task],
    repo: Option<&str>,
    since_days: Option<u32>,
    now: i64,
) -> Vec<FailureRow> {
    let cutoff = since_days.map(|d| now - i64::from(d) * DAY_MS);
    let mut rows: Vec<FailureRow> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for t in tasks.iter().filter(|t| repo.is_none_or(|r| t.repo == r)) {
        for a in &t.attempts {
            let Some(f) = &a.failure else { continue };
            let seen = a.ended_at.unwrap_or(a.started_at);
            if cutoff.is_some_and(|c| seen < c) {
                continue;
            }
            let i = *index.entry(f.signature.clone()).or_insert_with(|| {
                rows.push(FailureRow {
                    signature: f.signature.clone(),
                    kind: f.kind,
                    count: 0,
                    tasks: Vec::new(),
                    last_seen: i64::MIN,
                    example_detail: String::new(),
                    example_task_id: String::new(),
                });
                rows.len() - 1
            });
            let row = &mut rows[i];
            row.count += 1;
            if !row.tasks.iter().any(|r| r.id == t.id) {
                row.tasks.push(TaskRef {
                    id: t.id.clone(),
                    title: t.title.clone(),
                });
            }
            if seen >= row.last_seen {
                row.last_seen = seen;
                row.example_detail = f.detail.clone();
                row.example_task_id = t.id.clone();
            }
        }
    }
    rows.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then(b.last_seen.cmp(&a.last_seen))
            .then(a.signature.cmp(&b.signature))
    });
    rows
}

pub fn catalogue_json(store: &Store, params: &Value) -> Result<Value, String> {
    let repo = params.get("repo").and_then(Value::as_str);
    let since = params
        .get("sinceDays")
        .and_then(Value::as_u64)
        .map(|d| d.min(u64::from(u32::MAX)) as u32);
    let tasks = store.list_tasks().map_err(|e| e.to_string())?;
    serde_json::to_value(failure_catalogue(&tasks, repo, since, now_ms()))
        .map_err(|e| e.to_string())
}

fn one_line(s: &str, max: usize) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        out.push('~');
    }
    out.replace('|', "/")
}

pub fn failures_table(rows: &[FailureRow], now: i64) -> String {
    let mut out =
        String::from("| count | kind | tasks | last seen | signature |\n|---|---|---|---|---|\n");
    for r in rows {
        let ago_h = (now - r.last_seen).max(0) / 3_600_000;
        let ago = if ago_h >= 48 {
            format!("{}d ago", ago_h / 24)
        } else {
            format!("{ago_h}h ago")
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            r.count,
            r.kind.as_str(),
            r.tasks.len(),
            ago,
            one_line(&r.signature, 100)
        ));
    }
    out
}

/// `orchd failures --data <dir> [--repo <path>] [--since-days <n>]`.
pub fn run(args: &[String]) -> i32 {
    let flag = |name: &str| args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone());
    let Some(data) = flag("--data").map(PathBuf::from) else {
        eprintln!("orchd failures: --data <dir> is required");
        return 2;
    };
    if !data.join("tasks").is_dir() {
        eprintln!("orchd failures: no task store at {}", data.display());
        return 1;
    }
    let since = match flag("--since-days").map(|v| v.parse::<u32>()) {
        Some(Ok(d)) => Some(d),
        Some(Err(_)) => {
            eprintln!("orchd failures: --since-days takes a whole number");
            return 2;
        }
        None => None,
    };
    let tasks = match Store::new(&data).and_then(|s| s.list_tasks()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("orchd failures: {e}");
            return 1;
        }
    };
    let now = now_ms();
    print!(
        "{}",
        failures_table(
            &failure_catalogue(&tasks, flag("--repo").as_deref(), since, now),
            now
        )
    );
    0
}
