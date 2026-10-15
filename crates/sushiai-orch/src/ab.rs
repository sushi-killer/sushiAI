//! `sushiai orch ab --data <dir>`: every task grouped by the experiment variant it
//! ran with, one row per variant, read straight from the task store.
//! `--eval <set>` limits it to one eval set's tasks and adds a per-task table.

use crate::policy::settings::WorkBuckets;
use crate::store::Store;
use crate::task::{Attempt, Task};
use serde_json::Value;
use std::path::PathBuf;

pub fn run(args: &[String]) -> i32 {
    let data = args
        .windows(2)
        .find(|w| w[0] == "--data")
        .map(|w| PathBuf::from(&w[1]));
    let Some(data) = data else {
        eprintln!("sushiai orch ab: --data <dir> is required");
        return 2;
    };
    if !data.join("tasks").is_dir() {
        eprintln!("sushiai orch ab: no task store at {}", data.display());
        return 1;
    }
    let eval = args
        .windows(2)
        .find(|w| w[0] == "--eval")
        .map(|w| w[1].clone());
    let store = match Store::new(&data) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("sushiai orch ab: {e}");
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
            print!(
                "{}",
                report::report_with(&tasks, eval.as_deref(), &breakdown)
            );
            0
        }
        Err(e) => {
            eprintln!("sushiai orch ab: {e}");
            1
        }
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bucket {
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
pub(crate) fn classify_call(
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

/// The top-level tool calls in `events.jsonl` order as (index of the event,
/// tool name, input); a subagent's calls carry `parent_tool_use_id` and are
/// skipped. This is the one definition of "a tool call" for the breakdown
/// and for evolution signals.
pub(crate) fn top_level_calls(events: &[Value]) -> impl Iterator<Item = (usize, &str, &Value)> {
    events.iter().enumerate().flat_map(|(i, ev)| {
        let blocks = if ev.get("type").and_then(Value::as_str) != Some("assistant")
            || ev.get("parent_tool_use_id").is_some_and(|p| !p.is_null())
        {
            None
        } else {
            ev.pointer("/message/content").and_then(Value::as_array)
        };
        blocks
            .into_iter()
            .flatten()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use"))
            .map(move |b| {
                let name = b.get("name").and_then(Value::as_str).unwrap_or("");
                (i, name, b.get("input").unwrap_or(&Value::Null))
            })
    })
}

const EDIT_TOOLS: [&str; 5] = ["Edit", "Write", "MultiEdit", "NotebookEdit", "apply_patch"];

/// Whether one streamed stdout line shows the agent editing a file: a
/// top-level Claude edit `tool_use`, or a Codex `file_change` item.
pub(crate) fn is_edit_event(line: &Value) -> bool {
    if top_level_calls(std::slice::from_ref(line)).any(|(_, name, _)| EDIT_TOOLS.contains(&name)) {
        return true;
    }
    line.get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t.starts_with("item."))
        && line.pointer("/item/type").and_then(Value::as_str) == Some("file_change")
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
    for (_, name, input) in top_level_calls(events) {
        match classify_call(name, input, changed, verify, rules) {
            Bucket::Process => b.process += 1,
            Bucket::Evidence => b.evidence += 1,
            Bucket::Verify => b.verify += 1,
            Bucket::Task => b.task += 1,
            Bucket::Explore => b.explore += 1,
        }
    }
    b
}

pub mod report;
