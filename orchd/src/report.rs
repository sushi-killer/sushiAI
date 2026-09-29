//! The one short report a finished top-level task or graph leaves for the
//! owner: deterministic markdown built from the task records, the cost
//! records and a few git facts, with no model call. `build` is pure; the
//! engine gathers `Facts` and stores the result (`engine/report.rs`).

use crate::brief;
use crate::costs::CostRecord;
use crate::model::{AttemptStatus, Stage, Task, TaskStatus, Verdict};
use std::collections::{BTreeMap, HashSet};

/// What git says about the work; empty when the repository cannot tell.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// `<short sha> <subject>`, newest first.
    pub commits: Vec<String>,
    /// Path, lines added, lines removed (`None` for a binary file).
    pub files: Vec<(String, Option<u64>, Option<u64>)>,
    /// Evidence images: screenshots the runs left and images the work added.
    pub images: Vec<String>,
}

pub const IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

pub fn is_image(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

fn money(v: f64) -> String {
    format!("${v:.3}")
}

fn minutes(ms: i64) -> String {
    let secs = (ms / 1000).max(0);
    if secs < 90 {
        format!("{secs}s")
    } else {
        format!("{} min", (secs + 30) / 60)
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Criteria the owner (or the answer policy) dropped, as their text: the
/// live criteria list no longer holds them.
fn dropped_criteria(task: &Task) -> Vec<String> {
    task.decisions
        .iter()
        .filter_map(|d| {
            let rest = d.strip_prefix("Orchestrator: dropped criterion ")?;
            let rest = rest.strip_suffix(" as the owner answered")?;
            let (_, text) = rest.split_once(" (")?;
            Some(text.strip_suffix(')').unwrap_or(text).to_string())
        })
        .collect()
}

/// The graph of `top`: itself first, then every descendant in the order
/// `all` lists them.
pub fn graph_of<'a>(top: &'a Task, all: &'a [Task]) -> Vec<&'a Task> {
    let mut ids: HashSet<&str> = HashSet::from([top.id.as_str()]);
    let mut out = vec![top];
    loop {
        let before = out.len();
        for t in all {
            if !ids.contains(t.id.as_str()) && t.parent.as_deref().is_some_and(|p| ids.contains(p))
            {
                ids.insert(t.id.as_str());
                out.push(t);
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// The last implement attempt that reached a review, if any.
fn last_review(task: &Task) -> Option<&crate::model::ReviewResult> {
    task.attempts
        .iter()
        .rev()
        .filter(|a| a.stage == Stage::Implement)
        .find_map(|a| a.review.as_ref())
}

/// Statuses of criterion `i`, in the words the report uses.
fn criterion_status(task: &Task, i: usize) -> Vec<String> {
    let mut out = Vec::new();
    let text = &task.criteria[i];
    let review = last_review(task);
    let mentions = |prefix: &str| {
        review.is_some_and(|r| {
            r.findings
                .iter()
                .any(|f| f.starts_with(prefix) && f.contains(text.as_str()))
        })
    };
    match review {
        Some(_) if mentions("Unmet criterion") => out.push("unmet (review)".to_string()),
        Some(_) if mentions("Not checked by review") => out.push("unchecked".to_string()),
        Some(r) if r.verdict == Verdict::Pass => out.push("met by review".to_string()),
        Some(_) => out.push("not met at the last review".to_string()),
        None => out.push("unchecked".to_string()),
    }
    let outcomes = task
        .attempts
        .iter()
        .rev()
        .find(|a| !a.verify.is_empty())
        .map(|a| a.verify.as_slice())
        .unwrap_or_default();
    for check in task.checks.iter().filter(|c| c.criterion == i) {
        if let Some(v) = outcomes.iter().find(|v| v.command == check.run) {
            let verdict = if v.code == Some(0) {
                "passed"
            } else {
                "failed"
            };
            out.push(format!("grounded check {verdict}: `{}`", check.run));
        }
    }
    let held = brief::held_out_label(i);
    if let Some(v) = outcomes.iter().find(|v| v.command == held) {
        let verdict = if v.code == Some(0) {
            "passed"
        } else {
            "failed"
        };
        out.push(format!("{held} {verdict}"));
    }
    out
}

fn push_criteria(out: &mut String, task: &Task, facts: &Facts, used: &mut HashSet<String>) {
    for (i, text) in task.criteria.iter().enumerate() {
        out.push_str(&format!(
            "- {text} - {}\n",
            criterion_status(task, i).join("; ")
        ));
        for image in facts.images.iter().filter(|p| {
            text.contains(basename(p))
                || task
                    .checks
                    .iter()
                    .any(|c| c.criterion == i && c.run.contains(basename(p)))
        }) {
            used.insert(image.clone());
            out.push_str(&format!("  ![{}]({image})\n", basename(image)));
        }
    }
    for text in dropped_criteria(task) {
        out.push_str(&format!("- {text} - dropped\n"));
    }
}

fn attempts_line(task: &Task) -> String {
    let attempts: Vec<_> = task
        .attempts
        .iter()
        .filter(|a| a.stage == Stage::Implement)
        .collect();
    let mut kinds: BTreeMap<&str, u32> = BTreeMap::new();
    for a in &attempts {
        if let Some(f) = &a.failure {
            *kinds.entry(f.kind.as_str()).or_default() += 1;
        }
    }
    let failed = attempts
        .iter()
        .filter(|a| a.status == AttemptStatus::Failed)
        .count();
    let mut line = format!(
        "- {}: {} attempt{}",
        task.title,
        attempts.len(),
        if attempts.len() == 1 { "" } else { "s" }
    );
    if failed > 0 || !kinds.is_empty() {
        let list: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} x{n}")).collect();
        line.push_str(&format!(", {failed} failed ({})", list.join(", ")));
    }
    line.push('\n');
    line
}

fn cost_tables(out: &mut String, records: &[&CostRecord]) {
    let mut by_stage: BTreeMap<&str, (f64, u32, u64)> = BTreeMap::new();
    let mut by_model: BTreeMap<&str, (f64, u32)> = BTreeMap::new();
    for r in records {
        let s = by_stage.entry(r.stage.as_str()).or_default();
        s.0 += r.cost_usd;
        s.1 += 1;
        s.2 += r.ms;
        let m = by_model.entry(r.model.as_str()).or_default();
        m.0 += r.cost_usd;
        m.1 += 1;
    }
    out.push_str("| Stage | Runs | Cost | Time |\n|---|---|---|---|\n");
    for (stage, (cost, runs, ms)) in &by_stage {
        out.push_str(&format!(
            "| {stage} | {runs} | {} | {} |\n",
            money(*cost),
            minutes(*ms as i64)
        ));
    }
    out.push_str("\n| Model | Runs | Cost |\n|---|---|---|\n");
    for (model, (cost, runs)) in &by_model {
        out.push_str(&format!("| {model} | {runs} | {} |\n", money(*cost)));
    }
}

/// The report for `top` (a top-level task) and its `graph` (`graph_of`).
pub fn build(top: &Task, graph: &[&Task], records: &[CostRecord], facts: &Facts) -> String {
    let mut out = format!("# {}\n\n", top.title);
    if !top.goal.trim().is_empty() {
        out.push_str(&format!("{}\n\n", top.goal.trim()));
    }

    out.push_str("## Outcome\n\n");
    let status = match top.status {
        TaskStatus::Done => "done",
        _ => "not done",
    };
    match &top.landed_sha {
        Some(sha) => out.push_str(&format!(
            "- {status}, landed on `{}` at `{}`\n",
            top.base_ref.as_deref().unwrap_or("its base"),
            sha.chars().take(9).collect::<String>()
        )),
        None => out.push_str(&format!("- {status}, on branch `{}`\n", top.branch)),
    }
    match &top.lead_touch {
        Some(t) => out.push_str(&format!(
            "- Lead touch: {} (by {}){}\n",
            if t.touched { "needed a fix" } else { "clean" },
            t.by,
            if t.note.is_empty() {
                String::new()
            } else {
                format!(": {}", t.note)
            }
        )),
        None => out.push_str("- Lead touch: unknown\n"),
    }

    out.push_str("\n## What changed\n\n");
    if facts.commits.is_empty() && facts.files.is_empty() {
        out.push_str("- Nothing recorded.\n");
    }
    for c in &facts.commits {
        out.push_str(&format!("- commit {c}\n"));
    }
    if !facts.files.is_empty() {
        let plus: u64 = facts.files.iter().filter_map(|f| f.1).sum();
        let minus: u64 = facts.files.iter().filter_map(|f| f.2).sum();
        out.push_str(&format!(
            "\n{} files changed, +{plus} -{minus}\n\n",
            facts.files.len()
        ));
        for (path, a, r) in &facts.files {
            match (a, r) {
                (Some(a), Some(r)) => out.push_str(&format!("- `{path}` +{a} -{r}\n")),
                _ => out.push_str(&format!("- `{path}` (binary)\n")),
            }
        }
    }

    out.push_str("\n## Criteria\n\n");
    let mut used = HashSet::new();
    for task in graph {
        if graph.len() > 1 {
            out.push_str(&format!("**{}**\n\n", task.title));
        }
        push_criteria(&mut out, task, facts, &mut used);
        out.push('\n');
    }
    let loose: Vec<&String> = facts.images.iter().filter(|i| !used.contains(*i)).collect();
    if !loose.is_empty() {
        out.push_str("Evidence images:\n\n");
        for image in loose {
            out.push_str(&format!("![{}]({image})\n", basename(image)));
        }
        out.push('\n');
    }

    out.push_str("## Assumptions and automatic answers\n\n");
    let mut any = false;
    for task in graph {
        for a in &task.assumptions {
            any = true;
            let who = match a.by.as_str() {
                "policy" => "answered by rule",
                "judge" => "answered by the cheap judge",
                _ => "assumed by the planner",
            };
            let state = if a.overturned {
                format!(
                    "overturned by the owner: {}",
                    a.owner_answer.as_deref().unwrap_or("")
                )
            } else {
                "overturnable with `task.overturn`".to_string()
            };
            out.push_str(&format!(
                "- {}: {} -> {} ({who}; {state})\n",
                task.title, a.question, a.answer
            ));
        }
    }
    if !any {
        out.push_str("- None.\n");
    }

    out.push_str("\n## Cost\n\n");
    let ids: HashSet<&str> = graph.iter().map(|t| t.id.as_str()).collect();
    let mine: Vec<&CostRecord> = records
        .iter()
        .filter(|r| r.task_id.as_deref().is_some_and(|id| ids.contains(id)))
        .collect();
    let total: f64 = mine.iter().map(|r| r.cost_usd).sum();
    out.push_str(&format!(
        "Total {} over {} wall time.\n\n",
        money(total),
        minutes(top.updated_at - top.created_at)
    ));
    if mine.is_empty() {
        out.push_str("No cost records.\n");
    } else {
        cost_tables(&mut out, &mine);
    }

    out.push_str("\n## Attempts\n\n");
    for task in graph {
        out.push_str(&attempts_line(task));
    }

    out.push_str("\n## Follow-ups\n\n");
    let mut any = false;
    for task in graph {
        let handoff = task
            .attempts
            .iter()
            .rev()
            .find(|a| a.stage == Stage::Implement && a.status == AttemptStatus::Passed)
            .and_then(|a| a.handoff.as_deref())
            .map(str::trim)
            .filter(|h| !h.is_empty());
        if let Some(h) = handoff {
            any = true;
            out.push_str(&format!("- {}: {h}\n", task.title));
        }
    }
    if !any {
        out.push_str("- None.\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_image_matches_extensions_case_insensitively() {
        assert!(is_image("artifacts/a.PNG"));
        assert!(!is_image("src/a.rs"));
        assert!(!is_image("png"));
    }

    #[test]
    fn minutes_reads_seconds_then_minutes() {
        assert_eq!(minutes(12_000), "12s");
        assert_eq!(minutes(600_000), "10 min");
    }
}
