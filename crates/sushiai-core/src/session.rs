use sushiai_protocol::{SessionInfo, SessionStatus};

pub fn mark_exited(info: &mut SessionInfo, code: Option<i32>) {
    info.status = SessionStatus::Exited;
    info.exit_code = code;
}
