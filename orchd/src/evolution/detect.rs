//! Pure signal detection: what in a finished task points at a rule, skill or
//! prompt that could be better. No file, git or process access; the caller
//! hands over everything it reads.

use super::{ExcerptRef, Signal, SignalKind};
use crate::ab::{classify_call, top_level_calls, Bucket};
use crate::engine::decision::normalize_signature_line;
use crate::model::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// Events of every implement attempt, keyed by (task id, attempt n); one
/// element per line of `events.jsonl` (`Null` for a line that did not parse)
/// so an index plus one is the file's line number.
pub type AttemptEvents = HashMap<(String, u32), Vec<Value>>;

struct Call<'a> {
    line: usize,
    bucket: Bucket,
    name: &'a str,
    input: &'a Value,
}

struct Ctx<'a> {
    task: &'a Task,
    settings: &'a Settings,
    events: &'a AttemptEvents,
    out: Vec<Signal>,
}

pub fn detect(
    task: &Task,
    repo_tasks: &[Task],
    events: &AttemptEvents,
    messages: &[Message],
    settings: &Settings,
    base_failures: &[String],
) -> Vec<Signal> {
    let mut cx = Ctx {
        task,
        settings,
        events,
        out: Vec::new(),
    };
    let earlier: Vec<&Task> = repo_tasks
        .iter()
        .filter(|t| {
            t.id != task.id
                && t.repo == task.repo
                && t.eval_set.is_none()
                && t.created_at < task.created_at
        })
        .collect();
    cx.per_attempt(&earlier);
    cx.discovery(&earlier);
    cx.repeated_review();
    cx.owner_questions();
    cx.graph(repo_tasks, messages);
    for cmd in base_failures {
        cx.push(
            SignalKind::Preexisting,
            0,
            format!("`{cmd}` fails on the task's base commit too"),
            0,
            None,
        );
    }
    cx.out
}

