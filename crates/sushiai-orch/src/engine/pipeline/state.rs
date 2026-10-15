use super::Spent;
use crate::task::{AttemptStatus, Stage, Task, TaskStatus, Verdict};
use std::collections::HashMap;

/// Where one stage of `task` stands, from recorded facts only.
pub(super) fn task_state(
    id: &str,
    enabled: bool,
    task: &Task,
    spent: &HashMap<&'static str, Spent>,
) -> &'static str {
    let seen = |s: &str| spent.get(s).is_some_and(|x| x.seen);
    if !enabled && id != "brief" && id != "question" {
        return "skipped";
    }
    match id {
        "brief" => {
            if task.status == TaskStatus::Drafting {
                "pending"
            } else {
                "done"
            }
        }
        "plan" => attempt_state(
            task,
            Stage::Plan,
            if task.request.is_none() {
                "skipped"
            } else {
                "pending"
            },
        ),
        "brief-check" => {
            if task.brief_check.done {
                "done"
            } else {
                "pending"
            }
        }
        "base-checks" => {
            if task.brief_check.feasibility_done {
                "done"
            } else {
                "pending"
            }
        }
        "implement" => attempt_state(task, Stage::Implement, "pending"),
        "checks" => check_state(task),
        "held-out" => held_out_state(task),
        "review" => review_state(task, seen("review")),
        "judge" => {
            if !task.brief_check.judged.is_empty() {
                "done"
            } else {
                "pending"
            }
        }
        "land" => {
            if task.landed_sha.is_some() {
                "done"
            } else if task.status == TaskStatus::Landing {
                "running"
            } else {
                "pending"
            }
        }
        "advisor" => {
            if seen("advisor") {
                "done"
            } else {
                "pending"
            }
        }
        _ => {
            if task.question.is_some() {
                "running"
            } else if !task.question_history.is_empty() {
                "done"
            } else {
                "pending"
            }
        }
    }
}

fn attempt_state(task: &Task, stage: Stage, absent: &'static str) -> &'static str {
    let mut attempts = task
        .attempts
        .iter()
        .filter(|attempt| attempt.stage == stage)
        .peekable();
    if attempts
        .clone()
        .any(|attempt| attempt.status == AttemptStatus::Running)
    {
        "running"
    } else if attempts
        .clone()
        .any(|attempt| attempt.status == AttemptStatus::Passed)
    {
        "done"
    } else if attempts.peek().is_some() {
        if matches!(task.status, TaskStatus::Failed | TaskStatus::Stopped) {
            "failed"
        } else {
            "running"
        }
    } else {
        absent
    }
}

fn check_state(task: &Task) -> &'static str {
    match task.attempts.iter().rev().find(|attempt| {
        attempt
            .verify
            .iter()
            .any(|result| !crate::verify::evidence::is_held_out(result))
    }) {
        None => "pending",
        Some(attempt) if attempt.status == AttemptStatus::Running => "running",
        Some(attempt) => {
            if attempt.verify.iter().any(|result| {
                !crate::verify::evidence::is_held_out(result) && result.code != Some(0)
            }) {
                "failed"
            } else {
                "done"
            }
        }
    }
}

fn held_out_state(task: &Task) -> &'static str {
    let result = task.attempts.iter().rev().find_map(|attempt| {
        attempt
            .verify
            .iter()
            .rev()
            .find(|result| crate::verify::evidence::is_held_out(result))
            .map(|result| result.code == Some(0))
    });
    match result {
        Some(true) => "done",
        Some(false) => "failed",
        None => "pending",
    }
}

fn review_state(task: &Task, seen: bool) -> &'static str {
    match task
        .attempts
        .iter()
        .rev()
        .find_map(|attempt| attempt.review.as_ref())
        .map(|review| review.verdict)
    {
        Some(Verdict::Pass) => "done",
        Some(Verdict::Fail) => "failed",
        None if seen => "running",
        None => "pending",
    }
}
