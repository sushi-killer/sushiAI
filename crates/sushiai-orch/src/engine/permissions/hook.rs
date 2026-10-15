use crate::engine::permissions::answers::permission_question;
use crate::engine::permissions::answers::record_permission_answer;
use crate::engine::permissions::gate::gate_for;
use crate::engine::permissions::gate::Gate;
use crate::engine::permissions::staging::staging_reason;
use crate::engine::questions::answers::record_answer_for;
use crate::engine::run_control::HookContext;
use crate::engine::state::App;
use crate::policy::permissions::parse_choice;
use crate::policy::permissions::Choice;
use crate::policy::permissions::Subject;
use crate::policy::permissions::WEB_TOOLS;
use crate::prompts::clip::truncate_chars;
use crate::store::permissions::add_rule;
use crate::store::permissions::Scope;
use crate::task::AnsweredBy;
use crate::task::TaskStatus;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::oneshot;

const MAX_DETAIL_CHARS: usize = 400;

fn decision(kind: &str, reason: &str) -> serde_json::Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": kind,
            "permissionDecisionReason": reason,
        }
    })
}

/// Holds the call: the task shows one question, the run waits, and the answer
/// comes back through `task.answer`. `None` when the run ended first.
pub(super) async fn ask(
    app: &Arc<App>,
    ctx: &HookContext,
    subject: &Subject,
    detail: &str,
) -> Option<String> {
    let question = permission_question(std::slice::from_ref(subject), detail);
    let (tx, rx) = oneshot::channel();
    app.permission_waits
        .lock()
        .unwrap()
        .insert(ctx.task_id.clone(), tx);
    // The harness is silent while it waits for us: the stall clock pauses.
    let _paused = ctx.hook_running.start();
    if let Err(e) = app.update_task(&ctx.task_id, |task| {
        task.ask(question.clone(), task.updated_at);
    }) {
        eprintln!("orch: updating task {} failed: {e}", ctx.task_id);
        app.permission_waits.lock().unwrap().remove(&ctx.task_id);
        return None;
    }
    let reply = tokio::select! {
        _ = ctx.cancel.cancelled() => None,
        a = rx => a.ok(),
    };
    let evidence = reply.as_ref().and_then(|r| r.default_evidence.as_deref());
    let answer = reply.as_ref().map(|r| r.text.clone());
    app.permission_waits.lock().unwrap().remove(&ctx.task_id);
    // `task.answer` marks an agent's answer (it may only deny here).
    let caller = app
        .take_agent_answer(&ctx.task_id)
        .unwrap_or(AnsweredBy::Owner);
    if let Err(e) = app.update_task(&ctx.task_id, |task| {
        if task
            .question
            .as_ref()
            .is_some_and(|q| q.text == question.text)
        {
            if let Some(answer) = &answer {
                task.clear_question();
                record_answer_for(
                    task,
                    Some(&question),
                    &crate::engine::questions::reply::Reply {
                        text: (answer).to_string(),
                        default_evidence: (evidence).map(str::to_string),
                        like: (None).map(str::to_string),
                    },
                    caller,
                );
                task.resume_phase(TaskStatus::Running, task.updated_at);
            } else if app
                .controls
                .lock()
                .unwrap()
                .get(&ctx.task_id)
                .is_none_or(|ctrl| ctrl.signals.cancel.is_cancelled())
            {
                task.resume_phase(app.cancelled_status(&task.status), task.updated_at);
                if task.status == TaskStatus::Stopped {
                    app.close_stopped_task(task);
                }
            } else {
                // The hook token also fires when a run ends while its task
                // loop continues; only the expired permission question closes.
                task.clear_question();
            }
        }
    }) {
        eprintln!("orch: updating task {} failed: {e}", ctx.task_id);
    }
    answer
}

