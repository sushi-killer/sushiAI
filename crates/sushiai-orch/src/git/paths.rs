/// Whether two repo paths name the same folder: symlinked paths (macOS
/// /var -> /private/var) do.
pub fn same_repo_path(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_end_matches('/'), b.trim_end_matches('/'));
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(x), Ok(y)) if x == y
        )
}

pub(crate) fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}
