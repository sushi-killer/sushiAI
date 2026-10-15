use super::outcome::RunOutcome;

impl RunOutcome {
    /// The provider failure behind `error`, `None` when the run had none. The
    /// structured class wins; the error text is read only when the harness
    /// reported no class. `now_secs` turns a stated reset into a wait.
    pub fn provider_failure(&self, now_secs: i64) -> Option<ProviderFailure> {
        let error = self.error.as_deref()?;
        Some(
            self.structured_failure
                .unwrap_or_else(|| text_provider_failure(error, now_secs)),
        )
    }
}

/// The failure class in a raw error text. Only `provider_failure` calls it:
/// a harness with no structured class (Codex), or a run that died with its
/// stderr tail. Substrings only; no harness message shape is pinned here.
fn text_provider_failure(error: &str, now_secs: i64) -> ProviderFailure {
    let e = error.to_ascii_lowercase();
    let any = |needles: &[&str]| needles.iter().any(|n| e.contains(n));
    // A bare 429 is a line number or a port as often as a status.
    let limit = any(&[
        "rate limit",
        "rate_limit",
        "usage limit",
        "too many requests",
        // The Claude CLI printed this 403 for minutes while the account was
        // refused upstream; a logged-out CLI (401) still fails at once.
        "403 request not allowed",
        "429 too many",
        "status 429",
        "status code 429",
        "\"status\":429",
        "\"status\": 429",
        "http 429",
        "http/1.1 429",
        "http/2 429",
    ]);
    let retry_after_ms = stated_reset(error)
        .filter(|reset| *reset > now_secs)
        .map(|reset| ((reset - now_secs + 30) * 1000) as u64);
    if limit {
        ProviderFailure::RateLimited { retry_after_ms }
    } else if e.contains("overloaded_error") {
        ProviderFailure::Overloaded { retry_after_ms }
    } else if e.contains("credit balance is too low") {
        ProviderFailure::QuotaExhausted { retry_after_ms }
    } else if any(&["invalid x-api-key", "api error: 401"]) {
        ProviderFailure::Auth
    } else {
        ProviderFailure::Other
    }
}

/// The reset time (Unix seconds) a message states after a `|`, the shape the
/// Claude CLI has printed ("... limit reached|1760000000").
fn stated_reset(error: &str) -> Option<i64> {
    let digits: String = error
        .rsplit_once('|')?
        .1
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (digits.len() == 10).then(|| digits.parse().ok()).flatten()
}

/// Why a run failed at the provider. Callers read this enum; `retry_after_ms`
/// is the reset the run stated, when it stated one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderFailure {
    RateLimited { retry_after_ms: Option<u64> },
    QuotaExhausted { retry_after_ms: Option<u64> },
    Auth,
    Overloaded { retry_after_ms: Option<u64> },
    Other,
}
