use crate::engine::chat::types::ChatMode;

use super::{push, ChatSession, MAX_MESSAGES};

#[test]
fn a_session_keeps_its_latest_messages_only() {
    let mut thread = ChatSession::default();
    for i in 0..(MAX_MESSAGES + 5) {
        push(&mut thread, "user", &i.to_string(), ChatMode::Chat);
    }
    assert_eq!(thread.messages.len(), MAX_MESSAGES);
    assert_eq!(thread.messages[0].text, "5");
}
