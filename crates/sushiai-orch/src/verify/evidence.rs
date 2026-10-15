use crate::task::attempt::VerifyOutcome;

/// The name a held-out check goes by everywhere its command must not
/// appear: decision lines, the attempt's verify list, failure details.
pub fn is_held_out(v: &VerifyOutcome) -> bool {
    v.command.starts_with("held-out check (criterion ")
}
