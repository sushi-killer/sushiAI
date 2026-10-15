use super::default_prices;
use crate::policy::pricing::price_for;

#[test]
fn codex_cost_bills_cached_tokens_at_the_cached_rate_once() {
    let p = default_prices()["gpt-5.6-luna"];
    // 359,428 input of which 324,864 cached, 3,869 output (a real run).
    let cost = p.codex_cost(359_428, 324_864, 3_869);
    let expected = (34_564.0 * 0.20 + 324_864.0 * 0.02 + 3_869.0 * 1.20) / 1e6;
    assert!((cost - expected).abs() < 1e-9, "{cost}");
}

#[test]
fn claude_prices_match_a_dated_model_id_and_the_longest_family() {
    let prices = default_prices();
    let haiku = price_for(&prices, "claude-haiku-4-5-20251001").unwrap();
    assert_eq!(haiku.input, 1.0);
    assert_eq!(price_for(&prices, "claude-sonnet-5-5").unwrap().input, 2.0);
    assert_eq!(
        price_for(&prices, "claude-opus-5-5-20260901")
            .unwrap()
            .output,
        20.0
    );
    assert!(price_for(&prices, "claude-opus-5").is_none());
    assert!(price_for(&prices, "claude-sonnet-50").is_none());
    let opus = prices["claude-opus-5-5"];
    let cost = opus.claude_cost(1_000_000, 1_000_000, 1_000_000, 1_000_000);
    assert!((cost - (4.0 + 5.0 + 0.20 + 20.0)).abs() < 1e-9, "{cost}");
    // Codex prices keep no cache-write price and stay off the wire.
    let v = serde_json::to_value(prices["gpt-5.6-luna"]).unwrap();
    assert!(v.get("cacheWrite").is_none());
    assert_eq!(serde_json::to_value(opus).unwrap()["cacheWrite"], 5.0);
}
