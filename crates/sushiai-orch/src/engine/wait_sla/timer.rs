use crate::clock::now_ms;
use crate::engine::state::App;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// How often the timer looks at waiting questions.
const SLA_CHECK: Duration = Duration::from_secs(60);

impl App {
    /// The once-a-minute waiting-SLA check; ends with the daemon.
    pub(in crate::engine) fn spawn_wait_sla_timer(&self) {
        let app = self.arc();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(SLA_CHECK);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if app.shutting_down.load(Ordering::SeqCst) {
                    return;
                }
                app.wait_sla_tick(now_ms()).await;
            }
        });
    }
}
