pub(crate) enum RunError {
    Cancelled,
    Io(String),
    /// The harness binary does not exist: nothing ran, so it costs no attempt.
    NotFound(String),
}
