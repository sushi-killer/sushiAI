use super::authority::{check_authority, removal_refusal};
use super::judge::run_answer_judge;
use super::{policy_answers, MAX_POLICY_ANSWERS};
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::policy::settings::Settings;
use crate::prompts::engine_text;
use crate::task::checks::authority::{criterion_commands, Origin};
use crate::task::checks::commands::flat_command;
use crate::task::checks::rewrite::{shown_commands, CheckRewrite};
use crate::task::checks::snapshot::CriterionEdit;
use crate::task::{Question, Task};
use crate::verify::healing::reply::{judge_json, reason_of, str_field};
use serde_json::json;
use std::sync::Arc;

pub(in crate::engine) const REWRITE_CHECK: &str = "rewrite check";

pub(in crate::engine) fn planner_check_criterion(
    settings: &Settings,
    task: &Task,
    n: usize,
) -> bool {
    let commands: Vec<String> = criterion_commands(task, n)
        .into_iter()
        .filter(|c| !flat_command(c).is_empty())
        .collect();
    !commands.is_empty() && removal_refusal(settings, task, &commands, Origin::Other).is_none()
}

pub(in crate::engine) fn planner_check_brief(
    criterion: &str,
    checks: &[String],
    evidence: &str,
) -> String {
    format!(
        "{}\n\n{}\n",
        engine_text("answer_policy_planner_check_judge"),
        json!({"criterion": criterion, "checks": checks, "evidence": evidence})
    )
}

#[derive(Debug, PartialEq)]
pub(super) enum PlannerVerdict {
    Drop(String),
    Rewrite {
        command: String,
        criterion_text: String,
        reason: String,
    },
    Retry(String),
}

pub(super) fn parse_planner_verdict(text: &str) -> Option<PlannerVerdict> {
    let v = judge_json(text)?;
    let reason = reason_of(&v);
    match str_field(&v, "verdict").to_ascii_lowercase().as_str() {
        "drop" => Some(PlannerVerdict::Drop(reason)),
        "retry" => Some(PlannerVerdict::Retry(reason)),
        "rewrite" => {
            let command = v.get("command")?.as_str()?.trim().to_string();
            (!command.is_empty()).then(|| PlannerVerdict::Rewrite {
                command,
                criterion_text: str_field(&v, "criterionText"),
                reason,
            })
        }
        _ => None,
    }
}

pub(in crate::engine) fn rewrite_criterion_check(
    settings: &Settings,
    task: &mut Task,
    edit: &CriterionEdit,
    rewrite: &CheckRewrite,
) -> Result<(), String> {
    crate::task::checks::rewrite::replace(&check_authority(settings, task), task, edit, rewrite)
}

pub(super) async fn settle_planner_check(
    app: &Arc<App>,
    task: &Task,
    question: &Question,
    attempt_n: u32,
    cancel: &CancelToken,
) -> super::publication::AnswerRuling {
    let mut ruling = super::publication::AnswerRuling {
        answer: None,
        rewrite: None,
        cost: 0.0,
        notes: Vec::new(),
    };
    let Some(index) = question.criterion else {
        return ruling;
    };
    let Some(edit) = CriterionEdit::capture(task, index) else {
        return ruling;
    };
    let why = if policy_answers(task, question) >= MAX_POLICY_ANSWERS {
        format!("{MAX_POLICY_ANSWERS} automatic answers on this criterion are spent")
    } else {
        let brief = planner_check_brief(
            &task.criteria[index],
            &shown_commands(task, index),
            question.evidence.as_deref().unwrap_or_default(),
        );
        let judged = run_answer_judge(app, task, &brief, attempt_n, cancel).await;
        ruling.cost = judged.cost;
        ruling.notes = judged.notes;
        if cancel.is_cancelled() {
            return ruling;
        }
        match judged.reply.map(|reply| parse_planner_verdict(&reply)) {
            Ok(Some(PlannerVerdict::Drop(reason))) => {
                ruling.answer = Some(super::AutomaticAnswer::Judge {
                    text: "drop criterion".into(),
                    evidence: reason,
                });
                return ruling;
            }
            Ok(Some(PlannerVerdict::Retry(reason))) => {
                ruling.answer = Some(super::AutomaticAnswer::Judge {
                    text: "retry".into(),
                    evidence: reason,
                });
                return ruling;
            }
            Ok(Some(PlannerVerdict::Rewrite {
                command,
                criterion_text,
                reason,
            })) => {
                ruling.answer = Some(super::AutomaticAnswer::Judge {
                    text: REWRITE_CHECK.into(),
                    evidence: reason.clone(),
                });
                ruling.rewrite = Some((
                    edit,
                    CheckRewrite {
                        command,
                        criterion_text,
                        reason,
                    },
                ));
                return ruling;
            }
            Ok(None) => "the judge gave no usable reply".to_string(),
            Err(error) => error.to_string(),
        }
    };
    ruling.answer = Some(super::AutomaticAnswer::Policy {
        text: "drop criterion".into(),
        evidence: format!("{why}; every check of this criterion was written by the planner, so it is dropped without the owner"),
    });
    ruling
}
