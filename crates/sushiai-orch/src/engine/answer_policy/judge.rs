use crate::clip::truncate_chars;
use crate::engine::attempts::ledger::CostTag;
use crate::engine::attempts::run::{run_harness, RunIdentity, RunInvocation};
use crate::engine::attempts::tracking::RunTrack;
use crate::engine::brief_check::routing::check_route_for;
use crate::engine::review::native::{review_request, write_readonly_claude_settings_with_profile};
use crate::engine::state::App;
use crate::harness;
use crate::harness::cancel::CancelToken;
use crate::harness::guard::{helper_timeout_message, Guard};
use crate::policy::routing::Harness;
use crate::prompts::engine_text;
use crate::task::{Question, Task};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) fn impossible_parts(question: &Question) -> Option<(usize, String)> {
    Some((question.criterion?, question.evidence.clone()?))
}

pub(super) enum Judgement {
    Verified(String),
    Refuted(String),
}

const MAX_REASON_CHARS: usize = 300;

pub(super) fn parse_judgement(text: &str) -> Option<Judgement> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    let v: serde_json::Value = serde_json::from_str(text.get(start..=end)?).ok()?;
    let reason = truncate_chars(
        &v.get("reason")
            .and_then(|r| r.as_str())
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        MAX_REASON_CHARS,
    );
    match v.get("verified")?.as_bool()? {
        true => Some(Judgement::Verified(reason)),
        false => Some(Judgement::Refuted(reason)),
    }
}

pub(in crate::engine) fn impossible_brief(criterion: &str, evidence: &str) -> String {
    format!(
        "{}\n\n{}\n",
        engine_text("answer_policy_judge"),
        json!({"criterion": criterion, "evidence": evidence})
    )
}

/// `drop criterion` only when the judge verified the evidence, else `retry`;
/// `None` when the judge is off or gave no usable reply (the owner decides).
pub(super) async fn judge_impossible(
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
    let Some((index, evidence)) = impossible_parts(question) else {
        return ruling;
    };
    let Some(criterion) = task.criteria.get(index) else {
        return ruling;
    };
    let judged = run_answer_judge(
        app,
        task,
        &impossible_brief(criterion, &evidence),
        attempt_n,
        cancel,
    )
    .await;
    ruling.cost = judged.cost;
    ruling.notes = judged.notes;
    if let Ok(reply) = judged.reply {
        let answer = match parse_judgement(&reply) {
            Some(Judgement::Verified(reason)) => Some(("drop criterion", reason)),
            Some(Judgement::Refuted(reason)) => Some(("retry", reason)),
            None => None,
        };
        if let Some((text, evidence)) = answer {
            if let Some(why) = super::shared_check_drop_refusal(
                &app.settings.read().unwrap(),
                task,
                question,
                text,
            ) {
                ruling.notes.push(format!("Answer judge: {why}"));
            } else {
                ruling.answer = Some(super::AutomaticAnswer::Judge {
                    text: text.to_string(),
                    evidence,
                });
            }
        }
    }
    ruling
}

pub(super) struct AnswerJudge {
    pub(super) reply: Result<String, &'static str>,
    pub(super) cost: f64,
    pub(super) notes: Vec<String>,
}

pub(super) async fn run_answer_judge(
    app: &Arc<App>,
    task: &Task,
    brief_text: &str,
    attempt_n: u32,
    cancel: &CancelToken,
) -> AnswerJudge {
    const NO_ROUTE: &str = "no judge route is set";
    let mut judged = AnswerJudge {
        reply: Err(NO_ROUTE),
        cost: 0.0,
        notes: Vec::new(),
    };
    let Some((_, Some(route), fallback)) = check_route_for(app, &task.variant()) else {
        return judged;
    };
    if let Some(note) = fallback {
        judged.notes.push(format!("Answer judge: {note}"));
    }
    let settings = app.settings.read().unwrap().clone();
    let worktree = PathBuf::from(&task.worktree);
    let Ok(run_dir) = app
        .store
        .claim_helper_run_dir(&task.id, "answer-judge", Some(attempt_n))
    else {
        judged.reply = Err("the judge run could not start");
        return judged;
    };
    let mcp_path = run_dir.join("mcp.json");
    let _ = std::fs::write(&mcp_path, br#"{"mcpServers":{}}"#);
    let settings_path = run_dir.join("settings.json");
    let key_path = run_dir.join("key");
    let deny_read = vec![app.data_dir.to_string_lossy().to_string()];
    if matches!(route.harness, Harness::Claude) {
        write_readonly_claude_settings_with_profile(
            &route,
            app,
            &settings,
            &deny_read,
            &settings_path,
        );
    }
    let _ = std::fs::write(run_dir.join("brief.md"), brief_text);
    let events_path = run_dir.join("events.jsonl");
    let req = harness::RunRequest {
        repo_settings: false,
        resume: None,
        ..review_request(&route, &worktree, &mcp_path, &settings_path, &[])
    };
    let limit = app.helper_timeout();
    let result = run_harness(
        app,
        RunIdentity {
            id: &task.id,
            attempt_n,
            track: RunTrack::No,
            tag: CostTag::task("answer_judge", &route.id),
        },
        RunInvocation {
            request: &req,
            brief: brief_text,
            events: &events_path,
            schema: None,
        },
        cancel,
        Guard::helper("answer judge", limit),
    )
    .await;
    let _ = std::fs::remove_file(&key_path);
    // A cancelled or failed judge run gives no answer; the wait that follows
    // sees the cancellation itself.
    let Ok(outcome) = result else {
        judged.reply = Err("the judge run failed");
        return judged;
    };
    if outcome.timed_out {
        judged
            .notes
            .push(helper_timeout_message("answer judge", limit));
    }
    judged.cost = outcome.cost_usd.unwrap_or(0.0);
    judged.reply = if outcome.error.is_some() {
        Err("the judge run failed")
    } else {
        Ok(outcome.final_text.unwrap_or_default())
    };
    judged
}
