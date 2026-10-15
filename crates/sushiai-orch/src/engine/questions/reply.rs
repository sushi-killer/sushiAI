/// An answer on its way to a parked loop. `default_evidence` is set when the
/// waiting SLA gave it (`wait_sla.rs`): the loop records a policy assumption
/// with that evidence instead of an owner or orchestrator line.
#[derive(Debug)]
pub(in crate::engine) struct Reply {
    pub text: String,
    pub default_evidence: Option<String>,
    /// The overseer answered like an earlier answer (`task.answer {like}`).
    pub like: Option<String>,
}

#[cfg(test)]
impl From<&str> for Reply {
    fn from(text: &str) -> Self {
        Reply {
            text: text.to_string(),
            default_evidence: None,
            like: None,
        }
    }
}
