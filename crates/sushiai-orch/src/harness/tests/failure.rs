use super::claude_stream;
use crate::harness::{replay_events, ProviderFailure, RunOutcome};
use crate::policy::routing::Harness;

fn claude_error(category: Option<&str>, status: Option<u64>, text: &str) -> RunOutcome {
    let mut lines = Vec::new();
    if let Some(category) = category {
        lines.push(format!(
            r#"{{"type":"assistant","error":"{category}","message":{{"content":[]}}}}"#
        ));
    }
    let status = status.map_or(String::new(), |s| format!(r#","api_error_status":{s}"#));
    lines.push(format!(
        r#"{{"type":"result","subtype":"success","is_error":true,"result":"{text}"{status}}}"#
    ));
    claude_stream(&lines.iter().map(String::as_str).collect::<Vec<_>>())
}

#[test]
fn a_claude_error_names_its_failure_class_from_the_stream() {
    let now = 1_760_000_000;
    let class = |o: RunOutcome| o.provider_failure(now);
    let rate = ProviderFailure::RateLimited {
        retry_after_ms: None,
    };
    assert_eq!(
        class(claude_error(Some("rate_limit"), None, "x")),
        Some(rate)
    );
    assert_eq!(class(claude_error(None, Some(429), "x")), Some(rate));
    assert_eq!(
        class(claude_error(None, Some(529), "x")),
        Some(ProviderFailure::Overloaded {
            retry_after_ms: None
        })
    );
    assert_eq!(
        class(claude_error(Some("billing_error"), None, "x")),
        Some(ProviderFailure::QuotaExhausted {
            retry_after_ms: None
        })
    );
    assert_eq!(
        class(claude_error(Some("authentication_failed"), None, "x")),
        Some(ProviderFailure::Auth)
    );
    assert_eq!(
        class(claude_error(None, Some(401), "x")),
        Some(ProviderFailure::Auth)
    );
    assert_eq!(
        class(claude_error(Some("server_error"), None, "boom")),
        Some(ProviderFailure::Other)
    );
}

#[test]
fn the_structured_class_wins_over_the_error_text() {
    let now = 1_760_000_000;
    let out = claude_error(None, Some(401), "rate limit exceeded");
    assert_eq!(out.provider_failure(now), Some(ProviderFailure::Auth));
    // A 403 the stream does not classify keeps the text's verdict.
    let out = claude_error(None, Some(403), "403 Request not allowed");
    assert_eq!(
        out.provider_failure(now),
        Some(ProviderFailure::RateLimited {
            retry_after_ms: None
        })
    );
}

#[test]
fn a_codex_error_is_classified_from_its_text_with_the_stated_reset() {
    let now = 1_760_000_000;
    let codex = |message: &str| {
        let line = serde_json::json!({"type": "turn.failed", "error": {"message": message}});
        replay_events(Harness::Codex, &line.to_string())
    };
    assert_eq!(
        codex("usage limit reached|1760003600").provider_failure(now),
        Some(ProviderFailure::RateLimited {
            retry_after_ms: Some(3630 * 1000)
        })
    );
    assert_eq!(
        codex("usage limit reached|1759999000").provider_failure(now),
        Some(ProviderFailure::RateLimited {
            retry_after_ms: None
        })
    );
    assert_eq!(
        codex("overloaded_error").provider_failure(now),
        Some(ProviderFailure::Overloaded {
            retry_after_ms: None
        })
    );
    assert_eq!(
        codex("Your credit balance is too low").provider_failure(now),
        Some(ProviderFailure::QuotaExhausted {
            retry_after_ms: None
        })
    );
    assert_eq!(
        codex("Failed to authenticate. API Error: 401 invalid x-api-key").provider_failure(now),
        Some(ProviderFailure::Auth)
    );
    assert_eq!(
        codex("model not supported").provider_failure(now),
        Some(ProviderFailure::Other)
    );
    assert_eq!(RunOutcome::default().provider_failure(now), None);
    // A TypeError at cli.js:429:15 is a line number, not a status.
    assert_eq!(
        codex("TypeError at cli.js:429:15").provider_failure(now),
        Some(ProviderFailure::Other)
    );
}
