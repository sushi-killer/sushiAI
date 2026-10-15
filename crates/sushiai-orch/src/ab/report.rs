use crate::ab::Breakdown;
use crate::task::{Attempt, FailureKind, Stage, Task, TaskStatus, Verdict};
use std::collections::BTreeMap;

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

/// `breakdown` gives one implement attempt's work breakdown, `None` when its
/// events are unavailable or it made no tool calls (Codex runs, which log a
/// different event shape).
pub fn report_with(
    tasks: &[Task],
    eval: Option<&str>,
    breakdown: &dyn Fn(&Task, &Attempt) -> Option<Breakdown>,
) -> String {
    let mut groups: BTreeMap<(String, String), Vec<&Task>> = BTreeMap::new();
    let in_scope = |t: &&Task| eval.is_none_or(|e| t.eval_set.as_deref() == Some(e));
    // A subtask's cost is already in its parent's: the request counts once.
    for t in tasks
        .iter()
        .filter(in_scope)
        .filter(|t| t.variant.is_some() && t.parent.is_none())
    {
        let key = serde_json::to_string(&t.variant).unwrap_or_default();
        groups
            .entry((key, fingerprint_label(t)))
            .or_default()
            .push(t);
    }
    let headline_groups = groups.clone();
    let mut fingerprints: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (variant, fp) in groups.keys() {
        fingerprints
            .entry(variant.clone())
            .or_default()
            .push(fp.clone());
    }
    let mut out = String::from(
        "| variant | fingerprint | tasks | done | attempts/task | $/task | median $ | median min to done | review FAIL | owner answers/task | assumptions/task | overturn rate | process % | evidence % | verify % | task % | explore % | tool calls/attempt | stalls | median prefix tokens | cache-read tokens/attempt |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for ((variant, fingerprint), tasks) in groups {
        out.push_str(&report_row(&variant, &fingerprint, &tasks, breakdown));
    }
    for (variant, fps) in fingerprints.iter().filter(|(_, f)| f.len() > 1) {
        out.push_str(&format!(
            "\nvariant `{variant}` mixes {} fingerprints: {}.\n",
            fps.len(),
            fps.join(", ")
        ));
    }
    if eval.is_some() {
        out.push_str(&headline_table(&headline_groups));
        out.push_str(&stale_baselines(&headline_groups));
        out.push_str(&paired_table(&headline_groups));
        out.push_str(&per_task_table(tasks, eval));
    }
    let before = tasks
        .iter()
        .filter(in_scope)
        .filter(|t| t.variant.is_none())
        .count();
    if before > 0 {
        out.push_str(&format!(
            "\n{before} task(s) from before variants left out.\n"
        ));
    }
    out
}

/// A task is a success when it is done and its eval check, if the set gave
/// one, exited 0. A check that was set but never ran (or was cut off) is not.
fn succeeded(t: &Task) -> bool {
    t.status == TaskStatus::Done
        && match (&t.eval_check_cmd, &t.eval_check) {
            (None, _) => true,
            (Some(_), Some(c)) => c.code == Some(0),
            (Some(_), None) => false,
        }
}

type Groups<'a> = BTreeMap<(String, String), Vec<&'a Task>>;

/// Tasks of one group by eval name, in name order.
fn by_name<'a>(ts: &[&'a Task]) -> BTreeMap<String, Vec<&'a Task>> {
    let mut names: BTreeMap<String, Vec<&Task>> = BTreeMap::new();
    for t in ts {
        let name = t.eval_name.clone().unwrap_or_else(|| "-".into());
        names.entry(name).or_default().push(t);
    }
    names
}

