//! What a check printed, read from the full output of common test runners:
//! the failing tests, the passing tests, whether the run crashed, and a
//! fingerprint of everything that is not a test line. Generic: orchd never
//! knows which runner a repo uses, so the parser accepts the usual shapes of
//! cargo test and nextest, TAP (`node --test`), pytest, jest and vitest, go
//! test, and cargo-style `... FAILED` lines. A name is accepted only when the
//! line looks like a test id, so a log line is never taken for a test.

use std::collections::BTreeSet;

/// Most names kept per list for one run; a run with more is marked truncated.
pub const MAX_NAMES: usize = 500;

/// One parsed run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub failing: Vec<String>,
    pub passing: Vec<String>,
    /// A list held more than [`MAX_NAMES`] names and was cut.
    pub truncated: bool,
    /// The output shows a crash, an abort or a build that did not finish.
    pub crashed: bool,
    /// Digit-blind fingerprint of the output without its test lines.
    pub rest_key: String,
}

fn strip_duration(name: &str) -> &str {
    let name = name.trim();
    if let Some(open) = name.rfind(" (") {
        let inner = &name[open + 2..];
        if inner.ends_with(')')
            && inner[..inner.len() - 1]
                .chars()
                .all(|c| c.is_ascii_digit() || ".ms smin".contains(c))
        {
            return name[..open].trim_end();
        }
    }
    name
}

fn clean(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && name != "FAILED" && name != "ok").then(|| name.to_string())
}

/// Whether `name` has the shape of a test id: a module path, a suite
/// separator or a path to a test file. Bare words and log text do not.
fn looks_like_test_id(name: &str) -> bool {
    if name.contains("::") || name.contains(" > ") || name.contains('\u{203a}') {
        return true;
    }
    let first = name.split_whitespace().next().unwrap_or("");
    let file = first.rsplit('/').next().unwrap_or(first);
    const EXTS: [&str; 10] = [
        ".rs", ".ts", ".tsx", ".js", ".cjs", ".mjs", ".py", ".go", ".rb", ".java",
    ];
    first.contains('/') && EXTS.iter().any(|e| file.contains(e))
}

enum Line {
    Failing(String),
    Passing(String),
}

