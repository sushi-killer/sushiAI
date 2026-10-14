//! The failing tests a check printed, read from the full output of common test
//! runners, and a fingerprint of that output. Both are generic: orchd never
//! knows which runner a repo uses, so the parser accepts the usual shapes of
//! cargo test and nextest, TAP (`node --test`), pytest, jest and vitest, go
//! test, and any line that ends in `FAILED`.

use std::collections::BTreeSet;

/// Most names kept for one run; a run with more is capped, never dropped.
pub const MAX_NAMES: usize = 500;

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

/// The test named by one output line, `None` when the line names no failure.
fn failing_name(line: &str) -> Option<String> {
    let t = line.trim();
    // go test: `--- FAIL: TestX (0.00s)`
    if let Some(rest) = t.strip_prefix("--- FAIL:") {
        return clean(strip_duration(rest));
    }
    // TAP: `not ok 3 - name # time=1ms`
    if let Some(rest) = t.strip_prefix("not ok ") {
        let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit()).trim();
        let rest = rest.strip_prefix('-').unwrap_or(rest);
        let rest = rest.split(" # ").next().unwrap_or(rest);
        return clean(rest);
    }
    // nextest: `FAIL [   0.012s] crate bin::test` and `FAIL [..] (1/3) crate test`
    if let Some(rest) = t.strip_prefix("FAIL [") {
        let (_, after) = rest.split_once(']')?;
        let after = after.trim();
        let after = match after.strip_prefix('(') {
            Some(counter) => counter.split_once(')').map_or(after, |(_, r)| r.trim()),
            None => after,
        };
        return clean(after);
    }
    // pytest summary: `FAILED tests/a.py::test_x - AssertionError: ...`
    if let Some(rest) = t
        .strip_prefix("FAILED ")
        .or_else(|| t.strip_prefix("ERROR "))
    {
        let rest = rest.split(" - ").next().unwrap_or(rest);
        return clean(rest);
    }
    // go test package line: `FAIL\tpkg\t0.1s`, jest/vitest: `FAIL src/a.test.ts > suite > case`
    if let Some(rest) = t.strip_prefix("FAIL") {
        if rest.starts_with(['\t', ' ']) {
            let rest = rest.trim();
            let rest = rest.split('\t').next().unwrap_or(rest);
            return clean(rest);
        }
        return None;
    }
    // jest: `● suite › case`
    if let Some(rest) = t.strip_prefix('\u{25cf}') {
        let rest = rest.trim();
        if rest.is_empty() || rest.contains("Console") || rest.starts_with("Test suite failed") {
            return None;
        }
        return clean(rest);
    }
    // cargo, pytest -v, gradle, junit-ish: `<name> ... FAILED`, `<name> FAILED`
    if let Some(head) = t.strip_suffix("FAILED") {
        let head = head.trim_end();
        let head = head.strip_suffix("...").unwrap_or(head).trim_end();
        let head = head.strip_prefix("test ").unwrap_or(head);
        return clean(head);
    }
    None
}

fn clean(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty() && name != "FAILED").then(|| name.to_string())
}

/// The failing test names in `output`, at most [`MAX_NAMES`].
pub fn failing_tests(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(failing_name)
        .take(MAX_NAMES)
        .collect()
}

/// A fingerprint of `output` that ignores digits (timings, counters, pids), so
/// two runs of one deterministic failure match. FNV-1a, stable across builds.
pub fn output_key(output: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut last_digit = false;
    for byte in output.trim().bytes() {
        let digit = byte.is_ascii_digit();
        if digit && last_digit {
            continue;
        }
        last_digit = digit;
        let byte = if digit { b'#' } else { byte };
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
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
    }

    #[test]
    fn reads_tap_pytest_jest_vitest_and_go() {
        assert_eq!(
            names("ok 1 - fine\nnot ok 2 - broken # time=3ms"),
            ["broken"]
        );
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
        assert_eq!(names("Suite > case FAILED"), ["Suite > case"]);
    }

    #[test]
    fn an_output_key_ignores_digits_only() {
        assert_eq!(output_key("boom 12ms"), output_key("boom 7ms"));
        assert_ne!(output_key("boom"), output_key("bang"));
    }
}
