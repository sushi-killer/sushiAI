use serde_json::Value;
use sushiai_protocol::connector;

#[test]
fn exit_code_matches_the_vectors_file() {
    let path = format!(
        "{}/tests/connector-vectors/exit-codes.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let vectors: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("vectors"))
        .expect("parse vectors");
    assert_eq!(vectors.as_object().expect("object").len(), 1);
    assert_eq!(vectors["daemonDied"], connector::DAEMON_DIED);
}
