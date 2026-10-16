use super::*;

#[test]
fn plan_gate_flags_a_dropped_shared_check_named_in_the_request() {
    let request = "Make it so, then run `npm run ci` and `echo hi`.";
    let shared = |c: &str| c == "npm run ci";
    let verify = vec!["cargo test -p x --lib".to_string()];
    let dropped = shared_check_findings(request, &verify, &[], &shared);
    assert_eq!(dropped.len(), 1, "{dropped:?}");
    assert_eq!(dropped[0].kind, "shared-check");
    assert_eq!(dropped[0].severity, Severity::Critical);
    let final_verify = vec!["npm run ci".to_string()];
    assert!(shared_check_findings(request, &[], &final_verify, &shared).is_empty());
}
