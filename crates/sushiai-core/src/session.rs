use sushiai_protocol::{SessionInfo, SessionStatus};

pub fn mark_exited(info: &mut SessionInfo, code: Option<i32>) {
    info.status = SessionStatus::Exited;
    info.exit_code = code;
    // An exited session takes no more hooks: its token stops working, its asks are gone.
    info.agent.token_hash = None;
    info.agent.asks.clear();
}
