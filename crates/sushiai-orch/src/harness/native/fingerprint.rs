use crate::harness;
use crate::task::Fingerprint;

pub(super) fn fingerprint_of(
    outcome: &harness::RunOutcome,
    req: &harness::RunRequest<'_>,
    argv: &[String],
    codex_version: Option<String>,
) -> Fingerprint {
    let settings_json = req
        .settings_path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    outcome.build_fingerprint(
        req.harness,
        codex_version,
        harness::prompt_hash(argv, settings_json.as_ref()),
    )
}
