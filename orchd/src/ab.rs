//! `orchd ab --data <dir>`: every task grouped by the experiment variant it
//! ran with, one row per variant, read straight from the task store.
//! `--eval <set>` limits it to one eval set's tasks and adds a per-task table.

use crate::model::{Attempt, FailureKind, Stage, Task, TaskStatus, Verdict, WorkBuckets};
use crate::store::Store;
use serde_json::Value;
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
    let eval = args
        .windows(2)
        .find(|w| w[0] == "--eval")
        .map(|w| w[1].clone());
    let store = match Store::new(&data) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("orchd ab: {e}");
            return 1;
        }
    };
    let rules = store
        .load_settings()
        .map(|s| s.work_buckets)
        .unwrap_or_default();
    match store.list_tasks() {
        Ok(tasks) => {
            let breakdown = |t: &Task, a: &Attempt| {
                let path = store.run_dir(&t.id, a.n).join("events.jsonl");
                let text = std::fs::read_to_string(path).ok()?;
                let events: Vec<Value> = text
                    .lines()
                    .filter_map(|l| serde_json::from_str(l).ok())
                    .collect();
                let verify: Vec<String> = t.verify.iter().chain(&t.final_verify).cloned().collect();
                Some(work_breakdown(&events, &a.changed_files, &verify, &rules))
            };
            print!("{}", report_with(&tasks, eval.as_deref(), &breakdown));
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

/// Where an implement attempt's tool calls went, by kind.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Breakdown {
    pub process: u32,
    pub evidence: u32,
    pub verify: u32,
    pub task: u32,
    pub explore: u32,
}

impl Breakdown {
    pub fn total(&self) -> u32 {
        self.process + self.evidence + self.verify + self.task + self.explore
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Process,
    Evidence,
    Verify,
    Task,
    Explore,
}

/// Build, test and lint tools common across ecosystems; a task's own verify
/// commands are added per task. Repository-specific paths come from
/// `settings.workBuckets`, never from here.
const VERIFY_COMMANDS: [&str; 18] = [
    "cargo test",
    "cargo clippy",
    "cargo fmt",
    "cargo build",
    "npm run",
    "npm test",
    "pnpm ",
    "yarn ",
    "node --test",
    "pytest",
    "go test",
    "make ",
    "tsc",
    "eslint",
    "prettier",
    "ruff",
    "mypy",
    "gradle",
];
const EVIDENCE_WORDS: [&str; 3] = ["screenshot", "playwright", ".png"];

/// Sorts one top-level tool call into a bucket, first match wins in the
/// order process, evidence, verify, task, explore.
fn classify_call(
    name: &str,
    input: &Value,
    changed: &[String],
    verify: &[String],
    rules: &WorkBuckets,
) -> Bucket {
    let text = input.to_string();
    if rules.process.iter().any(|p| text.contains(p.as_str())) {
        return Bucket::Process;
    }
    if EVIDENCE_WORDS.iter().any(|w| text.contains(w))
        || rules.evidence.iter().any(|p| text.contains(p.as_str()))
    {
        return Bucket::Evidence;
    }
    if name == "Bash" {
        let command = input.get("command").and_then(Value::as_str).unwrap_or("");
        if VERIFY_COMMANDS.iter().any(|c| command.contains(c))
            || verify
                .iter()
                .any(|v| !v.is_empty() && command.contains(v.as_str()))
        {
            return Bucket::Verify;
        }
    }
    if changed.iter().any(|f| !f.is_empty() && text.contains(f)) {
        return Bucket::Task;
    }
    Bucket::Explore
}

/// Splits an implement attempt's top-level tool calls (assistant `tool_use`
/// blocks in `events.jsonl`; a subagent's carry `parent_tool_use_id` and are
/// ignored) into process, evidence, verify, task and explore, given the files
/// the attempt changed. Path/keyword matching on the tool input, not
/// judgement: it can misfile a call (a `tsc` inside another word, an edit to
/// a changed file done through `sed -i`). Heuristic on purpose; the upgrade
/// path is an LLM grader over the same events, which needs a model call per
/// attempt and is not worth it yet.
pub fn work_breakdown(
    events: &[Value],
    changed: &[String],
    verify: &[String],
    rules: &WorkBuckets,
) -> Breakdown {
    let mut b = Breakdown::default();
    for ev in events {
        if ev.get("type").and_then(Value::as_str) != Some("assistant")
            || ev.get("parent_tool_use_id").is_some_and(|p| !p.is_null())
        {
            continue;
        }
        let Some(blocks) = ev.pointer("/message/content").and_then(Value::as_array) else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            let input = block.get("input").unwrap_or(&Value::Null);
            match classify_call(name, input, changed, verify, rules) {
                Bucket::Process => b.process += 1,
                Bucket::Evidence => b.evidence += 1,
                Bucket::Verify => b.verify += 1,
                Bucket::Task => b.task += 1,
                Bucket::Explore => b.explore += 1,
            }
        }
    }
    b
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
    let mut groups: BTreeMap<String, Vec<&Task>> = BTreeMap::new();
    let in_scope = |t: &&Task| eval.is_none_or(|e| t.eval_set.as_deref() == Some(e));
    // A subtask's cost is already in its parent's: the request counts once.
    for t in tasks
        .iter()
        .filter(in_scope)
        .filter(|t| t.variant.is_some() && t.parent.is_none())
    {
        let key = serde_json::to_string(&t.variant).unwrap_or_default();
        groups.entry(key).or_default().push(t);
    }
    let mut out = String::from(
        "| variant | tasks | done | attempts/task | $/task | median $ | median min to done | review FAIL | owner answers/task | process % | evidence % | verify % | task % | explore % | tool calls/attempt | stalls (cost not counted) | median prefix tokens | cache-read tokens/attempt |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
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
        out.push_str(&format!(
            "| `{variant}` | {} | {} | {:.1} | {:.2} | {} | {} | {review_fails}/{} | {:.1} | {} | {} | {} | {} | {} | {} | {stalls} | {} | {} |\n",
            ts.len(),
            done.len(),
            attempts as f64 / n,
            costs.iter().sum::<f64>() / n,
            fmt(median(costs), 2),
            fmt(median(minutes), 0),
            reviews.len(),
            owner as f64 / n,
            fmt(share(|b| b.process), 0),
            fmt(share(|b| b.evidence), 0),
            fmt(share(|b| b.verify), 0),
            fmt(share(|b| b.task), 0),
            fmt(share(|b| b.explore), 0),
            fmt(calls, 0),
            fmt(median(prefixes), 0),
            fmt(mean_cached, 0),
        ));
    }
    if eval.is_some() {
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

/// One row per (evalName, variant): the same task under each arm, so arms
/// compare on the same work and not on averages. Means when a task ran more
/// than once per arm (`--repeat`).
fn per_task_table(tasks: &[Task], eval: Option<&str>) -> String {
    let mut rows: BTreeMap<(String, String), Vec<&Task>> = BTreeMap::new();
    for t in tasks {
        if t.variant.is_none() || t.eval_set.as_deref() != eval || eval.is_none() {
            continue;
        }
        let name = t.eval_name.clone().unwrap_or_else(|| "-".into());
        let variant = serde_json::to_string(&t.variant).unwrap_or_default();
        rows.entry((name, variant)).or_default().push(t);
    }
    let mut out = String::from(
        "\n| task | variant | runs | done | $ | attempts |\n|---|---|---|---|---|---|\n",
    );
    for ((name, variant), ts) in rows {
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
            "| {name} | `{variant}` | {} | {done} | {:.2} | {:.1} |\n",
            ts.len(),
            cost / n,
            attempts as f64 / n,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RetryMode, Variant};

    fn report(tasks: &[Task]) -> String {
        report_with(tasks, None, &|_, _| None)
    }

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
        let variant = Variant::default();
        let mut a = task(variant.clone(), TaskStatus::Done, 1.0, &[]);
        a.attempts = vec![
            attempt("plan", Some(90_000), Some(90_000)),
            attempt("implement", Some(20_000), Some(1_000)),
            attempt("implement", None, Some(3_000)),
        ];
        let mut b = task(variant, TaskStatus::Done, 1.0, &[]);
        b.attempts = vec![
            attempt("implement", Some(30_000), Some(5_000)),
            attempt("implement", Some(22_000), None),
        ];
        let out = report(&[a, b]);
        let row = out.lines().nth(2).unwrap();
        // Median of 20k, 30k, 22k; mean cached of 1k, 3k, 5k.
        assert!(row.ends_with("| 22000 | 3000 |"), "{row}");
        assert!(out
            .lines()
            .next()
            .unwrap()
            .ends_with("| median prefix tokens | cache-read tokens/attempt |"));
    }

    #[test]
    fn different_variants_each_get_their_own_row() {
        let lean_output = Variant {
            lean_output: true,
            ..Variant::default()
        };
        let mut a = task(lean_output, TaskStatus::Done, 1.0, &[]);
        a.attempts = vec![attempt("implement", Some(15_000), Some(2_000))];
        let mut b = task(Variant::default(), TaskStatus::Done, 1.0, &[]);
        b.attempts = vec![attempt("implement", Some(30_000), Some(5_000))];

        let out = report(&[a, b]);
        let rows: Vec<&str> = out.lines().skip(2).filter(|l| l.starts_with('|')).collect();
        assert_eq!(rows.len(), 2, "{out}");
        let lean_output_row = rows
            .iter()
            .find(|r| r.contains("\"leanOutput\":true"))
            .unwrap();
        assert!(
            lean_output_row.ends_with("| 15000 | 2000 |"),
            "{lean_output_row}"
        );
        assert!(rows.iter().any(|r| !r.contains("\"leanOutput\":true")));
    }

    fn ev(parent: Option<&str>, name: &str, input: Value) -> Value {
        serde_json::json!({
            "type": "assistant",
            "parent_tool_use_id": parent,
            "message": {"content": [
                {"type": "text", "text": "thinking"},
                {"type": "tool_use", "id": "x", "name": name, "input": input}
            ]}
        })
    }

    #[test]
    fn breakdown_files_one_call_of_each_kind_and_ignores_subagent_calls() {
        let changed = vec!["src/a.ts".to_string()];
        let events = vec![
            ev(
                None,
                "Edit",
                serde_json::json!({"file_path": "/w/docs/LESSONS.md"}),
            ),
            ev(
                None,
                "Bash",
                serde_json::json!({"command": "node shot.mjs artifacts/x.png"}),
            ),
            ev(
                None,
                "Bash",
                serde_json::json!({"command": "cargo clippy --all-targets"}),
            ),
            ev(
                None,
                "Edit",
                serde_json::json!({"file_path": "/w/src/a.ts"}),
            ),
            ev(
                None,
                "Read",
                serde_json::json!({"file_path": "/w/src/other.ts"}),
            ),
            ev(
                Some("toolu_1"),
                "Bash",
                serde_json::json!({"command": "cargo test"}),
            ),
            serde_json::json!({"type": "user", "message": {"content": []}}),
        ];
        let rules = WorkBuckets {
            process: vec!["docs/LESSONS.md".into()],
            evidence: vec![],
        };
        let b = work_breakdown(&events, &changed, &[], &rules);
        assert_eq!(
            b,
            Breakdown {
                process: 1,
                evidence: 1,
                verify: 1,
                task: 1,
                explore: 1
            }
        );
    }

    #[test]
    fn breakdown_uses_generic_rules_repo_patterns_and_the_task_s_own_verify() {
        let changed = vec!["src/a.ts".to_string()];
        let events = vec![
            ev(
                None,
                "Bash",
                serde_json::json!({"command": "sed -n 1,20p src/a.ts"}),
            ),
            ev(
                None,
                "Grep",
                serde_json::json!({"pattern": "x", "path": "src/a.ts"}),
            ),
            ev(
                None,
                "Bash",
                serde_json::json!({"command": "node tools/shoot.mjs"}),
            ),
            ev(
                None,
                "Bash",
                serde_json::json!({"command": "./scripts/check.sh --fast"}),
            ),
            ev(None, "Bash", serde_json::json!({"command": "npm run ci"})),
            ev(
                None,
                "Write",
                serde_json::json!({"file_path": "CHANGELOG.md"}),
            ),
        ];
        // Without repo patterns only generic rules apply: the changelog and
        // the custom screenshot script are exploration.
        let none = work_breakdown(&events, &changed, &[], &WorkBuckets::default());
        assert_eq!(
            none,
            Breakdown {
                process: 0,
                evidence: 0,
                verify: 1,
                task: 2,
                explore: 3
            }
        );
        let rules = WorkBuckets {
            process: vec!["CHANGELOG.md".into()],
            evidence: vec!["tools/shoot.mjs".into()],
        };
        let verify = vec!["./scripts/check.sh".to_string()];
        let b = work_breakdown(&events, &changed, &verify, &rules);
        assert_eq!(
            b,
            Breakdown {
                process: 1,
                evidence: 1,
                verify: 2,
                task: 2,
                explore: 0
            }
        );
    }

    #[test]
    fn eval_report_keeps_only_that_set_and_adds_a_row_per_task_and_variant() {
        let fresh = Variant {
            retry_mode: RetryMode::Fresh,
            ..Variant::default()
        };
        let mk = |v: Variant, set: Option<&str>, name: &str, cost: f64| {
            let mut t = task(v, TaskStatus::Done, cost, &[]);
            t.eval_set = set.map(String::from);
            t.eval_name = Some(name.into());
            t.attempts = vec![attempt("implement", None, None)];
            t
        };
        let tasks = vec![
            mk(Variant::default(), Some("set-1"), "alpha", 1.0),
            mk(fresh.clone(), Some("set-1"), "alpha", 2.0),
            mk(Variant::default(), Some("set-1"), "beta", 3.0),
            mk(Variant::default(), Some("other"), "gamma", 9.0),
            mk(Variant::default(), None, "delta", 9.0),
        ];
        let out = report_with(&tasks, Some("set-1"), &|_, _| {
            Some(Breakdown {
                process: 1,
                evidence: 0,
                verify: 1,
                task: 2,
                explore: 0,
            })
        });
        assert!(!out.contains("gamma") && !out.contains("delta"), "{out}");
        let per_task: Vec<&str> = out
            .lines()
            .skip_while(|l| !l.starts_with("| task |"))
            .skip(2)
            .filter(|l| l.starts_with('|'))
            .collect();
        assert_eq!(per_task.len(), 3, "{out}");
        assert!(
            per_task[0].starts_with("| alpha | `{")
                && per_task[0].contains("| 1 | 1 | 2.00 | 1.0 |"),
            "{out}"
        );
        assert!(per_task
            .iter()
            .any(|r| r.starts_with("| beta |") && r.contains("| 3.00 |")));
        // Variant rows: 2 arms, shares 25/0/25/50/0 and 4 calls.
        let row = out.lines().nth(2).unwrap();
        assert!(row.contains("| 25 | 0 | 25 | 50 | 0 | 4 |"), "{row}");
        // Without --eval the per-task table is absent.
        assert!(!report(&tasks).contains("| task | variant |"));
    }

    #[test]
    fn tasks_without_eval_fields_still_load() {
        let t = task(Variant::default(), TaskStatus::Done, 1.0, &[]);
        assert!(t.eval_set.is_none() && t.eval_name.is_none());
        let json = serde_json::to_value(&t).unwrap();
        assert!(json.get("evalSet").is_none());
    }
}
