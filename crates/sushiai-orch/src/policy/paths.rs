/// Simple glob match: `*` matches any run of characters (including `/`,
/// deliberately simpler than shell globbing since protected paths are
/// meant to be broad, e.g. `src/app/**`), `?` matches one character,
/// anything else matches literally.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_match_chars(&p, &t)
}

fn glob_match_chars(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => {
            if glob_match_chars(&p[1..], t) {
                return true;
            }
            !t.is_empty() && glob_match_chars(p, &t[1..])
        }
        '?' => !t.is_empty() && glob_match_chars(&p[1..], &t[1..]),
        c => !t.is_empty() && t[0] == c && glob_match_chars(&p[1..], &t[1..]),
    }
}

pub fn matches_any_protected(path: &str, globs: &[String]) -> bool {
    globs.iter().any(|g| glob_match(g, path))
}

/// True for a path that is a test file: any directory component named `test`,
/// `tests`, `__tests__`, `spec`, `specs` or `e2e`, or a file name with
/// `.test.`, `.spec.` or `_test.` in it, or starting with `test_`.
pub(crate) fn is_test_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let Some((name, dirs)) = parts.split_last() else {
        return false;
    };
    let is_dir_name =
        |p: &str| matches!(p, "test" | "tests" | "__tests__" | "spec" | "specs" | "e2e");
    dirs.iter().any(|d| is_dir_name(d))
        || is_dir_name(name)
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_test.")
        || name.starts_with("test_")
}
