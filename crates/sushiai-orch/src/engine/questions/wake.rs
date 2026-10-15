use crate::clock::now_ms;
use crate::engine::answer_policy::owner_only;
use crate::engine::questions::answers::overseer_allowed_answers;
use crate::engine::state::App;
use crate::task::Question;
use crate::task::Task;
use crate::task::TaskStatus;
use serde_json::json;
use std::sync::atomic::Ordering;

/// The waiting question as an overseer turn gets it: its typed fields and the
/// answers its gates allow (`allowedAnswers`), after the wake prompt.
pub(in crate::engine) fn waiting_prompt(app: &App, key: &str, task: &Task) -> String {
    let prompt = overseer_prompt(key, task);
    let Some(question) = task.question.as_ref() else {
        return prompt;
    };
    let allowed = overseer_allowed_answers(&app.settings.read().unwrap(), task, question);
    let typed = json!({
        "kind": question.kind,
        "text": question.text,
        "options": question.options,
        "allowedAnswers": allowed,
    });
    format!("{prompt}\n\nThe waiting question: {typed}")
}

/// Wakes the orchestrator chat (an overseer turn) for a task that just parked
/// on a question nobody answered by rule or precedent, when
/// `auto_answer` is on. Parking happens once per question, so this never wakes
/// twice for the same one. An owner-only kind never wakes it: the owner
/// answers those, and `task_answer` would refuse the agent anyway.
///
/// The answer policy and triage had their turn before the task parked, so a
/// question nobody else takes here is handed to the owner (`for_owner`): an
/// owner-only kind at once (also in quiet hours), any question with
/// auto-answer off, and one whose overseer turn cannot start
/// (`crate::engine::chat::wake::wake_overseer`). A held question is handed on when its turn ends.
pub(in crate::engine) fn wake_orchestrator_for_waiting(app: &App, task: &Task) {
    let Some(question) = task.question.as_ref() else {
        return;
    };
    if owner_only(question.kind) {
        // Nobody wakes for it; quiet hours would leave the owner unaware.
        app.record_quiet_parked(task);
        app.hand_to_owner(&task.id, question.asked_at);
        return;
    }
    if !app.settings.read().unwrap().auto_answer {
        app.hand_to_owner(&task.id, question.asked_at);
        return;
    }
    if answer_like_precedent(app, task, question) {
        return;
    }
    wake_for_waiting(app, task);
}

/// Wakes the overseer for every question that parked while auto-answer was
/// off: their own park-time wake found it off and never fires again.
pub(in crate::engine) fn wake_orchestrator_for_parked(app: &App) {
    for task in app.store.list_tasks().unwrap_or_default() {
        let parked = task.status == TaskStatus::Waiting
            && !task.archived
            && task
                .question
                .as_ref()
                .is_some_and(|q| !q.held_now(now_ms()));
        if parked {
            wake_orchestrator_for_waiting(app, &task);
        }
    }
}

fn wake_for_waiting(app: &App, task: &Task) {
    let prompt = app.with_precedents(waiting_prompt(app, "overseer_waiting", task), &[task]);
    crate::engine::chat::wake::wake_overseer(
        app,
        &task.repo,
        crate::engine::chat::wake::WakeRequest::new(prompt, false, "waiting", Some(&task.id)),
    );
}

/// Holds the question and answers it as the overseer, like the owner did
/// before (`decision_memory.rs`), with no overseer turn. Every guard of
/// `task.answer` still applies; one that refuses ends the hold and the
/// overseer is woken as usual. Whether it took the question.
fn answer_like_precedent(app: &App, task: &Task, question: &Question) -> bool {
    let Some((answer, like)) = app.precedent_answer(task, question) else {
        return false;
    };
    if tokio::runtime::Handle::try_current().is_err() || app.shutting_down.load(Ordering::SeqCst) {
        return false;
    }
    let held = app.hold_for_overseer(&task.repo, "waiting", Some(&task.id));
    if held.is_empty() {
        return false;
    }
    let app = app.arc();
    let (repo, id) = (task.repo.clone(), task.id.clone());
    tokio::spawn(async move {
        let params = json!({"id": id, "answer": answer, "source": "overseer", "like": like});
        if app.handle_task_answer(params).await.is_ok() {
            return;
        }
        app.release_holds(&repo, &held, false);
        if let Ok(Some(task)) = app.store.load_task(&id) {
            if task.status == TaskStatus::Waiting && task.question.is_some() {
                wake_for_waiting(&app, &task);
            }
        }
    });
    true
}

/// Wakes the overseer once, at the moment the task loop moves a top-level
/// task into `done`, when `auto_answer` is on and nothing landed or opened a
/// pull request yet. The task remembers it (`overseer_woken`), so a restart,
/// an unarchive or a rebroadcast never wakes it again. A busy chat drops the
/// wake; the next digest mentions the task.
pub(in crate::engine) fn wake_orchestrator_for_done(app: &App, task: &mut Task) {
    if task.status != TaskStatus::Done
        || task.parent.is_some()
        || task.archived
        || task.overseer_woken
        || task.pr_url.is_some()
        || task.landed_sha.is_some()
        || !app.settings.read().unwrap().auto_answer
    {
        return;
    }
    let prompt = overseer_prompt("overseer_done", task);
    // Marked either way: this is the one transition, not a retry loop.
    task.mark_overseer_woken();
    app.save_task_logged(task);
    crate::engine::chat::wake::wake_overseer(
        app,
        &task.repo,
        crate::engine::chat::wake::WakeRequest::new(prompt, false, "done", Some(&task.id)),
    );
}

/// A server-built overseer prompt (`prompts/orchestrator.yaml`) naming the task.
pub(super) fn overseer_prompt(key: &str, task: &Task) -> String {
    crate::prompts::get(key)
        .replace("{id}", &task.id)
        .replace("{title}", &task.title)
}