/// The test a line names, or `None`.
fn classify(line: &str) -> Option<Line> {
    let t = line.trim();
    // go test: `--- FAIL: TestX (0.00s)` / `--- PASS: TestX (0.00s)`
    if let Some(rest) = t.strip_prefix("--- FAIL:") {
        return clean(strip_duration(rest)).map(Line::Failing);
    }
    if let Some(rest) = t.strip_prefix("--- PASS:") {
        return clean(strip_duration(rest)).map(Line::Passing);
    }
    // TAP: `not ok 3 - name # time=1ms`, `ok 3 - name`
    for (prefix, failing) in [("not ok ", true), ("ok ", false)] {
        if let Some(rest) = t.strip_prefix(prefix) {
            if !rest.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit()).trim();
            let rest = rest.strip_prefix('-').unwrap_or(rest);
            let rest = rest.split(" # ").next().unwrap_or(rest);
            let name = clean(rest)?;
            return Some(if failing {
                Line::Failing(name)
            } else {
                Line::Passing(name)
            });
        }
    }
    // nextest: `FAIL [   0.012s] crate bin::test`, `PASS [...] ...`
    for (prefix, failing) in [("FAIL [", true), ("PASS [", false)] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let (_, after) = rest.split_once(']')?;
            let after = after.trim();
            let after = match after.strip_prefix('(') {
                Some(counter) => counter.split_once(')').map_or(after, |(_, r)| r.trim()),
                None => after,
            };
            let name = clean(after)?;
            return Some(if failing {
                Line::Failing(name)
            } else {
                Line::Passing(name)
            });
        }
    }
    // cargo: `test a::b ... FAILED` / `test a::b ... ok`
    if let Some(rest) = t.strip_prefix("test ") {
        if let Some(name) = rest.strip_suffix(" ... FAILED") {
            return clean(name).map(Line::Failing);
        }
        if let Some(name) = rest.strip_suffix(" ... ok") {
            return clean(name).map(Line::Passing);
        }
    }
    // pytest: `FAILED tests/a.py::test_x - msg`, `PASSED tests/a.py::t`,
    // `tests/a.py::test_x PASSED|FAILED`
    for (prefix, failing) in [("FAILED ", true), ("ERROR ", true), ("PASSED ", false)] {
        if let Some(rest) = t.strip_prefix(prefix) {
            let name = clean(rest.split(" - ").next().unwrap_or(rest))?;
            if !looks_like_test_id(&name) {
                return None;
            }
            return Some(if failing {
                Line::Failing(name)
            } else {
                Line::Passing(name)
            });
        }
    }
    if let Some(head) = t.strip_suffix(" PASSED") {
        let name = clean(head)?;
        return looks_like_test_id(&name).then_some(Line::Passing(name));
    }
    if let Some(head) = t.strip_suffix(" FAILED") {
        let name = clean(head)?;
        return looks_like_test_id(&name).then_some(Line::Failing(name));
    }
    // go package line `FAIL\tpkg\t0.1s`; jest/vitest `FAIL src/a.test.ts > suite`
    if let Some(rest) = t.strip_prefix("FAIL") {
        if !rest.starts_with(['\t', ' ']) {
            return None;
        }
        let rest = rest.trim();
        if rest.contains('\t') {
            let pkg = rest.split('\t').next().unwrap_or(rest);
            return clean(pkg).map(Line::Failing);
        }
        let name = clean(rest)?;
        return looks_like_test_id(&name).then_some(Line::Failing(name));
    }
    // jest: `● suite › case`, `✓ case (3 ms)`
    if let Some(rest) = t.strip_prefix('\u{25cf}') {
        let name = clean(rest)?;
        if name.contains("Console") || !looks_like_test_id(&name) {
            return None;
        }
        return Some(Line::Failing(name));
    }
    if let Some(rest) = t
        .strip_prefix('\u{2713}')
        .or_else(|| t.strip_prefix('\u{221a}'))
    {
        return clean(strip_duration(rest)).map(Line::Passing);
    }
    None
}

const CRASH_MARKERS: [&str; 12] = [
    "SIGSEGV",
    "SIGABRT",
    "SIGKILL",
    "(signal:",
    "process didn't exit successfully",
    "could not compile",
    "Segmentation fault",
    "core dumped",
    "fatal runtime error",
    "thread 'main' panicked",
    "error: aborting due to",
    "command not found",
];

fn fnv(hash: &mut u64, byte: u8) {
    *hash ^= u64::from(byte);
    *hash = hash.wrapping_mul(0x100000001b3);
}

/// Parses the whole `output` of one run. `cwd` is masked in the fingerprint,
/// so the same failure in two directories gives the same key.
pub fn parse(output: &str, cwd: &str) -> Report {
    let mut failing = BTreeSet::new();
    let mut passing = BTreeSet::new();
    let mut crashed = false;
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut last_digit = false;
    for line in output.lines() {
        match classify(line) {
            Some(Line::Failing(name)) => {
                failing.insert(name);
                continue;
            }
            Some(Line::Passing(name)) => {
                passing.insert(name);
                continue;
            }
            None => {}
        }
        let t = line.trim();
        if t.is_empty()
            || t.starts_with("Running ")
            || t.starts_with("Compiling ")
            || t.starts_with("Finished ")
            || t.starts_with("Doc-tests ")
        {
            continue;
        }
        if CRASH_MARKERS.iter().any(|m| t.contains(m)) {
            crashed = true;
        }
        let masked;
        let t = if cwd.is_empty() {
            t
        } else {
            masked = t.replace(cwd, "<cwd>");
            masked.as_str()
        };
        for byte in t.bytes().chain(std::iter::once(b'\n')) {
            let digit = byte.is_ascii_digit();
            if digit && last_digit {
                continue;
            }
            last_digit = digit;
            fnv(&mut hash, if digit { b'#' } else { byte });
        }
    }
    let truncated = failing.len() > MAX_NAMES || passing.len() > MAX_NAMES;
    Report {
        failing: failing.into_iter().take(MAX_NAMES).collect(),
        passing: passing.into_iter().take(MAX_NAMES).collect(),
        truncated,
        crashed,
        rest_key: format!("{hash:016x}"),
    }
}