fn call_detail(tool: &str, input: Option<&serde_json::Value>) -> String {
    match (tool, input) {
        (_, None) => String::new(),
        (t, Some(input)) if t.starts_with("mcp__") || WEB_TOOLS.contains(&t) => truncate_chars(
            &format!("Call: {input}"),
            MAX_DETAIL_CHARS,
        ),
        _ => "Claude Code protects this path for agents; the file is written in a staging directory and copied into place once you allow it.".to_string(),
    }
}

/// `hook.edit`'s permission half: the decision for one tool call, `{}` when
/// the call is not orchd's to judge.
pub(in crate::engine) async fn decide_call(
    app: &Arc<App>,
    ctx: &HookContext,
    payload: &serde_json::Value,
) -> serde_json::Value {
    let Some(tool) = payload.get("tool_name").and_then(|t| t.as_str()) else {
        return json!({});
    };
    let input = payload.get("tool_input");
    let first = gate_for(app, ctx, tool, input);
    let subject = match first {
        Gate::Pass => return json!({}),
        Gate::Allow => return decision("allow", "allowed for this run"),
        Gate::Stage(rel) => {
            ctx.staged.lock().unwrap().insert(rel.clone());
            return decision("deny", &staging_reason(ctx, &rel));
        }
        Gate::Deny(reason) => return decision("deny", &reason),
        Gate::Ask(subject) => subject,
    };
    // One question at a time; a call that waited behind another may find its
    // subject already answered.
    let _one = ctx.ask_lock.lock().await;
    match gate_for(app, ctx, tool, input) {
        Gate::Allow => return decision("allow", "allowed for this run"),
        Gate::Stage(rel) => {
            ctx.staged.lock().unwrap().insert(rel.clone());
            return decision("deny", &staging_reason(ctx, &rel));
        }
        Gate::Deny(reason) => return decision("deny", &reason),
        _ => {}
    }
    if ctx.run_denied.lock().unwrap().contains(&subject.rule()) {
        return decision("deny", &denied_reason(&subject, None));
    }
    let detail = call_detail(tool, input);
    let Some(answer) = ask(app, ctx, &subject, &detail).await else {
        return decision("deny", "The run was stopped before the owner answered.");
    };
    let choice = parse_choice(&answer);
    ctx.handled
        .lock()
        .unwrap()
        .insert(subject.name().to_string());
    if let Err(e) = app.update_task(&ctx.task_id, |task| {
        record_permission_answer(task, &subject, &choice)
    }) {
        eprintln!("orch: updating task {} failed: {e}", ctx.task_id);
    }
    match choice {
        Choice::Deny(note) => {
            ctx.run_denied.lock().unwrap().insert(subject.rule());
            decision("deny", &denied_reason(&subject, note.as_deref()))
        }
        Choice::Once | Choice::Always => {
            if choice == Choice::Always {
                add_rule(&app.data_dir, Scope::Repo(&ctx.repo), &subject.rule());
            }
            // A write to an outside service asks again on the next call
            // unless the owner chose Always; a path or a read-like tool stays
            // allowed for the rest of the run.
            let single_call = matches!(&subject, Subject::Tool(t) if t.starts_with("mcp__"))
                && choice == Choice::Once;
            match &subject {
                Subject::Path(rel) => {
                    ctx.run_grants.lock().unwrap().insert(subject.rule());
                    ctx.staged.lock().unwrap().insert(rel.clone());
                    decision("deny", &staging_reason(ctx, rel))
                }
                Subject::Tool(_) => {
                    if !single_call {
                        ctx.run_grants.lock().unwrap().insert(subject.rule());
                    }
                    decision("allow", "the owner allowed it")
                }
            }
        }
    }
}

pub(super) fn denied_reason(subject: &Subject, note: Option<&str>) -> String {
    let mut text = format!(
        "The owner denied {}. Do not try it again; meet the task another way, or report `blocked` and say what you could not do.",
        subject.name()
    );
    if let Some(note) = note {
        text.push_str(&format!(" The owner said: {note}"));
    }
    text
}
