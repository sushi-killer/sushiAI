use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Read,
    Write,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Read => "read",
            Kind::Write => "write",
        }
    }
}

/// Read or write: the owner's override first, then the server's
/// `readOnlyHint`; a tool with neither is a write.
pub fn classify(
    name: &str,
    read_only_hint: Option<bool>,
    overrides: &BTreeMap<String, String>,
) -> Kind {
    match overrides.get(name).map(String::as_str) {
        Some("read") => return Kind::Read,
        Some("write") => return Kind::Write,
        _ => {}
    }
    if read_only_hint == Some(true) {
        Kind::Read
    } else {
        Kind::Write
    }
}

#[cfg(test)]
mod tests;
