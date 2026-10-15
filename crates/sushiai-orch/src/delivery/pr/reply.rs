use crate::harness;
use serde::Deserialize;

/// The reply shape the drafter must match, enforced by the harness
/// (`--json-schema` / `--output-schema`).
/// A JSON schema, not prompt text: it stays in code, byte for byte, because
/// the claude argv carries it and the argv is part of every run's `promptHash`.
pub(crate) fn draft_schema() -> &'static str {
    r#"{"type":"object","properties":{"branch":{"type":"string"},"title":{"type":"string"},"body":{"type":"string"}},"required":["branch","title","body"],"additionalProperties":false}"#
}

/// The reply shape of the body check, enforced the same way.
pub(crate) fn body_check_schema() -> &'static str {
    r#"{"type":"object","properties":{"ok":{"type":"boolean"},"corrections":{"type":"array","items":{"type":"string"}},"body":{"type":["string","null"]}},"required":["ok","corrections","body"],"additionalProperties":false}"#
}

/// What the drafter replied: the branch, title and body of the pull request.
/// An empty field falls back to the task's own.
#[derive(Debug, Default, PartialEq, Deserialize)]
pub(crate) struct Draft {
    pub(crate) branch: String,
    pub(crate) title: String,
    pub(crate) body: String,
}

/// What the body check replied. `ok` is the verdict; `body` is the whole
/// corrected body when `ok` is false, and null otherwise.
#[derive(Debug, PartialEq, Deserialize)]
pub(crate) struct BodyCheck {
    pub(crate) ok: bool,
    pub(crate) corrections: Vec<String>,
    pub(crate) body: Option<String>,
}

/// The turn's reply, read as the schema the harness enforced. A reply that
/// does not deserialize fails the turn; it is never read leniently.
pub(crate) fn schema_reply<T: serde::de::DeserializeOwned>(
    outcome: &harness::RunOutcome,
    what: &str,
) -> Result<T, String> {
    if outcome.timed_out {
        return Err("the run timed out".to_string());
    }
    if let Some(error) = &outcome.error {
        return Err(error.clone());
    }
    serde_json::from_str(outcome.final_text.as_deref().unwrap_or_default())
        .map_err(|_| format!("the reply did not match the {what} schema"))
}

#[derive(Clone, Copy)]
pub(crate) enum PrTurn {
    Draft,
    BodyCheck,
}
impl PrTurn {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Draft => "pr-draft",
            Self::BodyCheck => "pr-truth",
        }
    }
    pub(crate) fn schema(self) -> &'static str {
        match self {
            Self::Draft => draft_schema(),
            Self::BodyCheck => body_check_schema(),
        }
    }
}
