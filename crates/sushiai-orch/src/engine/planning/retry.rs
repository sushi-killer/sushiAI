pub(in crate::engine::planning) enum Retry {
    First,
    Parse,
    Error(Option<String>),
}

impl Retry {
    pub(in crate::engine::planning) fn number(&self) -> u32 {
        if matches!(self, Self::First) {
            0
        } else {
            1
        }
    }
    pub(in crate::engine::planning) fn session(&self) -> Option<&str> {
        match self {
            Self::Error(session) => session.as_deref(),
            _ => None,
        }
    }
    pub(in crate::engine::planning) fn doubled(&self) -> bool {
        matches!(self, Self::Error(_))
    }
}
