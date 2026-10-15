use crate::task::{Task, VerifyOutcome};

impl Task {
    /// Delivery checks can run without an implement attempt, including a finished parent.
    pub(crate) fn record_delivery_checks(&mut self, results: Vec<VerifyOutcome>) {
        self.data.delivery_checks.extend(results);
    }
}
