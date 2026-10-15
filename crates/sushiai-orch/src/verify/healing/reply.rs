use crate::prompts::clip::truncate_chars;
use crate::prompts::engine_text;
use crate::prompts::text::tail_chars;
use crate::task::attempt::Stage;
use crate::task::attempt::VerifyOutcome;
use crate::task::review::CriterionRuling;
use crate::task::review::ReviewResult;
use crate::task::state::Task;
use serde_json::json;

/// The most follow-ups a task keeps for its report.
pub(crate) const MAX_REASON_CHARS: usize = 300;

/// The outermost JSON object of a judge's reply, whatever surrounds it.
pub(crate) fn judge_json(text: &str) -> Option<serde_json::Value> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    serde_json::from_str(text.get(start..=end)?).ok()
}

pub(crate) fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

pub(crate) fn reason_of(v: &serde_json::Value) -> String {
    let r = str_field(v, "reason");
    if r.is_empty() {
        "no reason given".to_string()
    } else {
        truncate_chars(&r, MAX_REASON_CHARS)
    }
}

#[derive(Clone)]
pub(crate) enum Feasibility {
    /// Fails as it should.
    AsWritten,
    Rewrite {
        command: String,
        criterion: Option<(usize, String)>,
        reason: String,
    },
    Unrelated {
        reason: String,
    },
}

pub(crate) fn feasibility_prompt(task: &Task, failed: &VerifyOutcome) -> String {
    let criteria = indexed_criteria(task);
    format!(
        "{}\n\n{}\n",
        engine_text("healing_feasibility"),
        json!({
            "goal": task.goal,
            "criteria": criteria,
            "command": failed.command,
            "exitCode": failed.code,
            "output": tail_chars(failed.tail.trim(), 1500),
        })
    )
}

pub(crate) fn parse_feasibility(text: &str, criteria: usize) -> Option<Feasibility> {
    let v = judge_json(text)?;
    match str_field(&v, "verdict").to_ascii_lowercase().as_str() {
        "feasible" => Some(Feasibility::AsWritten),
        "unrelated" => Some(Feasibility::Unrelated {
            reason: reason_of(&v),
        }),
        "rewrite" => {
            let command = str_field(&v, "command");
            if command.is_empty() {
                return None;
            }
            let text = str_field(&v, "criterionText");
            let criterion = v
                .get("criterion")
                .and_then(|i| i.as_u64())
                .map(|i| i as usize)
                .filter(|i| *i < criteria && !text.is_empty())
                .map(|i| (i, text));
            Some(Feasibility::Rewrite {
                command,
                criterion,
                reason: reason_of(&v),
            })
        }
        _ => None,
    }
}

pub(crate) enum Ruling {
    CodeGap,
    Infeasible { criterion: String, reason: String },
}

/// A criterion this review marks unmet that an earlier attempt's review
/// marked unmet too and the judge has not ruled on yet: its index, the two
/// attempts and both reasons.
pub(crate) struct Repeated {
    pub criterion: usize,
    pub earlier: (u32, String),
    pub current: (u32, String),
}

pub(crate) fn repeated_unmet(
    task: &Task,
    idx: usize,
    attempt_n: u32,
    review: &ReviewResult,
) -> Option<Repeated> {
    let reason = |r: &CriterionRuling| r.evidence.clone().unwrap_or_else(|| r.criterion.clone());
    for ruling in review.criteria.iter().filter(|r| r.met == Some(false)) {
        let Some(i) = ruling.index.filter(|i| *i < task.criteria.len()) else {
            continue;
        };
        if task.brief_check.judged.contains(&task.criteria[i]) {
            continue;
        }
        for earlier in task.attempts[..idx]
            .iter()
            .filter(|a| a.stage == Stage::Implement)
        {
            let Some(r) = earlier.review.as_ref() else {
                continue;
            };
            if let Some(before) = r
                .criteria
                .iter()
                .find(|r| r.met == Some(false) && r.index == Some(i))
            {
                return Some(Repeated {
                    criterion: i,
                    earlier: (earlier.n, reason(before)),
                    current: (attempt_n, reason(ruling)),
                });
            }
        }
    }
    None
}

pub(crate) fn ruling_prompt(task: &Task, rep: &Repeated) -> String {
    format!(
        "{}\n\n{}\n",
        engine_text("healing_ruling"),
        json!({
            "goal": task.goal,
            "criterion": task.criteria[rep.criterion],
            "reviews": [
                {"attempt": rep.earlier.0, "reason": rep.earlier.1},
                {"attempt": rep.current.0, "reason": rep.current.1},
            ],
        })
    )
}

pub(crate) fn parse_ruling(text: &str) -> Option<Ruling> {
    let v = judge_json(text)?;
    let verdict = str_field(&v, "verdict").to_ascii_lowercase();
    if verdict.contains("infeasible") {
        let criterion = str_field(&v, "criterion");
        if criterion.is_empty() {
            return None;
        }
        return Some(Ruling::Infeasible {
            criterion,
            reason: reason_of(&v),
        });
    }
    verdict.contains("code").then_some(Ruling::CodeGap)
}

/// The decisions after the first `seen` that record an owner, lead or policy answer.
pub(crate) fn new_answers(task: &Task) -> Vec<String> {
    task.decisions
        .iter()
        .skip(task.brief_check.answers_seen)
        .filter(|d| {
            (d.starts_with("Owner: ")
                && !d.starts_with("Owner: stop")
                && !d.starts_with("Owner: dropped"))
                || (d.starts_with("Lead: ") && d.contains(" -> "))
                || d.starts_with("Policy: ")
        })
        .cloned()
        .collect()
}

pub(crate) fn answer_prompt(task: &Task, answers: &[String]) -> String {
    let criteria = indexed_criteria(task);
    format!(
        "{}\n\n{}\n",
        engine_text("healing_answer"),
        json!({"goal": task.goal, "criteria": criteria, "answers": answers})
    )
}

pub(crate) fn parse_amendments(text: &str, criteria: usize) -> Vec<(usize, String)> {
    let Some(v) = judge_json(text) else {
        return Vec::new();
    };
    v.get("amend")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let i = item.get("criterion")?.as_u64()? as usize;
            let text = str_field(item, "text");
            (i < criteria && !text.is_empty()).then_some((i, text))
        })
        .collect()
}

fn indexed_criteria(task: &Task) -> Vec<String> {
    task.criteria
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{i}: {c}"))
        .collect()
}
