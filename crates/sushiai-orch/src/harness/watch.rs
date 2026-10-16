use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

/// An implement attempt's stall watchdog: the silence limit, paused while
/// the hook clock runs.
pub(crate) struct Stall {
    pub(super) limit: Duration,
    pub(super) paused: Arc<HookClock>,
}

/// Whether a hook runs now, and when the last one ended. The end counts as
/// activity: the stall window starts then.
#[derive(Default)]
pub(crate) struct HookClock {
    pub(super) running: AtomicBool,
    pub(super) ended: StdMutex<Option<tokio::time::Instant>>,
}

impl HookClock {
    pub(crate) fn start(&self) -> ClearOnDrop<'_> {
        self.running.store(true, Ordering::SeqCst);
        ClearOnDrop(self)
    }
}

pub(crate) struct ClearOnDrop<'a>(&'a HookClock);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        *self.0.ended.lock().unwrap() = Some(tokio::time::Instant::now());
        self.0.running.store(false, Ordering::SeqCst);
    }
}
