use crate::engine::state::App;
use std::sync::atomic::Ordering;
use std::time::Duration;
impl App {
    pub(super) fn helper_timeout(&self) -> Duration {
        Duration::from_millis(self.helper_timeout_ms.load(Ordering::SeqCst))
    }
}

impl App {
    /// The planner's limit: `planTimeoutSecs` (`0` = the default), unless a
    /// test set an override.
    pub(super) fn planner_timeout(&self) -> Duration {
        let over = self.plan_timeout_override_ms.load(Ordering::SeqCst);
        if over > 0 {
            return Duration::from_millis(over);
        }
        let secs = self.settings.read().unwrap().plan_timeout_secs;
        Duration::from_secs(if secs == 0 { 1800 } else { secs })
    }
}
