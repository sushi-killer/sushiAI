/// How a run relates to its attempt.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunTrack {
    /// A nested run (review, triage, audit, chat): pipes, no recovery.
    No,
    /// The attempt's own run, on files: its pgid and session are saved. A
    /// daemon shutdown kills it.
    Attempt,
    /// As `Attempt`, and a daemon shutdown leaves it running for the next
    /// daemon to adopt (the implement run).
    Resumable,
}
