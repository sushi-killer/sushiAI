use crate::harness::native::redaction::redact;
use std::collections::HashMap;

#[test]
fn redacts_known_values_longest_first() {
    let env = HashMap::from([
        ("TOKEN".to_string(), "invented-secret-value".to_string()),
        ("SHORT".to_string(), "xyz".to_string()),
    ]);
    assert_eq!(
        redact("invented-secret-value xyz", &env),
        "[REDACTED] [REDACTED]"
    );
}
