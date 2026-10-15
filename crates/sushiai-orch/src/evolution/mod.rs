//! The evolution loop, part one: when a task reaches `done` or `failed`,
//! record the signals it left behind (`detect`) in `<data>/evolution/`.
//! `propose` clusters them, starts the read-only proposer runs, gates and
//! stores their proposals and measures the adopted ones.

#[cfg(test)]
use crate::task::VerifyOutcome;

use crate::clock::now_ms;
use crate::engine::messages;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::store::Store;
use crate::task::Stage;
use std::path::Path;
use std::path::PathBuf;

use crate::task::{Task, TaskStatus};
use serde::{Deserialize, Serialize};
use std::io::Write;

mod detect;
mod propose;

pub use detect::{detect, AttemptEvents};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Loop,
    Discovery,
    ThrowawayScript,
    VerifySignature,
    ReviewFinding,
    OwnerQuestion,
    ProcessRead,
    Graph,
    RepeatedReview,
    Preexisting,
}

/// Where the evidence for a signal is: a file relative to the data dir and
/// a 1-based inclusive line range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcerptRef {
    pub file: String,
    pub from_line: usize,
    pub to_line: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signal {
    pub kind: SignalKind,
    pub task_id: String,
    pub repo: String,
    /// 0 for a signal about the whole task.
    #[serde(default)]
    pub attempt: u32,
    pub detail: String,
    pub wasted_calls: u32,
    pub wasted_usd: f64,
    pub excerpt_ref: ExcerptRef,
}

fn evolution_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("evolution")
}

fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}

fn already_detected(dir: &Path, task_id: &str) -> bool {
    std::fs::read_to_string(dir.join("detected.jsonl"))
        .map(|text| text.lines().any(|l| l.trim().trim_matches('"') == task_id))
        .unwrap_or(false)
}

/// One `events.jsonl` line per element; a line that does not parse is `Null`
/// so an index plus one stays the file's line number.
fn load_events(store: &Store, task_id: &str, n: u32) -> Option<Vec<serde_json::Value>> {
    let text = std::fs::read_to_string(store.run_dir(task_id, n).join("events.jsonl")).ok()?;
    Some(
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap_or(serde_json::Value::Null))
            .collect(),
    )
}

/// The commands a failed task could not get past: non-zero verify outcomes
/// and the command a parent's final check tripped on. Each appears once.
fn failing_commands(task: &Task) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |c: &str| {
        if !c.is_empty() && !out.iter().any(|o| o == c) {
            out.push(c.to_string());
        }
    };
    for v in task.attempts.iter().flat_map(|a| &a.verify) {
        if matches!(v.code, Some(c) if c != 0) {
            add(&v.command);
        }
    }
    for command in &task.loop_state.final_check_failures {
        add(command);
    }
    out
}

impl App {
    /// Called once from `finish_task_loop`; the work runs in the background
    /// and never holds up the loop.
    pub(super) fn record_evolution_signals(&self, task_id: &str) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let app = self.arc();
        let task_id = task_id.to_string();
        runtime.spawn(async move { app.detect_evolution_signals(&task_id).await });
    }

    pub(super) async fn detect_evolution_signals(&self, task_id: &str) {
        // Held across check and append: two calls for one task record it once.
        let _guard = self.evolution_lock.lock().await;
        let Ok(Some(task)) = self.store.load_task(task_id) else {
            return;
        };
        if !matches!(task.status, TaskStatus::Done | TaskStatus::Failed) || task.eval_set.is_some()
        {
            return;
        }
        let dir = evolution_dir(&self.data_dir);
        if already_detected(&dir, &task.id) {
            return;
        }
        let settings = self.settings.read().unwrap().clone();
        let mut base_failures = Vec::new();
        if task.status == TaskStatus::Failed {
            let run_dir = self.store.task_dir(&task.id).join("runs").join("evolution");
            let _ = std::fs::create_dir_all(&run_dir);
            let cancel = CancelToken::new();
            for cmd in failing_commands(&task) {
                let check = crate::engine::verification::runner::run_eval_check(
                    crate::engine::verification::runner::CheckRunner::from_context(
                        crate::engine::verification::context::CheckContext::new(
                            self,
                            Some(task_id),
                        ),
                        &run_dir.join("eval-check"),
                        self.task_sandbox(&task.repo, &run_dir.join("eval-check"), &run_dir)
                            .as_ref(),
                        &task.base_sha,
                        &cancel,
                    ),
                    Path::new(&task.repo),
                    &cmd,
                )
                .await;
                if matches!(check.code, Some(c) if c != 0)
                    && !crate::verify::load::timed_out_under_load(&check)
                {
                    base_failures.push(cmd);
                }
            }
        }
        let store_dir = self.data_dir.clone();
        let repo_tasks = self.repo_tasks(&task.repo);
        let detected = {
            let (task, settings) = (task.clone(), settings.clone());
            tokio::task::spawn_blocking(move || {
                let store = Store::new(&store_dir).ok()?;
                let mut events = AttemptEvents::new();
                for t in repo_tasks.iter().filter(|t| t.eval_set.is_none()) {
                    for a in t.attempts.iter().filter(|a| a.stage == Stage::Implement) {
                        if let Some(evs) = load_events(&store, &t.id, a.n) {
                            events.insert((t.id.clone(), a.n), evs);
                        }
                    }
                }
                let messages = messages::load(&store_dir);
                Some(detect(
                    &task,
                    &repo_tasks,
                    &events,
                    &messages,
                    &settings,
                    &base_failures,
                ))
            })
            .await
        };
        let Ok(Some(signals)) = detected else {
            return;
        };
        for s in &signals {
            let Ok(mut value) = serde_json::to_value(s) else {
                continue;
            };
            // Every stored signal carries its time, so "latest" and age mean something.
            value["ts"] = serde_json::json!(now_ms());
            let Ok(line) = serde_json::to_string(&value) else {
                continue;
            };
            if append_line(&dir.join("signals.jsonl"), &line).is_err() {
                return;
            }
        }
        let _ = append_line(
            &dir.join("detected.jsonl"),
            &serde_json::Value::String(task.id.clone()).to_string(),
        );
    }
}

