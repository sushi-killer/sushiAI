//! What a check printed, read from the full output of common test runners:
//! the failing tests, the passing tests, whether the run crashed, and a
//! fingerprint of everything that is not a test line. Generic: orchd never
//! knows which runner a repo uses, so the parser accepts the usual shapes of
//! cargo test and nextest, TAP (`node --test`), pytest, jest and vitest, go
//! test, and cargo-style `... FAILED` lines. A name is accepted only when the
//! line looks like a test id, so a log line is never taken for a test.

use crate::task::VerifyOutcome;
use std::collections::BTreeSet;
use std::path::Path;

/// One parsed run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub failing: Vec<String>,
    pub passing: Vec<String>,
    /// The output shows a crash, an abort or a build that did not finish.
    pub crashed: bool,
    /// A cargo run that stopped at its first failing test binary: tests of
    /// the later binaries never ran.
    pub stopped_early: bool,
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

/// Drops what changes between two runs of one test from the end of a name:
/// a timing (`3ms`, `(0.9ms)`, `1.2s`) and a count (`(2 tests)`).
fn normalize(name: &str) -> &str {
    let mut name = strip_duration(name);
    loop {
        let before = name;
        if let Some((head, last)) = name.rsplit_once(' ') {
            let digits = last.trim_end_matches(|c: char| c.is_ascii_alphabetic());
            let unit = &last[digits.len()..];
            let numeric = !digits.is_empty()
                && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
                && digits.chars().any(|c| c.is_ascii_digit());
            if numeric && matches!(unit, "ms" | "s" | "m") {
                name = head.trim_end();
            }
        }
        if name.ends_with(" tests)") || name.ends_with(" test)") {
            if let Some(open) = name.rfind(" (") {
                name = name[..open].trim_end();
            }
        }
        name = strip_duration(name);
        if name == before {
            return name;
        }
    }
}

fn clean(name: &str) -> Option<String> {
    let name = normalize(name.trim());
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
    // node --test spec reporter: `✖ name (0.9ms)`, `✔ name (0.1ms)`
    if let Some(rest) = t.strip_prefix('\u{2716}') {
        let rest = rest.trim();
        if rest.starts_with("failing tests:") {
            return None;
        }
        return clean(rest).map(Line::Failing);
    }
    if let Some(rest) = t.strip_prefix('\u{2714}') {
        return clean(rest).map(Line::Passing);
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
        return clean(rest).map(Line::Passing);
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
    let mut first_stop = false;
    let mut all_targets = false;
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
        first_stop |= t.starts_with("error: test failed, to rerun pass");
        all_targets |= t.starts_with("error: ") && t.contains(" failed:") && t.contains("target");
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
    Report {
        failing: failing.into_iter().collect(),
        passing: passing.into_iter().collect(),
        crashed,
        stopped_early: first_stop && !all_targets,
        rest_key: format!("{hash:016x}"),
    }
}

const LISTS_DIR: &str = "test-lists";

/// Writes the lists of `outcome` to `data_dir/test-lists/<key>.json` (one file
/// per distinct pair of lists) and sets its counts and `tests_key`. A write
/// that fails leaves the lists in memory and the key empty.
pub fn stash_lists(data_dir: &Path, outcome: &mut VerifyOutcome) {
    outcome.failing_count = outcome.failing.len();
    outcome.passing_count = outcome.passing.len();
    outcome.tests_key = String::new();
    if outcome.failing.is_empty() && outcome.passing.is_empty() {
        return;
    }
    let mut hash: u64 = 0xcbf29ce484222325;
    for name in outcome.failing.iter().chain(&outcome.passing) {
        name.bytes().chain([0xff]).for_each(|b| fnv(&mut hash, b));
    }
    for byte in outcome.failing.len().to_le_bytes() {
        fnv(&mut hash, byte);
    }
    let key = format!("{hash:016x}");
    let dir = data_dir.join(LISTS_DIR);
    let path = dir.join(format!("{key}.json"));
    let lists = serde_json::json!({"failing": outcome.failing, "passing": outcome.passing});
    if path.exists()
        || (std::fs::create_dir_all(&dir).is_ok()
            && std::fs::write(&path, lists.to_string()).is_ok())
    {
        outcome.tests_key = key;
    }
}

/// Reads the lists `stash_lists` wrote back into `outcome`. Unchanged when the
/// file is missing or does not match the counts: such a run is not comparable.
pub fn hydrate_lists(data_dir: &Path, outcome: &mut VerifyOutcome) {
    if outcome.tests_key.is_empty() || !outcome.failing.is_empty() || !outcome.passing.is_empty() {
        return;
    }
    let lists: Option<serde_json::Value> = std::fs::read_to_string(
        data_dir
            .join(LISTS_DIR)
            .join(format!("{}.json", outcome.tests_key)),
    )
    .ok()
    .and_then(|t| serde_json::from_str(&t).ok());
    let names = |key: &str| -> Option<Vec<String>> {
        serde_json::from_value(lists.as_ref()?.get(key)?.clone()).ok()
    };
    if let (Some(failing), Some(passing)) = (names("failing"), names("passing")) {
        if failing.len() == outcome.failing_count && passing.len() == outcome.passing_count {
            outcome.failing = failing;
            outcome.passing = passing;
        }
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
    fn a_long_run_keeps_every_name() {
        let many: String = (0..1200)
            .map(|i| format!("not ok {i} - t{i}\nok {i} - p{i}\n"))
            .collect();
        let report = parse(&many, "");
        assert_eq!(report.failing.len(), 1200);
        assert_eq!(report.passing.len(), 1200);
    }
}
