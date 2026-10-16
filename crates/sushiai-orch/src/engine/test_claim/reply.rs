use crate::clip::truncate_chars;
use crate::engine::protected::paths::MAX_TEST_BYTES;

const MAX_WHY_CHARS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::engine) enum Verdict {
    Confirmed,
    Rejected,
    Unclear,
}

/// The judge's reply: verdict and a one-line reason.
pub(in crate::engine) fn parse_ruling(text: &str) -> Option<(Verdict, String)> {
    let v: serde_json::Value = serde_json::from_str(&crate::prompts::reply::tagged_json(
        text,
        "sushi-test-ruling",
    )?)
    .ok()?;
    let verdict = match v
        .get("verdict")?
        .as_str()?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "confirmed" => Verdict::Confirmed,
        "rejected" => Verdict::Rejected,
        "unclear" => Verdict::Unclear,
        _ => return None,
    };
    let why = v
        .get("why")
        .and_then(|w| w.as_str())
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    Some((verdict, truncate_chars(&why, MAX_WHY_CHARS)))
}

/// The planner's rewritten file; `None` when empty or over the size cap.
pub(in crate::engine) fn parse_rewrite(text: &str) -> Option<String> {
    let v: serde_json::Value =
        serde_json::from_str(&crate::prompts::reply::tagged_json(text, "sushi-test")?).ok()?;
    let content = v.get("content")?.as_str()?.to_string();
    (!content.trim().is_empty() && content.len() <= MAX_TEST_BYTES).then_some(content)
}
