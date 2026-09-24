//! `orchd ab --data <dir>`: every task grouped by the experiment variant it
//! ran with, one row per variant, read straight from the task store.

use crate::model::{FailureKind, Stage, Task, TaskStatus, Verdict};
use crate::store::Store;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn run(args: &[String]) -> i32 {
    let data = args
        .windows(2)
        .find(|w| w[0] == "--data")
        .map(|w| PathBuf::from(&w[1]));
    let Some(data) = data else {
        eprintln!("orchd ab: --data <dir> is required");
        return 2;
    };
    if !data.join("tasks").is_dir() {
        eprintln!("orchd ab: no task store at {}", data.display());
        return 1;
    }
    match Store::new(&data).and_then(|s| s.list_tasks()) {
        Ok(tasks) => {
            print!("{}", report(&tasks));
            0
        }
        Err(e) => {
            eprintln!("orchd ab: {e}");
            1
        }
    }
}

fn median(mut xs: Vec<f64>) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(|a, b| a.total_cmp(b));
    Some(xs[xs.len() / 2])
}

fn fmt(x: Option<f64>, digits: usize) -> String {
    x.map(|v| format!("{v:.digits$}"))
        .unwrap_or_else(|| "-".into())
}

pub fn report(tasks: &[Task]) -> String {
    let mut groups: BTreeMap<String, Vec<&Task>> = BTreeMap::new();
    for t in tasks.iter().filter(|t| t.variant.is_some()) {
        let key = serde_json::to_string(&t.variant).unwrap_or_default();
        groups.entry(key).or_default().push(t);
    }
    let mut out = String::from(
        "| variant | tasks | done | attempts/task | $/task | median $ | median min to done | review FAIL | owner answers/task | stalls (cost not counted) |\n|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for (variant, ts) in groups {
        let n = ts.len() as f64;
        let done: Vec<&&Task> = ts.iter().filter(|t| t.status == TaskStatus::Done).collect();
        let implement = |t: &Task| {
            t.attempts
                .iter()
                .filter(|a| a.stage == Stage::Implement)
                .count()
        };
        let attempts: usize = ts.iter().map(|t| implement(t)).sum();
        let costs: Vec<f64> = ts.iter().map(|t| t.cost_usd).collect();
        let minutes: Vec<f64> = done
            .iter()
            .filter_map(|t| {
                // From the first implement attempt: drafting and plan
                // questions are not the variant's runtime.
                let start = t
                    .attempts
                    .iter()
                    .find(|a| a.stage == Stage::Implement)?
                    .started_at;
                let end = t.attempts.iter().filter_map(|a| a.ended_at).max()?;
                Some((end - start) as f64 / 60_000.0)
            })
            .collect();
        let reviews: Vec<Verdict> = ts
            .iter()
            .flat_map(|t| {
                t.attempts
                    .iter()
                    .filter_map(|a| a.review.as_ref().map(|r| r.verdict))
            })
            .collect();
        let review_fails = reviews.iter().filter(|v| **v == Verdict::Fail).count();
        let owner: usize = ts
            .iter()
            .map(|t| {
                t.decisions
                    .iter()
                    .filter(|d| d.starts_with("Owner:"))
                    .count()
            })
            .sum();
        let stalls = ts
            .iter()
            .flat_map(|t| &t.attempts)
            .filter(|a| a.failure.as_ref().map(|f| f.kind) == Some(FailureKind::Stall))
            .count();
        out.push_str(&format!(
            "| `{variant}` | {} | {} | {:.1} | {:.2} | {} | {} | {review_fails}/{} | {:.1} | {stalls} |\n",
            ts.len(),
            done.len(),
            attempts as f64 / n,
            costs.iter().sum::<f64>() / n,
            fmt(median(costs), 2),
            fmt(median(minutes), 0),
            reviews.len(),
            owner as f64 / n,
        ));
    }
    let before = tasks.iter().filter(|t| t.variant.is_none()).count();
    if before > 0 {
        out.push_str(&format!(
            "\n{before} task(s) from before variants left out.\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RetryMode, Variant};

    fn task(variant: Variant, status: TaskStatus, cost: f64, decisions: &[&str]) -> Task {
        let mut t: Task = serde_json::from_value(serde_json::json!({
            "id": "t", "title": "t", "goal": "g", "criteria": [], "verify": [],
            "repo": "/r", "worktree": "/w", "branch": "b", "baseSha": "s",
            "status": "queued", "tier": "standard", "createdAt": 0, "updatedAt": 0
        }))
        .unwrap();
        t.variant = Some(variant);
        t.status = status;
        t.cost_usd = cost;
        t.decisions = decisions.iter().map(|d| d.to_string()).collect();
        t
    }

    #[test]
    fn report_puts_each_variant_on_its_own_row_with_its_own_numbers() {
        let fresh = Variant {
            retry_mode: RetryMode::Fresh,
            ..Variant::default()
        };
        let tasks = vec![
            task(Variant::default(), TaskStatus::Done, 1.0, &["Owner: yes"]),
            task(Variant::default(), TaskStatus::Waiting, 3.0, &[]),
            task(fresh.clone(), TaskStatus::Done, 0.5, &[]),
        ];
        let mut old = task(Variant::default(), TaskStatus::Done, 9.0, &[]);
        old.variant = None;
        let tasks = [tasks, vec![old]].concat();
        let out = report(&tasks);
        assert!(
            out.contains("1 task(s) from before variants left out."),
            "{out}"
        );
        let rows: Vec<&str> = out.lines().skip(2).filter(|l| l.starts_with('|')).collect();
        assert_eq!(rows.len(), 2, "{out}");
        let resume = rows.iter().find(|r| r.contains("\"resume\"")).unwrap();
        assert!(resume.contains("| 2 | 1 |"), "{resume}");
        assert!(resume.contains("| 2.00 |"), "{resume}");
        assert!(
            resume.contains("| 0.5 |"),
            "owner answers per task: {resume}"
        );
        let fresh_row = rows.iter().find(|r| r.contains("\"fresh\"")).unwrap();
        assert!(fresh_row.contains("| 1 | 1 |"), "{fresh_row}");
        assert!(fresh_row.contains("| 0.50 |"), "{fresh_row}");
    }
}