/// What labs report, per variant and fingerprint: pass@1 over all runs, pass^k
/// (the share of eval tasks whose every one of k repeats succeeded, k the
/// largest repeat count seen), cost per successful task and median minutes to
/// done.
fn headline_table(groups: &Groups) -> String {
    let mut out = String::from(
        "\n| variant | fingerprint | runs | success rate | pass^k | $/successful task | median min to done |\n|---|---|---|---|---|---|---|\n",
    );
    for ((variant, fingerprint), ts) in groups {
        let successes = ts.iter().filter(|t| succeeded(t)).count();
        let names = by_name(ts);
        let k = names.values().map(Vec::len).max().unwrap_or(0);
        let all_k = names
            .values()
            .filter(|runs| runs.len() == k && runs.iter().all(|t| succeeded(t)))
            .count();
        let cost: f64 = ts.iter().map(|t| t.cost_usd).sum();
        let minutes: Vec<f64> = ts
            .iter()
            .filter(|t| t.status == TaskStatus::Done)
            .filter_map(|t| {
                let start = t
                    .attempts
                    .iter()
                    .find(|a| a.stage == Stage::Implement)?
                    .started_at;
                let end = t.attempts.iter().filter_map(|a| a.ended_at).max()?;
                Some((end - start) as f64 / 60_000.0)
            })
            .collect();
        out.push_str(&format!(
            "| `{variant}` | {fingerprint} | {} | {}/{} = {:.0}% | {all_k}/{} = {:.0}% (k={k}) | {} | {} |\n",
            ts.len(),
            successes,
            ts.len(),
            successes as f64 / ts.len() as f64 * 100.0,
            names.len(),
            all_k as f64 / names.len().max(1) as f64 * 100.0,
            fmt((successes > 0).then(|| cost / successes as f64), 2),
            fmt(median(minutes), 0),
        ));
    }
    out
}

/// One line per variant whose runs in this set carry a fingerprint other than
/// the newest one for that variant: its older numbers were measured on a
/// different model, harness version or prompt.
fn stale_baselines(groups: &Groups) -> String {
    let mut newest: BTreeMap<&str, (i64, &str)> = BTreeMap::new();
    for ((variant, fingerprint), ts) in groups {
        if fingerprint == "-" {
            continue;
        }
        let latest = ts.iter().map(|t| t.created_at).max().unwrap_or(0);
        let e = newest.entry(variant).or_insert((latest, fingerprint));
        if latest > e.0 {
            *e = (latest, fingerprint);
        }
    }
    let mut out = String::new();
    for (variant, fingerprint) in groups.keys() {
        if let Some((_, new)) = newest.get(variant.as_str()) {
            if fingerprint != "-" && fingerprint != new {
                out.push_str(&format!(
                    "\nbaseline stale: {fingerprint} -> {new} (variant `{variant}`)\n"
                ));
            }
        }
    }
    out
}

/// Per eval task, each variant's successes/k and total $.
fn paired_table(groups: &Groups) -> String {
    let mut variants: Vec<&String> = groups.keys().map(|(v, _)| v).collect();
    variants.dedup();
    let mut cells: BTreeMap<String, BTreeMap<&String, (usize, usize, f64)>> = BTreeMap::new();
    for ((variant, _), ts) in groups {
        for (name, runs) in by_name(ts) {
            let c = cells
                .entry(name)
                .or_default()
                .entry(variant)
                .or_insert((0, 0, 0.0));
            c.0 += runs.iter().filter(|t| succeeded(t)).count();
            c.1 += runs.len();
            c.2 += runs.iter().map(|t| t.cost_usd).sum::<f64>();
        }
    }
    let mut out = String::from("\n| eval task |");
    for v in &variants {
        out.push_str(&format!(" `{v}` successes/k, $ |"));
    }
    out.push_str("\n|---|");
    out.push_str(&"---|".repeat(variants.len()));
    out.push('\n');
    for (name, row) in cells {
        out.push_str(&format!("| {name} |"));
        for v in &variants {
            match row.get(v) {
                Some((ok, k, cost)) => out.push_str(&format!(" {ok}/{k}, ${cost:.2} |")),
                None => out.push_str(" - |"),
            }
        }
        out.push('\n');
    }
    out
}

/// What the task's first implement attempt really ran (`model (version)`),
/// `-` for a task from before fingerprints, or one that never implemented.
fn fingerprint_label(t: &Task) -> String {
    t.attempts
        .iter()
        .filter(|a| a.stage == Stage::Implement)
        .find_map(|a| a.fingerprint.as_ref())
        .map_or_else(
            || "-".to_string(),
            |f| {
                format!(
                    "{}: {}",
                    format!("{:?}", f.harness).to_lowercase(),
                    f.label()
                )
            },
        )
}