/// An answered question for tests; the owner gave `answer`.
#[cfg(test)]
pub(crate) fn owner_answer(question: &str, answer: &str) -> crate::task::AnsweredQuestion {
    crate::task::AnsweredQuestion {
        question: question.into(),
        options: Vec::new(),
        kind: crate::task::QuestionKind::AgentQuestion,
        asked_by: None,
        asked_at: None,
        answer: answer.into(),
        answered_at: 1,
        answered_by: crate::task::AnsweredBy::Owner,
        failure_signature: None,
        commands: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{task_with_status, test_app};
    use super::*;

    fn lines(app: &App, file: &str) -> Vec<String> {
        std::fs::read_to_string(evolution_dir(&app.data_dir).join(file))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    #[tokio::test]
    async fn evolution_second_call_appends_nothing() {
        let (app, _dir) = test_app();
        let mut t = task_with_status(TaskStatus::Done);
        t.fixture_data().question_history = vec![owner_answer("which one?", "first")];
        app.store.save_task(&t).unwrap();
        app.detect_evolution_signals(&t.id).await;
        let first = lines(&app, "signals.jsonl");
        assert_eq!(first.len(), 1);
        let signal: Signal = serde_json::from_str(&first[0]).unwrap();
        assert_eq!(signal.kind, SignalKind::OwnerQuestion);
        assert_eq!(lines(&app, "detected.jsonl").len(), 1);
        app.detect_evolution_signals(&t.id).await;
        assert_eq!(lines(&app, "signals.jsonl"), first);
        assert_eq!(lines(&app, "detected.jsonl").len(), 1);
    }

    #[tokio::test]
    async fn evolution_every_recorded_signal_carries_a_timestamp() {
        let (app, _dir) = test_app();
        let mut t = task_with_status(TaskStatus::Done);
        t.fixture_data().question_history = vec![owner_answer("which one?", "first")];
        app.store.save_task(&t).unwrap();
        let before = now_ms();
        app.detect_evolution_signals(&t.id).await;
        let line = &lines(&app, "signals.jsonl")[0];
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(v["ts"].as_i64().unwrap() >= before);
    }

    #[tokio::test]
    async fn evolution_task_without_signals_is_still_recorded_once() {
        let (app, _dir) = test_app();
        let t = task_with_status(TaskStatus::Done);
        app.store.save_task(&t).unwrap();
        app.detect_evolution_signals(&t.id).await;
        assert!(lines(&app, "signals.jsonl").is_empty());
        assert_eq!(lines(&app, "detected.jsonl").len(), 1);
    }

    #[tokio::test]
    async fn evolution_eval_and_unfinished_tasks_are_skipped() {
        let (app, _dir) = test_app();
        let mut eval = task_with_status(TaskStatus::Done);
        eval.fixture_data().eval_set = Some("set".into());
        eval.fixture_data().question_history = vec![owner_answer("q", "a")];
        let mut running = task_with_status(TaskStatus::Running);
        running.fixture_data().question_history = vec![owner_answer("q", "a")];
        app.store.save_task(&eval).unwrap();
        app.store.save_task(&running).unwrap();
        app.detect_evolution_signals(&eval.id).await;
        app.detect_evolution_signals(&running.id).await;
        assert!(lines(&app, "signals.jsonl").is_empty());
        assert!(lines(&app, "detected.jsonl").is_empty());
    }

    #[test]
    fn evolution_failing_commands_are_distinct_and_include_parent_check() {
        let mut t = task_with_status(TaskStatus::Failed);
        let out = |command: &str, code| VerifyOutcome {
            command: command.into(),
            code,
            tail: String::new(),
            ms: 0,
            ..Default::default()
        };
        let mut a = super::super::test_support::attempt_with_failure(1, "s");
        a.verify = vec![
            out("npm test", Some(1)),
            out("cargo fmt", Some(0)),
            out("slow", None),
        ];
        let mut b = a.clone();
        b.n = 2;
        t.fixture_data().attempts = vec![a, b];
        t.fixture_data().decisions = vec!["reworded".into()];
        t.fixture_data().loop_state.final_check_failures = vec!["npm run ci".into()];
        assert_eq!(failing_commands(&t), vec!["npm test", "npm run ci"]);
    }
}

pub mod proposal;