impl<'a> Ctx<'a> {
    fn attempt_events(&self, n: u32) -> &'a [Value] {
        self.events
            .get(&(self.task.id.clone(), n))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn calls(&self, a: &Attempt) -> Vec<Call<'a>> {
        calls_of(
            self.task,
            a,
            self.attempt_events(a.n),
            &self.settings.work_buckets,
        )
    }

    fn excerpt(&self, n: u32, lines: Option<(usize, usize)>) -> ExcerptRef {
        let len = self.attempt_events(n).len();
        if n == 0 || len == 0 {
            return ExcerptRef {
                file: format!("tasks/{}/task.json", self.task.id),
                from_line: 1,
                to_line: 1,
            };
        }
        let (from, to) = lines.unwrap_or((1, len));
        ExcerptRef {
            file: format!("tasks/{}/runs/{n}/events.jsonl", self.task.id),
            from_line: from,
            to_line: to,
        }
    }

    fn push(
        &mut self,
        kind: SignalKind,
        attempt: u32,
        detail: String,
        wasted_calls: u32,
        lines: Option<(usize, usize)>,
    ) {
        let cost = self
            .task
            .attempts
            .iter()
            .find(|a| a.n == attempt)
            .and_then(|a| a.cost_usd);
        let total = self
            .task
            .attempts
            .iter()
            .find(|a| a.n == attempt)
            .map_or(0, |a| self.calls(a).len());
        let excerpt_ref = self.excerpt(attempt, lines);
        self.out.push(Signal {
            kind,
            task_id: self.task.id.clone(),
            repo: self.task.repo.clone(),
            attempt,
            detail,
            wasted_calls,
            wasted_usd: wasted_usd(wasted_calls, total, cost),
            excerpt_ref,
        });
    }

    fn total_calls(&self, a: &Attempt) -> u32 {
        self.calls(a).len() as u32
    }

    /// loop, throwaway_script, verify_signature, review_finding, process_read.
    fn per_attempt(&mut self, earlier: &[&Task]) {
        let task = self.task;
        let known_signatures: HashSet<&str> = earlier
            .iter()
            .flat_map(|t| &t.attempts)
            .filter_map(|a| a.failure.as_ref())
            .map(|f| f.signature.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        let known_findings: HashSet<String> = earlier
            .iter()
            .flat_map(|t| &t.attempts)
            .flat_map(findings)
            .collect();
        for a in &task.attempts {
            let total = self.total_calls(a);
            if let Some(f) = &a.failure {
                if f.kind == FailureKind::Loop {
                    self.push(
                        SignalKind::Loop,
                        a.n,
                        format!("attempt {} was stopped by the loop detector", a.n),
                        total,
                        None,
                    );
                }
                if known_signatures.contains(f.signature.as_str()) {
                    self.push(
                        SignalKind::VerifySignature,
                        a.n,
                        format!(
                            "failure `{}` already happened in an earlier task",
                            f.signature
                        ),
                        total,
                        None,
                    );
                }
            }
            let repeated: Vec<String> = findings(a)
                .into_iter()
                .filter(|f| known_findings.contains(f))
                .collect();
            if !repeated.is_empty() {
                self.push(
                    SignalKind::ReviewFinding,
                    a.n,
                    format!(
                        "review finding seen in an earlier task: {}",
                        repeated.join("; ")
                    ),
                    total,
                    None,
                );
            }
            if a.stage != Stage::Implement {
                continue;
            }
            let calls = self.calls(a);
            let process: Vec<&Call> = calls
                .iter()
                .filter(|c| c.bucket == Bucket::Process)
                .collect();
            if let (Some(first), Some(last)) = (process.first(), process.last()) {
                self.push(
                    SignalKind::ProcessRead,
                    a.n,
                    format!("{} tool call(s) went into process files", process.len()),
                    process.len() as u32,
                    Some((first.line, last.line)),
                );
            }
            for (path, matching) in throwaways(&calls) {
                self.push(
                    SignalKind::ThrowawayScript,
                    a.n,
                    format!("{path} was created and then deleted"),
                    matching.len() as u32,
                    matching.first().zip(matching.last()).map(|(f, l)| (*f, *l)),
                );
            }
        }
    }

    fn discovery(&mut self, earlier: &[&Task]) {
        let mut sample: Vec<f64> = Vec::new();
        let mut tasks_with_implement = 0u32;
        for t in earlier {
            let mut any = false;
            for a in t.attempts.iter().filter(|a| a.stage == Stage::Implement) {
                any = true;
                if let Some(evs) = self.events.get(&(t.id.clone(), a.n)) {
                    let calls = calls_of(t, a, evs, &self.settings.work_buckets);
                    if !calls.is_empty() {
                        sample.push(explore_of(&calls).len() as f64);
                    }
                }
            }
            tasks_with_implement += any as u32;
        }
        if tasks_with_implement < self.settings.evolution.min_tasks {
            return;
        }
        sample.sort_by(|a, b| a.total_cmp(b));
        let Some(median) = median_sorted(&sample) else {
            return;
        };
        for a in self
            .task
            .attempts
            .iter()
            .filter(|a| a.stage == Stage::Implement)
        {
            let calls = self.calls(a);
            let explore = explore_of(&calls);
            if (explore.len() as f64) > 2.0 * median {
                let wasted = (explore.len() as f64 - median) as u32;
                let lines = explore.first().zip(explore.last()).map(|(f, l)| (*f, *l));
                self.push(
                    SignalKind::Discovery,
                    a.n,
                    format!(
                        "{} explore calls, the repo median is {median}",
                        explore.len()
                    ),
                    wasted,
                    lines,
                );
            }
        }
    }

    fn repeated_review(&mut self) {
        for pair in self.task.attempts.windows(2) {
            let before: HashSet<String> = findings(&pair[0]).into_iter().collect();
            let same: Vec<String> = findings(&pair[1])
                .into_iter()
                .filter(|f| before.contains(f))
                .collect();
            if !same.is_empty() {
                let total = self.total_calls(&pair[1]);
                self.push(
                    SignalKind::RepeatedReview,
                    pair[1].n,
                    format!(
                        "review raised the same finding again on attempt {}: {}",
                        pair[1].n,
                        same.join("; ")
                    ),
                    total,
                    None,
                );
            }
        }
    }

    fn owner_questions(&mut self) {
        for d in self
            .task
            .decisions
            .iter()
            .filter(|d| d.starts_with("Owner:"))
        {
            self.push(SignalKind::OwnerQuestion, 0, d.clone(), 0, None);
        }
    }

    fn graph(&mut self, repo_tasks: &[Task], messages: &[Message]) {
        let task = self.task;
        let siblings: Vec<&Task> = match &task.parent {
            Some(p) => repo_tasks
                .iter()
                .filter(|t| t.id != task.id && t.parent.as_deref() == Some(p.as_str()))
                .collect(),
            None => Vec::new(),
        };
        if task.parent.is_some() {
            for a in &task.attempts {
                let Some(f) = &a.failure else { continue };
                if f.kind == FailureKind::Verify && f.detail.contains("These files conflict:") {
                    self.push(
                        SignalKind::Graph,
                        a.n,
                        "the subtask conflicted with files changed on its parent's branch"
                            .to_string(),
                        0,
                        None,
                    );
                }
            }
        }
        let children: Vec<&Task> = repo_tasks
            .iter()
            .filter(|t| t.parent.as_deref() == Some(task.id.as_str()))
            .collect();
        if task.status == TaskStatus::Failed
            && !children.is_empty()
            && children.iter().all(|c| c.status == TaskStatus::Done)
        {
            self.push(
                SignalKind::Graph,
                0,
                "every subtask is done but the parent failed".to_string(),
                0,
                None,
            );
        }
        let undeclared: Vec<&Task> = siblings
            .into_iter()
            .filter(|s| !task.depends_on.contains(&s.id))
            .collect();
        if undeclared.is_empty() {
            return;
        }
        let mut sources: Vec<(u32, String)> = Vec::new();
        for a in &task.attempts {
            if let Some(f) = a
                .failure
                .as_ref()
                .filter(|f| f.kind == FailureKind::Blocked)
            {
                sources.push((a.n, f.detail.clone()));
            }
        }
        if let Some(q) = &task.question {
            sources.push((0, q.text.clone()));
        }
        for m in messages.iter().filter(|m| m.from == task.id) {
            sources.push((0, m.text.clone()));
        }
        for s in undeclared {
            let hit = sources.iter().find(|(_, text)| {
                text.contains(s.id.as_str()) || (!s.title.is_empty() && text.contains(&s.title))
            });
            if let Some((n, _)) = hit {
                let n = *n;
                self.push(
                    SignalKind::Graph,
                    n,
                    format!(
                        "asked about sibling \"{}\" ({}) that it does not depend on",
                        s.title, s.id
                    ),
                    0,
                    None,
                );
            }
        }
    }
}