/// One row per (evalName, variant): the same task under each arm, so arms
/// compare on the same work and not on averages. Means when a task ran more
/// than once per arm (`--repeat`).
fn per_task_table(tasks: &[Task], eval: Option<&str>) -> String {
    let mut rows: BTreeMap<(String, String, String), Vec<&Task>> = BTreeMap::new();
    for t in tasks {
        if t.variant.is_none() || t.eval_set.as_deref() != eval || eval.is_none() {
            continue;
        }
        let name = t.eval_name.clone().unwrap_or_else(|| "-".into());
        let variant = serde_json::to_string(&t.variant).unwrap_or_default();
        rows.entry((name, variant, fingerprint_label(t)))
            .or_default()
            .push(t);
    }
    let mut out = String::from(
        "\n| task | variant | fingerprint | runs | done | $ | attempts |\n|---|---|---|---|---|---|---|\n",
    );
    for ((name, variant, fingerprint), ts) in rows {
        let n = ts.len() as f64;
        let done = ts.iter().filter(|t| t.status == TaskStatus::Done).count();
        let attempts: usize = ts
            .iter()
            .map(|t| {
                t.attempts
                    .iter()
                    .filter(|a| a.stage == Stage::Implement)
                    .count()
            })
            .sum();
        let cost: f64 = ts.iter().map(|t| t.cost_usd).sum();
        out.push_str(&format!(
            "| {name} | `{variant}` | {fingerprint} | {} | {done} | {:.2} | {:.1} |\n",
            ts.len(),
            cost / n,
            attempts as f64 / n,
        ));
    }
    out
}

fn report_row(
    variant: &str,
    fingerprint: &str,
    ts: &[&Task],
    breakdown: &dyn Fn(&Task, &Attempt) -> Option<Breakdown>,
) -> String {
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
    let assumed: usize = ts.iter().map(|t| t.assumptions.len()).sum();
    let overturned: usize = ts
        .iter()
        .flat_map(|t| &t.assumptions)
        .filter(|a| a.overturned)
        .count();
    let overturn_rate = (assumed > 0).then(|| overturned as f64 / assumed as f64 * 100.0);
    format!(
        "| `{variant}` | {fingerprint} | {} | {} | {:.1} | {:.2} | {} | {} | {review_fails}/{} | {:.1} | {:.1} | {} | {}",
        ts.len(),
        done.len(),
        attempts as f64 / n,
        costs.iter().sum::<f64>() / n,
        fmt(median(costs), 2),
        fmt(median(minutes), 0),
        reviews.len(),
        owner as f64 / n,
        assumed as f64 / n,
        overturn_rate.map(|rate| format!("{rate:.0}%")).unwrap_or_else(|| "-".into()),
        work_columns(ts, breakdown),
    )
}

fn work_columns(ts: &[&Task], breakdown: &dyn Fn(&Task, &Attempt) -> Option<Breakdown>) -> String {
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
    let shares: Vec<(Breakdown, f64)> = ts
        .iter()
        .flat_map(|t| {
            t.attempts
                .iter()
                .filter(|a| a.stage == Stage::Implement)
                .filter_map(|a| breakdown(t, a))
        })
        .filter(|b| b.total() > 0)
        .map(|b| (b, b.total() as f64))
        .collect();
    let share = |pick: fn(&Breakdown) -> u32| {
        (!shares.is_empty()).then(|| {
            shares
                .iter()
                .map(|(b, n)| pick(b) as f64 / n * 100.0)
                .sum::<f64>()
                / shares.len() as f64
        })
    };
    let calls = (!shares.is_empty())
        .then(|| shares.iter().map(|(_, n)| n).sum::<f64>() / shares.len() as f64);
    let stalls = ts
        .iter()
        .flat_map(|t| &t.attempts)
        .filter(|a| a.failure.as_ref().map(|f| f.kind) == Some(FailureKind::Stall))
        .count();
    format!(
        "{} | {} | {} | {} | {} | {} | {stalls} | {} | {} |\n",
        fmt(share(|b| b.process), 0),
        fmt(share(|b| b.evidence), 0),
        fmt(share(|b| b.verify), 0),
        fmt(share(|b| b.task), 0),
        fmt(share(|b| b.explore), 0),
        fmt(calls, 0),
        fmt(median(prefixes), 0),
        fmt(mean_cached, 0),
    )
}

#[cfg(test)]
#[path = "report/tests.rs"]
mod tests;
