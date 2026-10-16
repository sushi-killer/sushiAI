use crate::clip::tail_chars;
use crate::task::attempt::VerifyOutcome;
use crate::task::checks::types::Baseline;
use crate::verify::load::timed_out_under_load;
use std::time::Duration;

/// Lines a test runner prints for a failure: cargo's `... FAILED` and
/// `panicked at`, node's `not ok` / `AssertionError`, and the assertion
/// detail lines that follow a panic.
fn is_failure_line(line: &str) -> bool {
    let t = line.trim();
    t.ends_with("FAILED")
        || t.starts_with("not ok")
        || t.starts_with("---- ")
        || t.contains("panicked at")
        || t.contains("AssertionError")
        || t.starts_with("assertion ")
        || t.starts_with("left:")
        || t.starts_with("right:")
        || t.starts_with("error:")
        || t.starts_with("Error:")
        || t.starts_with("FAIL")
        || eslint_summary_has_errors(t)
}

/// eslint's `✖ N problems (M errors, ...)` with M > 0.
fn eslint_summary_has_errors(t: &str) -> bool {
    let Some(rest) = t.strip_prefix('\u{2716}') else {
        return false;
    };
    let Some((_, after)) = rest.split_once("problem") else {
        return false;
    };
    let Some((_, counts)) = after.split_once('(') else {
        return false;
    };
    let digits: String = counts.chars().take_while(char::is_ascii_digit).collect();
    counts[digits.len()..].starts_with(" error") && digits.parse::<u32>().is_ok_and(|n| n > 0)
}

/// The failure lines of one stream. A `Repository conventions failed` line
/// starts a block whose `- ` bullets (after blank lines) are kept too.
fn stream_highlights(text: &str) -> Vec<&str> {
    let mut kept = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        if in_block {
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if t.starts_with("- ") {
                kept.push(line);
                continue;
            }
            in_block = false;
        }
        if line.contains("Repository conventions failed") {
            in_block = true;
            kept.push(line);
        } else if is_failure_line(line) {
            kept.push(line);
        }
    }
    kept
}

/// The failing tests' names and assertion lines from both streams, keeping
/// the last lines when there are more than fit.
fn failure_highlights(out: &str, err: &str, max: usize) -> String {
    let mut lines = stream_highlights(out);
    lines.extend(stream_highlights(err));
    tail_chars(&lines.join("\n"), max)
}

/// Each stream's own tail plus the failure lines found anywhere in either:
/// concatenated, a noisy stderr (cargo's compile log) pushed stdout's end --
/// where test runners list what failed -- out of the kept window, so a retry
/// never saw which test broke. The failure lines come last because the
/// consumers (brief, Stop hook) keep the end of the detail.
pub(crate) fn verify_tail(stdout: &[u8], stderr: &[u8]) -> String {
    let out = String::from_utf8_lossy(stdout);
    let err = String::from_utf8_lossy(stderr);
    let highlights = failure_highlights(&out, &err, 1000);
    let mut body = match (out.trim().is_empty(), err.trim().is_empty()) {
        (true, true) => String::new(),
        (true, _) => tail_chars(&err, 3000),
        (_, true) => tail_chars(&out, 3000),
        _ => format!(
            "--- stderr (tail) ---\n{}\n--- stdout (tail) ---\n{}",
            tail_chars(&err, 900),
            tail_chars(&out, 2100)
        ),
    };
    if !highlights.is_empty() {
        body.push_str("\n--- failures ---\n");
        body.push_str(&highlights);
    }
    body
}

pub(crate) fn timed_out_tail(timeout: Duration) -> String {
    let secs = timeout.as_secs();
    if secs >= 60 && secs.is_multiple_of(60) {
        format!("timed out after {} minutes", secs / 60)
    } else {
        format!("timed out after {secs} seconds")
    }
}

pub(crate) const CANCELLED_TAIL: &str = "cancelled";

pub(crate) const SPAWN_FAILED_PREFIX: &str = "failed to run: ";

pub(crate) const INTERNAL_ERROR_PREFIX: &str = "internal error: ";

/// How a check ran on the base checkout. `None` for a cancel, which aborts
/// the baseline instead of classifying anything.
pub(crate) fn classify_baseline(outcome: &VerifyOutcome) -> Option<Baseline> {
    if outcome.tail == CANCELLED_TAIL {
        return None;
    }
    if outcome.code != Some(0) && timed_out_under_load(outcome) {
        return Some(Baseline::Env);
    }
    match outcome.code {
        Some(0) => Some(Baseline::Pass),
        Some(126 | 127) => Some(Baseline::Env),
        Some(_) => Some(Baseline::Fail),
        None if outcome.tail == CANCELLED_TAIL => None,
        None if outcome.tail.starts_with(SPAWN_FAILED_PREFIX)
            || outcome.tail.starts_with(INTERNAL_ERROR_PREFIX) =>
        {
            Some(Baseline::Env)
        }
        // A timeout is a check that does not pass.
        None => Some(Baseline::Fail),
    }
}
