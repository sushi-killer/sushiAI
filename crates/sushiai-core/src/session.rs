use sushiai_protocol::{SessionInfo, SessionStatus};

pub fn mark_exited(info: &mut SessionInfo, code: Option<i32>) {
    info.status = SessionStatus::Exited;
    info.exit_code = code;
    // An exited session takes no more hooks: its token stops working, its asks are gone.
    info.agent.token_hash = None;
    info.agent.asks.clear();
}

/// The process is gone but the agent conversation can be resumed: the record sleeps. Like an
/// exit, it ends the token and the asks; unlike one, it keeps the agent session.
pub fn mark_hibernated(info: &mut SessionInfo, now_ms: u64) {
    info.status = SessionStatus::Hibernated;
    info.exit_code = None;
    info.holder_pid = None;
    info.agent.token_hash = None;
    info.agent.asks.clear();
    info.agent.hibernated_at = Some(now_ms);
}
