/// FNV-1a 64-bit -- fast, dependency-free, plenty for a cache key (not a
/// security boundary).
pub(crate) fn simple_hash(s: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Strips digits, ids and hashes (long tokens with a digit, hex) and
/// absolute-path-looking tokens (so `/tmp/xyz123/a.ts:42`
/// across two runs still normalizes to the same signature) and collapses
/// whitespace.
pub(crate) fn normalize_signature_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    for word in line.split_whitespace() {
        // A path is cut at its first slash.
        let word = word.split('/').next().unwrap_or("");
        if word.is_empty() {
            continue;
        }
        let core = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        let ident = core.len() >= 8
            && core
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        let hex = core.len() >= 7 && core.chars().all(|c| c.is_ascii_hexdigit());
        // A request id, hash or token differs on every run of one failure.
        if hex || (ident && core.chars().any(|c| c.is_ascii_digit())) {
            out.push_str(" ID ");
            continue;
        }
        out.extend(word.chars().filter(|c| !c.is_ascii_digit()));
        out.push(' ');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
