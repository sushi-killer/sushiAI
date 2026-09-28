//! `orchd ab --data <dir>`: every task grouped by the experiment variant it
//! ran with, one row per variant, read straight from the task store.

use crate::model::{Attempt, FailureKind, Stage, Task, TaskStatus, Verdict};
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
    let mid = xs.len() / 2;
    Some(if xs.len().is_multiple_of(2) {
        (xs[mid - 1] + xs[mid]) / 2.0
    } else {
        xs[mid]
    })
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
        "| variant | tasks | done | attempts/task | $/task | median $ | median min to done | review FAIL | owner answers/task | stalls (cost not counted) | median prefix tokens | cache-read tokens/attempt |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n",
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
        let implement_attempts: Vec<&Attempt> = ts
            .iter()
            .flat_map(|t| &t.attempts)
            .filter(|a| a.stage == Stage::Implement)
            .collect();
        // Fresh Claude implement attempts only; the rest record none.
        let prefixes: Vec<f64> = implement_attempts
            .iter()
            .filter_map(|a| a.prefix_tokens.map(|p| p as f64))
            .collect();
        let cached: Vec<f64> = implement_attempts
            .iter()
            .filter_map(|a| a.usage.as_ref().map(|u| u.cached as f64))
            .collect();
        let mean_cached =
            (!cached.is_empty()).then(|| cached.iter().sum::<f64>() / cached.len() as f64);
        let stalls = ts
            .iter()
            .flat_map(|t| &t.attempts)
            .filter(|a| a.failure.as_ref().map(|f| f.kind) == Some(FailureKind::Stall))
            .count();
        out.push_str(&format!(
            "| `{variant}` | {} | {} | {:.1} | {:.2} | {} | {} | {review_fails}/{} | {:.1} | {stalls} | {} | {} |\n",
            ts.len(),
            done.len(),
            attempts as f64 / n,
            costs.iter().sum::<f64>() / n,
            fmt(median(costs), 2),
            fmt(median(minutes), 0),
            reviews.len(),
            owner as f64 / n,
            fmt(median(prefixes), 0),
            fmt(mean_cached, 0),
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
    fn median_of_an_even_count_is_the_mean_of_the_middle_two() {
        assert_eq!(median(vec![6.1, 0.66]), Some((6.1 + 0.66) / 2.0));
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![]), None);
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
        assert!(
            fresh_row.ends_with("| 0 | - | - |"),
            "nothing recorded: {fresh_row}"
        );
    }

    fn attempt(stage: &str, prefix: Option<u64>, cached: Option<u64>) -> Attempt {
        let mut a: Attempt = serde_json::from_value(serde_json::json!({
            "n": 1, "stage": stage, "routeId": "r", "harness": "claude", "model": "m",
            "reason": "x", "resumed": false, "startedAt": 0, "status": "passed"
        }))
        .unwrap();
        a.prefix_tokens = prefix;
        a.usage = cached.map(|c| crate::model::Usage {
            input: 0,
            output: 0,
            cached: c,
        });
        a
    }

    #[test]
    fn report_shows_median_prefix_and_mean_cache_read_tokens_per_implement_attempt() {
        let lean = Variant {
            lean_context: true,
            ..Variant::default()
        };
        let mut a = task(lean.clone(), TaskStatus::Done, 1.0, &[]);
        a.attempts = vec![
            attempt("plan", Some(90_000), Some(90_000)),
            attempt("implement", Some(20_000), Some(1_000)),
            attempt("implement", None, Some(3_000)),
        ];
        let mut b = task(lean, TaskStatus::Done, 1.0, &[]);
        b.attempts = vec![
            attempt("implement", Some(30_000), Some(5_000)),
            attempt("implement", Some(22_000), None),
        ];
        let out = report(&[a, b]);
        let row = out
            .lines()
            .find(|l| l.contains("\"leanContext\":true"))
            .unwrap();
        // Median of 20k, 30k, 22k; mean cached of 1k, 3k, 5k.
        assert!(row.ends_with("| 22000 | 3000 |"), "{row}");
        assert!(out
            .lines()
            .next()
            .unwrap()
            .ends_with("| median prefix tokens | cache-read tokens/attempt |"));
    }

    #[test]
    fn lean_output_gets_its_own_row_separate_from_lean_context() {
        let lean_output = Variant {
            lean_output: true,
            ..Variant::default()
        };
        let mut a = task(lean_output, TaskStatus::Done, 1.0, &[]);
        a.attempts = vec![attempt("implement", Some(15_000), Some(2_000))];
        let lean_context = Variant {
            lean_context: true,
            ..Variant::default()
        };
        let mut b = task(lean_context, TaskStatus::Done, 1.0, &[]);
        b.attempts = vec![attempt("implement", Some(30_000), Some(5_000))];

        let out = report(&[a, b]);
        let rows: Vec<&str> = out.lines().skip(2).filter(|l| l.starts_with('|')).collect();
        assert_eq!(rows.len(), 2, "{out}");
        let lean_output_row = rows
            .iter()
            .find(|r| r.contains("\"leanOutput\":true"))
            .unwrap();
        assert!(
            !lean_output_row.contains("\"leanContext\":true"),
            "{lean_output_row}"
        );
        assert!(
            lean_output_row.ends_with("| 15000 | 2000 |"),
            "{lean_output_row}"
        );
        assert!(rows.iter().any(|r| r.contains("\"leanContext\":true")));
    }
}
