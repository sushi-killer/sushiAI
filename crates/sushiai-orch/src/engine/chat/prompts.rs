use super::types::ChatMessage;
use crate::clip::truncate_chars;
use crate::prompts::engine_text;

/// The owner's `text` followed by the questions tasks asked while it was typed.
pub(super) fn with_questions(text: &str, questions: &str) -> String {
    format!(
        "{text}\n\n---\n\n{}\n\n{questions}",
        engine_text("chat_questions_intro")
    )
}

/// The prompt of a turn that only answers task `questions`.
pub(super) fn wake_questions_prompt(state: &str, questions: &str) -> String {
    format!(
        "{}{} {}\n\n{questions}",
        state_header(state),
        engine_text("chat_questions_intro"),
        engine_text("chat_question_turn_reply")
    )
}

const MAX_HISTORY_TEXT: usize = 1500;

/// The kept part of an edited conversation, compact, for a fresh harness
/// session that has no memory of it.
pub(super) fn history_prompt(kept: &[ChatMessage]) -> String {
    if kept.is_empty() {
        return String::new();
    }
    let lines = kept
        .iter()
        .map(|m| {
            let who = if m.role == "user" {
                "Owner"
            } else {
                "Orchestrator"
            };
            format!("{who}: {}", truncate_chars(m.text.trim(), MAX_HISTORY_TEXT))
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "{}\n\n{lines}\n\n---\n\nThe owner's edited message:\n\n",
        engine_text("chat_history_intro")
    )
}

/// The top of a wake prompt: the overseer's state file, which stands in for the
/// chat history a wake does not resume.
pub(super) fn state_header(state: &str) -> String {
    format!("{}\n\n{state}\n---\n\n", engine_text("chat_overseer_state"))
}

#[cfg(test)]
#[path = "tests/prompts.rs"]
mod tests;
