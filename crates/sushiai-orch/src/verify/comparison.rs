use crate::task::attempt::VerifyOutcome;
use std::collections::BTreeSet;

/// The pure comparison of one run with the base's run of the same command.
/// A waiver needs strong evidence; anything doubtful is `Unparsed`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Compared {
    /// Nothing fails here that does not fail on the base, every test that
    /// passes on the base passes here, and the rest of the output matches;
    /// the names failing in both.
    Covered(Vec<String>),
    /// Tests failing here and not on the base.
    New(BTreeSet<String>),
    /// Not comparable: no parsed run, a crash, an early stop or
    /// output beyond the test lines that differs.
    Unparsed,
}

pub(crate) fn compare(run: &VerifyOutcome, base: &VerifyOutcome) -> Compared {
    // A silent or unparsed run (no stored fingerprint) has nothing to compare.
    if run.output_key.is_empty() || base.output_key.is_empty() {
        return Compared::Unparsed;
    }
    // A run that stopped at its first failing unit never ran the rest.
    if run.stopped_early || base.stopped_early {
        return Compared::Unparsed;
    }
    if run.crashed || base.crashed {
        return Compared::Unparsed;
    }
    if run.failing.is_empty() || base.failing.is_empty() {
        return Compared::Unparsed;
    }
    let (now, before): (BTreeSet<&String>, BTreeSet<&String>) =
        (run.failing.iter().collect(), base.failing.iter().collect());
    let new: BTreeSet<String> = now.difference(&before).map(|n| (*n).clone()).collect();
    if !new.is_empty() {
        return Compared::New(new);
    }
    // An early stop (fail-fast) leaves out tests the base ran and passed.
    let passing: BTreeSet<&String> = run.passing.iter().collect();
    if !base.passing.iter().all(|t| passing.contains(t)) {
        return Compared::Unparsed;
    }
    // Same tests, but something else in the output changed (a new error).
    if run.output_key != base.output_key {
        return Compared::Unparsed;
    }
    Compared::Covered(run.failing.clone())
}

#[cfg(test)]
mod tests;
