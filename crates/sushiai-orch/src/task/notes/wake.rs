pub(crate) struct WakeDecision<'a> {
    pub(crate) at: i64,
    pub(crate) reason: &'a str,
    pub(crate) task_id: Option<&'a str>,
    pub(crate) outcome: &'a str,
}
