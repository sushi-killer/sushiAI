use crate::engine::chat::types::ChatMessage;
use serde_json::json;

use super::{history_prompt, state_header, wake_questions_prompt, with_questions};
use crate::prompts::assert_engine_snapshot as snapshot;

fn message(role: &str, text: &str) -> ChatMessage {
    serde_json::from_value(json!({
        "id": "m", "role": role, "text": text, "ts": 1_000
    }))
    .unwrap()
}

#[test]
fn prompt_snapshot_chat_questions_with_the_owners_message() {
    snapshot(
        "chat_questions_owner",
        &with_questions(
            "Please ship it.",
            "Task \"Add a Save button\" asks: retry or stop?",
        ),
    );
}

#[test]
fn prompt_snapshot_chat_questions_wake() {
    snapshot(
        "chat_questions_wake",
        &wake_questions_prompt(
            "## State\n- decided: retry",
            "Task \"Add a Save button\" asks: retry or stop?",
        ),
    );
}

#[test]
fn prompt_snapshot_chat_overseer_state() {
    snapshot(
        "chat_overseer_state",
        &state_header("## State\n- decided: retry\n"),
    );
}

#[test]
fn prompt_snapshot_chat_history() {
    let kept = vec![
        message("user", "Plan the settings work."),
        message("assistant", "Two tasks: the dialog, then the save."),
    ];
    snapshot("chat_history", &history_prompt(&kept));
}
