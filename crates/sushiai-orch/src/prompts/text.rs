use crate::clip::truncate_bytes;
use crate::prompts::{brief_text, fill};

/// A failure detail opens with what failed (`<cmd> exited 101.`) and ends
/// with why (the test runner's summary): keep both ends, drop the middle.
pub(crate) fn clip_middle(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(200).collect();
    let tail: String = s.chars().skip(count - (max - 200)).collect();
    format!("{head}\n\u{2026}\n{tail}")
}

/// Cap applied to each piece of agent-written text quoted into a brief for
/// another session -- independent of, and generally tighter than,
/// [`MAX_FAILURE_DETAIL`] (which is sized for an agent's own retry brief, not
/// a nested untrusted quote inside a different session's prompt).
const UNTRUSTED_MAX_CHARS: usize = 2000;

fn escape_untrusted_tags(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for (i, c) in text.char_indices() {
        if c == '<' {
            let rest = text[i + 1..].trim_start();
            let rest = rest.strip_prefix('/').unwrap_or(rest).trim_start();
            if rest
                .get(.."untrusted-data".len())
                .is_some_and(|tag| tag.eq_ignore_ascii_case("untrusted-data"))
            {
                escaped.push_str("&lt;");
                continue;
            }
        }
        escaped.push(c);
    }
    escaped
}

/// Wraps a piece of agent-written text (the question itself, a summary, an
/// `"Agent: ..."` decision note, or a failure detail) in an explicit fence
/// before it goes into a brief for another session: all of it comes from a
/// previous, less-trusted agent session in the same worktree that session is
/// about to read, so it must never be mistaken for that session's own
/// instructions.
pub(crate) fn untrusted_block(label: &str, text: &str) -> String {
    let mut out = fill(
        brief_text("untrusted"),
        &[
            ("label", &escape_untrusted_tags(label)),
            (
                "text",
                &escape_untrusted_tags(&truncate_bytes(text, UNTRUSTED_MAX_CHARS)),
            ),
        ],
    );
    out.push('\n');
    out
}

#[cfg(test)]
#[path = "tests/text.rs"]
mod tests;