/// The failing test names in `output` (a stored tail, no directory known).
pub fn failing_tests(output: &str) -> BTreeSet<String> {
    parse(output, "").failing.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(output: &str) -> Vec<String> {
        failing_tests(output).into_iter().collect()
    }

    #[test]
    fn reads_cargo_and_nextest() {
        let out =
            "test a::ok ... ok\ntest a::bad ... FAILED\n        FAIL [   0.012s] crate b::worse\n";
        assert_eq!(names(out), vec!["a::bad", "crate b::worse"]);
        assert_eq!(parse(out, "").passing, vec!["a::ok"]);
    }

    #[test]
    fn reads_tap_pytest_jest_vitest_and_go() {
        assert_eq!(
            names("ok 1 - fine\nnot ok 2 - broken # time=3ms"),
            ["broken"]
        );
        assert_eq!(parse("ok 1 - fine", "").passing, vec!["fine"]);
        assert_eq!(
            names("FAILED tests/a.py::test_x - AssertionError: no\nERROR tests/b.py::test_y"),
            ["tests/a.py::test_x", "tests/b.py::test_y"]
        );
        assert_eq!(
            names(" FAIL  src/a.test.ts > suite > case"),
            ["src/a.test.ts > suite > case"]
        );
        assert_eq!(
            names("  \u{25cf} suite \u{203a} case"),
            ["suite \u{203a} case"]
        );
        assert_eq!(
            names("--- FAIL: TestX/sub (0.00s)\nFAIL\tpkg/x\t0.1s\nFAIL"),
            ["TestX/sub", "pkg/x"]
        );
        assert_eq!(names("pkg/a_test.py::case FAILED"), ["pkg/a_test.py::case"]);
        assert_eq!(
            parse("--- PASS: TestY (0.00s)\n\u{2713} ok case (3 ms)", "").passing,
            vec!["TestY", "ok case"]
        );
    }

    #[test]
    fn log_lines_are_not_tests() {
        assert!(names("ERROR connecting to db\nFAIL to connect\nbuild FAILED").is_empty());
    }

    #[test]
    fn a_crash_is_marked() {
        assert!(parse("error: could not compile `x`", "").crashed);
        assert!(parse("process didn't exit successfully (signal: 11, SIGSEGV)", "").crashed);
        assert!(!parse("test a::b ... FAILED", "").crashed);
    }

    #[test]
    fn the_key_ignores_digits_and_the_directory_only() {
        let key = |o: &str, d: &str| parse(o, d).rest_key;
        assert_eq!(key("boom 12ms", ""), key("boom 7ms", ""));
        assert_eq!(key("at /a/b/x.rs", "/a/b"), key("at /c/x.rs", "/c"));
        assert_ne!(key("boom", ""), key("bang", ""));
        // Test lines are not part of the key.
        assert_eq!(key("not ok 1 - a\nboom", ""), key("not ok 2 - b\nboom", ""));
    }

    #[test]
    fn more_than_the_cap_marks_the_run_truncated() {
        let many: String = (0..=MAX_NAMES)
            .map(|i| format!("not ok {i} - t{i}\n"))
            .collect();
        let report = parse(&many, "");
        assert!(report.truncated);
        assert_eq!(report.failing.len(), MAX_NAMES);
    }
}