fn wasted_usd(wasted: u32, total: usize, cost: Option<f64>) -> f64 {
    match cost {
        Some(c) if total > 0 && wasted > 0 => wasted as f64 / total as f64 * c,
        _ => 0.0,
    }
}

fn median_sorted(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mid = xs.len() / 2;
    Some(if xs.len().is_multiple_of(2) {
        (xs[mid - 1] + xs[mid]) / 2.0
    } else {
        xs[mid]
    })
}

fn findings(a: &Attempt) -> Vec<String> {
    let mut out: Vec<String> = a
        .review
        .iter()
        .flat_map(|r| &r.findings)
        .map(|f| normalize_signature_line(f.trim()))
        .filter(|f| !f.is_empty())
        .collect();
    out.dedup();
    out
}

fn calls_of<'a>(
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

fn explore_of<'a, 'b>(calls: &'b [Call<'a>]) -> Vec<usize> {
    calls
        .iter()
        .filter(|c| c.bucket == Bucket::Explore)
        .map(|c| c.line)
        .collect()
}

/// Files a Write/Edit made that a later Bash `rm` removed, each with the
/// event lines of every call whose input names it.
fn throwaways(calls: &[Call]) -> Vec<(String, Vec<usize>)> {
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
        if out.iter().any(|(p, _)| p == path) {
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

fn removes(cmd: &str, path: &str, base: &str) -> bool {
    let tokens: Vec<&str> = cmd
        .split(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|'))
        .map(|t| t.trim_matches(|c| c == '"' || c == '\''))
        .collect();
    tokens.contains(&"rm")
        && tokens.iter().any(|t| {
            !t.is_empty()
                && (*t == path
                    || *t == base
                    || t.ends_with(&format!("/{base}"))
                    || path.ends_with(&format!("/{t}")) && t.contains('/'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{attempt_with_failure, task_with_status};
    use serde_json::json;

    fn task(id: &str, created: i64) -> Task {
        let mut t = task_with_status(TaskStatus::Done);
        t.id = id.into();
        t.created_at = created;
        t
    }

    fn call(name: &str, input: Value) -> Value {
        json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":name,"input":input}]}})
    }

    fn read(path: &str) -> Value {
        call("Read", json!({"file_path": path}))
    }

    fn run(task: &Task, others: &[Task], evs: &[(&str, u32, Vec<Value>)]) -> Vec<Signal> {
        let mut all = vec![task.clone()];
        all.extend(others.iter().cloned());
        let events: AttemptEvents = evs
            .iter()
            .map(|(id, n, v)| ((id.to_string(), *n), v.clone()))
            .collect();
        detect(task, &all, &events, &[], &Settings::default(), &[])
    }

    fn of_kind(signals: &[Signal], kind: SignalKind) -> Vec<&Signal> {
        signals.iter().filter(|s| s.kind == kind).collect()
    }

    fn failure(kind: FailureKind, detail: &str, signature: &str) -> Failure {
        Failure {
            kind,
            detail: detail.into(),
            signature: signature.into(),
        }
    }

    fn reviewed(mut a: Attempt, findings: &[&str]) -> Attempt {
        a.review = Some(ReviewResult {
            verdict: Verdict::Fail,
            findings: findings.iter().map(|f| f.to_string()).collect(),
            repeated: Vec::new(),
            severities: Vec::new(),
        });
        a
    }

    #[test]
    fn evolution_loop_signal_wastes_the_attempts_calls_and_cost() {
        let mut t = task("t", 10);
        let mut a = attempt_with_failure(1, "loop:x");
        a.failure = Some(failure(FailureKind::Loop, "same call", "loop:x"));
        a.cost_usd = Some(4.0);
        t.attempts = vec![a];
        let evs = vec![read("/a"), read("/b"), read("/c"), read("/d")];
        let s = run(&t, &[], &[("t", 1, evs)]);
        let l = of_kind(&s, SignalKind::Loop);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].attempt, l[0].wasted_calls), (1, 4));
        assert!((l[0].wasted_usd - 4.0).abs() < 1e-9);
        assert_eq!(l[0].excerpt_ref.file, "tasks/t/runs/1/events.jsonl");
        assert_eq!(
            (l[0].excerpt_ref.from_line, l[0].excerpt_ref.to_line),
            (1, 4)
        );
        let v = serde_json::to_value(l[0]).unwrap();
        assert_eq!(v["kind"], "loop");
        assert_eq!(v["taskId"], "t");
        assert_eq!(v["excerptRef"]["fromLine"], 1);
    }

    #[test]
    fn evolution_no_calls_or_no_cost_means_zero_usd() {
        let mut t = task("t", 10);
        let mut a = attempt_with_failure(1, "loop:x");
        a.failure = Some(failure(FailureKind::Loop, "d", "loop:x"));
        t.attempts = vec![a];
        let s = run(&t, &[], &[("t", 1, vec![read("/a")])]);
        assert_eq!(s[0].wasted_calls, 1);
        assert_eq!(s[0].wasted_usd, 0.0);
        let s = run(&t, &[], &[]);
        assert_eq!((s[0].wasted_calls, s[0].wasted_usd), (0, 0.0));
        assert_eq!(s[0].excerpt_ref.file, "tasks/t/task.json");
    }

    #[test]
    fn evolution_discovery_needs_history_and_twice_the_median() {
        let mut earlier = Vec::new();
        let mut evs: Vec<(&str, u32, Vec<Value>)> = Vec::new();
        for id in ["e1", "e2", "e3"] {
            let mut t = task(id, 1);
            t.attempts = vec![attempt_with_failure(1, "s")];
            earlier.push(t);
            evs.push((id, 1, vec![read("/x"), read("/y")]));
        }
        let mut t = task("t", 10);
        t.attempts = vec![attempt_with_failure(1, "s")];
        let many: Vec<Value> = (0..7).map(|i| read(&format!("/f{i}"))).collect();
        evs.push(("t", 1, many));
        let s = run(&t, &earlier, &evs);
        let d = of_kind(&s, SignalKind::Discovery);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].wasted_calls, 5);
        assert_eq!(
            (d[0].excerpt_ref.from_line, d[0].excerpt_ref.to_line),
            (1, 7)
        );
        // too little history
        let s = run(&t, &earlier[..2], &evs);
        assert!(of_kind(&s, SignalKind::Discovery).is_empty());
        // 4 explore calls is not more than 2x a median of 2
        evs.pop();
        evs.push(("t", 1, (0..4).map(|i| read(&format!("/f{i}"))).collect()));
        assert!(of_kind(&run(&t, &earlier, &evs), SignalKind::Discovery).is_empty());
    }

    #[test]
    fn evolution_throwaway_script_written_then_removed() {
        let mut t = task("t", 10);
        t.attempts = vec![attempt_with_failure(1, "s")];
        let evs = vec![
            call(
                "Write",
                json!({"file_path": "/wt/scratch/probe.py", "content": "x"}),
            ),
            call("Bash", json!({"command": "python /wt/scratch/probe.py"})),
            read("/other"),
            call("Bash", json!({"command": "rm -f probe.py"})),
        ];
        let s = run(&t, &[], &[("t", 1, evs)]);
        let x = of_kind(&s, SignalKind::ThrowawayScript);
        assert_eq!(x.len(), 1);
        assert_eq!(x[0].wasted_calls, 3);
        assert_eq!(
            (x[0].excerpt_ref.from_line, x[0].excerpt_ref.to_line),
            (1, 4)
        );
        // written and kept: nothing
        let kept = vec![call("Write", json!({"file_path": "/wt/a.rs"}))];
        assert!(of_kind(
            &run(&t, &[], &[("t", 1, kept)]),
            SignalKind::ThrowawayScript
        )
        .is_empty());
    }

    #[test]
    fn evolution_verify_signature_seen_in_earlier_task() {
        let mut old = task("old", 1);
        old.attempts = vec![attempt_with_failure(1, "verify:boom")];
        let mut newer = task("new", 20);
        newer.attempts = vec![
            attempt_with_failure(1, "verify:boom"),
            attempt_with_failure(2, "verify:other"),
        ];
        let s = run(&newer, &[old.clone()], &[]);
        let v = of_kind(&s, SignalKind::VerifySignature);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].attempt, 1);
        // a later task or another repo does not count
        old.created_at = 30;
        assert!(of_kind(
            &run(&newer, &[old.clone()], &[]),
            SignalKind::VerifySignature
        )
        .is_empty());
        old.created_at = 1;
        old.repo = "/elsewhere".into();
        assert!(of_kind(&run(&newer, &[old], &[]), SignalKind::VerifySignature).is_empty());
    }

    #[test]
    fn evolution_review_finding_matches_normalised_earlier_finding() {
        let mut old = task("old", 1);
        old.attempts = vec![reviewed(
            attempt_with_failure(1, "s"),
            &["missing test at /a/b.rs:12"],
        )];
        let mut newer = task("new", 20);
        newer.attempts = vec![reviewed(
            attempt_with_failure(1, "t"),
            &["missing test at /c/d.rs:99", "unrelated"],
        )];
        let s = run(&newer, &[old], &[]);
        let r = of_kind(&s, SignalKind::ReviewFinding);
        assert_eq!(r.len(), 1);
        assert!(r[0].detail.contains("missing test at"));
    }

    #[test]
    fn evolution_owner_question_one_per_owner_decision() {
        let mut t = task("t", 10);
        t.decisions = vec![
            "Owner: which db? -> sqlite".into(),
            "Jev: tier hard".into(),
            "Owner: dependency answered".into(),
        ];
        let s = run(&t, &[], &[]);
        let o = of_kind(&s, SignalKind::OwnerQuestion);
        assert_eq!(o.len(), 2);
        assert_eq!(o[0].attempt, 0);
        assert_eq!(o[0].excerpt_ref.file, "tasks/t/task.json");
        assert_eq!(o[0].wasted_calls, 0);
    }

    #[test]
    fn evolution_process_read_counts_matching_calls() {
        let mut t = task("t", 10);
        t.attempts = vec![attempt_with_failure(1, "s")];
        let mut settings = Settings::default();
        settings.work_buckets.process = vec!["docs/LESSONS.md".into()];
        let evs = vec![
            read("/wt/docs/LESSONS.md"),
            read("/wt/src/a.rs"),
            read("/wt/docs/LESSONS.md"),
        ];
        let events: AttemptEvents = [(("t".to_string(), 1), evs)].into();
        let s = detect(&t, std::slice::from_ref(&t), &events, &[], &settings, &[]);
        let p = of_kind(&s, SignalKind::ProcessRead);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].wasted_calls, 2);
        assert_eq!(
            (p[0].excerpt_ref.from_line, p[0].excerpt_ref.to_line),
            (1, 3)
        );
    }

    #[test]
    fn evolution_graph_conflict_failed_parent_and_sibling_question() {
        let mut parent = task("p", 1);
        parent.status = TaskStatus::Failed;
        let mut a = task("a", 2);
        a.parent = Some("p".into());
        a.title = "Add login".into();
        let mut b = task("b", 3);
        b.parent = Some("p".into());
        b.title = "Wire billing".into();
        let mut conflict = attempt_with_failure(1, "s");
        conflict.failure = Some(failure(
            FailureKind::Verify,
            "main moved. These files conflict: x.rs. Resolve",
            "s",
        ));
        b.attempts = vec![conflict];
        b.question = Some(Question {
            text: "Should I reuse what Add login built?".into(),
            options: vec![],
            kind: QuestionKind::AgentQuestion,
        });
        let repo = vec![parent.clone(), a.clone(), b.clone()];
        let run_for = |t: &Task, msgs: &[Message]| {
            detect(
                t,
                &repo,
                &AttemptEvents::new(),
                msgs,
                &Settings::default(),
                &[],
            )
        };
        // parent failed with all children done
        let mut done_a = a.clone();
        done_a.status = TaskStatus::Done;
        let mut done_b = b.clone();
        done_b.status = TaskStatus::Done;
        let s = detect(
            &parent,
            &[parent.clone(), done_a, done_b],
            &AttemptEvents::new(),
            &[],
            &Settings::default(),
            &[],
        );
        assert_eq!(of_kind(&s, SignalKind::Graph).len(), 1);
        // conflict + undeclared sibling in the question
        let s = run_for(&b, &[]);
        assert_eq!(of_kind(&s, SignalKind::Graph).len(), 2);
        // a declared dependency is not a graph problem
        let mut declared = b.clone();
        declared.depends_on = vec!["a".into()];
        declared.attempts.clear();
        assert!(of_kind(&run_for(&declared, &[]), SignalKind::Graph).is_empty());
        // a sent message naming the sibling's id counts
        let mut quiet = b.clone();
        quiet.attempts.clear();
        quiet.question = None;
        let msg = Message {
            id: "m".into(),
            repo: "/repo".into(),
            from: "b".into(),
            to: "a".into(),
            kind: MessageKind::Message,
            text: "hey, done?".into(),
            reply_to: None,
            ts: 0,
            delivered: false,
            delivered_at: None,
        };
        assert!(of_kind(
            &run_for(&quiet, std::slice::from_ref(&msg)),
            SignalKind::Graph
        )
        .is_empty());
        let msg = Message {
            text: "did Add login finish?".into(),
            ..msg
        };
        assert_eq!(
            of_kind(&run_for(&quiet, &[msg]), SignalKind::Graph).len(),
            1
        );
        // conflict outside a subtask is a moved base, not a sibling conflict
        let mut solo = b.clone();
        solo.parent = None;
        solo.question = None;
        assert!(of_kind(&run_for(&solo, &[]), SignalKind::Graph).is_empty());
    }

    #[test]
    fn evolution_repeated_review_on_consecutive_attempts() {
        let mut t = task("t", 10);
        t.attempts = vec![
            reviewed(attempt_with_failure(1, "a"), &["no test for x at line 4"]),
            reviewed(attempt_with_failure(2, "b"), &["no test for x at line 9"]),
            reviewed(attempt_with_failure(3, "c"), &["different"]),
        ];
        let s = run(&t, &[], &[]);
        let r = of_kind(&s, SignalKind::RepeatedReview);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].attempt, 2);
    }

    #[test]
    fn evolution_preexisting_one_signal_per_base_failure() {
        let t = task("t", 10);
        let s = detect(
            &t,
            std::slice::from_ref(&t),
            &AttemptEvents::new(),
            &[],
            &Settings::default(),
            &["npm test".to_string(), "cargo clippy".to_string()],
        );
        let p = of_kind(&s, SignalKind::Preexisting);
        assert_eq!(p.len(), 2);
        assert!(p[0].detail.contains("npm test"));
        assert_eq!((p[0].attempt, p[0].wasted_calls), (0, 0));
    }
}
