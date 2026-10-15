pub(crate) const HOUR_MS: i64 = 3_600_000;

/// Whether a reminder is due: one interval after the question was asked, or
/// after the last reminder for it. A restart after a long gap gives one
/// reminder, not one per missed interval.
pub(crate) fn reminder_due(
    asked_at: i64,
    reminded_at: Option<i64>,
    every_hours: u32,
    now: i64,
) -> bool {
    if every_hours == 0 {
        return false;
    }
    let base = reminded_at.filter(|r| *r >= asked_at).unwrap_or(asked_at);
    now - base >= i64::from(every_hours) * HOUR_MS
}

/// Whether a question has waited `after_hours` (0 = never).
pub(crate) fn default_due(asked_at: i64, after_hours: u32, now: i64) -> bool {
    after_hours > 0 && now - asked_at >= i64::from(after_hours) * HOUR_MS
}
