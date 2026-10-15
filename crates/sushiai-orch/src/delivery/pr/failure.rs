use std::time::Duration;

/// Pauses between the tries to open a pull request: five tries over about
/// half an hour. `ORCHD_PR_RETRY_MS` (comma separated) replaces them in tests.
pub(crate) fn retry_delays() -> Vec<Duration> {
    if let Ok(v) = std::env::var("ORCHD_PR_RETRY_MS") {
        return v
            .split(',')
            .filter_map(|ms| ms.trim().parse().ok())
            .map(Duration::from_millis)
            .collect();
    }
    [60, 240, 600, 900]
        .into_iter()
        .map(Duration::from_secs)
        .collect()
}

/// What a failed attempt means for the retry.
pub(crate) enum PrFailure {
    /// May pass on its own: retried, up to the bound.
    Transient,
    /// Can only repeat: one try and one owner notice. `hint` is what the
    /// owner does about it.
    Permanent(&'static str),
    /// `gh` knows no GitHub host among the repo's remotes: a note, no notice.
    NoGithubHost,
}

/// An orchd refusal is decided by its variant; an outside failure is read by
/// [`classify_external`], the one place that reads git or gh text.
pub(crate) fn failure_of(error: &crate::delivery::pr::error::PrError) -> PrFailure {
    match error {
        crate::delivery::pr::error::PrError::Refused(refusal) if refusal.permanent() => {
            PrFailure::Permanent("")
        }
        crate::delivery::pr::error::PrError::Refused(_) => PrFailure::Transient,
        crate::delivery::pr::error::PrError::External(text) => classify_external(text),
    }
}

/// Whether a git or gh failure can only repeat (auth, a missing remote, a
/// protected branch, a refused name) or may pass on its own (network, a busy
/// service, a rate limit, a timeout). `gh` exits 1 for every failure, so its
/// message is the only signal. A transient marker wins: a 403 that says "rate
/// limit" is not a refusal.
pub(crate) fn classify_external(error: &str) -> PrFailure {
    const NO_HOST: [&str; 2] = ["known github host", "no github host"];
    const TRANSIENT: [&str; 14] = [
        "rate limit",
        "secondary rate",
        "timed out",
        "temporarily",
        "try again",
        "http 500",
        "http 502",
        "http 503",
        "http 504",
        "connection reset",
        "connection refused",
        "could not resolve",
        "network is unreachable",
        "early eof",
    ];
    const PERMANENT: [&str; 16] = [
        "not logged in",
        "gh auth login",
        "authentication failed",
        "bad credentials",
        "permission denied",
        "permission to",
        "resource not accessible",
        "protected branch",
        "gh006",
        "gh013",
        "does not appear to be a git repository",
        "no such remote",
        "no such file or directory",
        "command not found",
        "repository not found",
        "refusing to allow",
    ];
    let e = error.to_lowercase();
    if NO_HOST.iter().any(|h| e.contains(h)) {
        return PrFailure::NoGithubHost;
    }
    if TRANSIENT.iter().any(|t| e.contains(t)) {
        return PrFailure::Transient;
    }
    let refused =
        PERMANENT.iter().any(|p| e.contains(p)) || e.contains("http 401") || e.contains("http 404");
    if !refused {
        return PrFailure::Transient;
    }
    if e.contains("refusing to allow") {
        PrFailure::Permanent(
            " The token needs the `workflow` scope, or the owner pushes the branch.",
        )
    } else {
        PrFailure::Permanent("")
    }
}
