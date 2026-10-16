//! The one place text is cut to a size. Each function says what it counts and
//! whether it marks the cut.

const ELLIPSIS: char = '\u{2026}';

/// At most `max` chars of `s`, from the start, with no marker.
pub fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((end, _)) => s[..end].to_string(),
        None => s.to_string(),
    }
}

/// At most `max` chars of `s`, then `…` when something was cut.
pub fn ellipsize_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((end, _)) => format!("{}{ELLIPSIS}", &s[..end]),
        None => s.to_string(),
    }
}

/// At most `max` bytes of `s`, cut on a char boundary so no multi-byte
/// character is split, then `…` when something was cut.
pub fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ELLIPSIS}", &s[..end])
}

/// The last `max` chars of `s` -- the failure detail is usually at the end of
/// a command's output, not the start.
pub fn tail_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    s.chars().skip(count - max).collect()
}
