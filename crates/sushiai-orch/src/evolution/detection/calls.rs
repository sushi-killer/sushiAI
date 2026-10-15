use super::Detection;
use crate::ab::{classify_call, top_level_calls, Bucket};
use crate::policy::settings::{Settings, WorkBuckets};
use crate::task::{Attempt, Task};
use serde_json::Value;

pub(super) struct Call<'a> {
    pub(super) line: usize,
    pub(super) bucket: Bucket,
    pub(super) name: &'a str,
    pub(super) input: &'a Value,
}

impl<'a> Detection<'a> {
    pub(super) fn attempt_events(&self, n: u32) -> &'a [Value] {
        self.events
            .get(&(self.task.id.clone(), n))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(super) fn calls(&self, a: &Attempt) -> Vec<Call<'a>> {
        calls_of(
            self.task,
            a,
            self.attempt_events(a.n),
            &bucket_rules(self.settings, &self.task.repo),
        )
    }

    pub(super) fn total_calls(&self, a: &Attempt) -> u32 {
        self.calls(a).len() as u32
    }
}

/// The work-bucket rules for one repo: the global evidence patterns and the
/// process files configured for that repo alone.
pub(super) fn bucket_rules(settings: &Settings, repo: &str) -> WorkBuckets {
    WorkBuckets {
        process: settings
            .evolution
            .process_files
            .get(repo)
            .cloned()
            .unwrap_or_default(),
        evidence: settings.work_buckets.evidence.clone(),
    }
}

pub(super) fn calls_of<'a>(
    task: &Task,
    a: &Attempt,
    events: &'a [Value],
    rules: &WorkBuckets,
) -> Vec<Call<'a>> {
    let verify: Vec<String> = task
        .verify
        .iter()
        .chain(&task.final_verify)
        .cloned()
        .collect();
    top_level_calls(events)
        .map(|(i, name, input)| Call {
            line: i + 1,
            bucket: classify_call(name, input, &a.changed_files, &verify, rules),
            name,
            input,
        })
        .collect()
}

pub(super) fn explore_of<'a, 'b>(calls: &'b [Call<'a>]) -> Vec<usize> {
    calls
        .iter()
        .filter(|c| c.bucket == Bucket::Explore)
        .map(|c| c.line)
        .collect()
}

/// Files a Write/Edit made that a later Bash `rm` removed, each with the
/// event lines of every call whose input names it. A file the attempt kept
/// in its diff (`changed`) is source, not a throwaway.
pub(super) fn throwaways(calls: &[Call], changed: &[String]) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, c) in calls.iter().enumerate() {
        if !matches!(c.name, "Write" | "Edit") {
            continue;
        }
        let Some(path) = c
            .input
            .get("file_path")
            .or_else(|| c.input.get("path"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if out.iter().any(|(p, _)| p == path)
            || changed
                .iter()
                .any(|f| !f.is_empty() && (path == f || path.ends_with(&format!("/{f}"))))
        {
            continue;
        }
        let base = path.rsplit('/').next().unwrap_or(path);
        let removed = calls[i + 1..].iter().any(|later| {
            later.name == "Bash"
                && later
                    .input
                    .get("command")
                    .and_then(Value::as_str)
                    .is_some_and(|cmd| removes(cmd, path, base))
        });
        if removed {
            let lines = calls
                .iter()
                .filter(|c| {
                    let text = c.input.to_string();
                    text.contains(path) || text.contains(base)
                })
                .map(|c| c.line)
                .collect();
            out.push((path.to_string(), lines));
        }
    }
    out
}

/// Whether `cmd` runs `rm` on the file: only the operands of an `rm`
/// command count, not other words of the command line and not heredoc text.
fn removes(cmd: &str, path: &str, base: &str) -> bool {
    let mut heredoc_end: Option<String> = None;
    for line in cmd.lines() {
        if let Some(end) = &heredoc_end {
            if line.trim() == end {
                heredoc_end = None;
            }
            continue;
        }
        if let Some((_, rest)) = line.split_once("<<") {
            let word = rest.trim_start_matches('-').trim();
            let word = word.split_whitespace().next().unwrap_or("");
            let word = word.trim_matches(|c| c == '"' || c == '\'');
            if !word.is_empty() {
                heredoc_end = Some(word.to_string());
            }
        }
        for segment in line.split([';', '&', '|']) {
            let words: Vec<&str> = segment
                .split_whitespace()
                .map(|t| t.trim_matches(|c| c == '"' || c == '\''))
                .collect();
            let start = words
                .iter()
                .position(|w| !matches!(*w, "sudo" | "command" | "time"))
                .unwrap_or(words.len());
            let is_rm = words
                .get(start)
                .is_some_and(|w| *w == "rm" || w.ends_with("/rm"));
            if !is_rm {
                continue;
            }
            let named = words[start + 1..].iter().any(|t| {
                let t = t.strip_prefix("./").unwrap_or(t);
                !t.is_empty()
                    && !t.starts_with('-')
                    && (t == path
                        || t == base
                        || path.ends_with(&format!("/{t}")) && t.contains('/'))
            });
            if named {
                return true;
            }
        }
    }
    false
}
