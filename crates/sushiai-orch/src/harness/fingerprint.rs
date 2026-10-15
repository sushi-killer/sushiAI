use super::outcome::RunOutcome;
use crate::policy::routing::Harness;
use crate::task::attempt::Fingerprint;

/// FNV-1a, 64 bit, hex: stable across Rust versions and platforms, which
/// `DefaultHasher` is not, and short enough to print in a report row.
fn short_hash(parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for part in parts {
        for byte in part.bytes().chain(std::iter::once(0xff)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("{:08x}", hash >> 32)
}

/// The argv without the values that differ per run or per machine: paths,
/// session ids, the MCP server wiring.
fn stable_argv(argv: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut skip_next = false;
    let mut after_resume = false;
    for (i, arg) in argv.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        let prev = i.checked_sub(1).map(|p| argv[p].as_str());
        if after_resume {
            after_resume = false;
            continue;
        }
        match arg.as_str() {
            "--mcp-config" | "--settings" | "--resume" | "-C" | "--output-schema" => {
                skip_next = true
            }
            "resume" if i == 1 => after_resume = true,
            a if a.starts_with("--image=") => {}
            a if a.starts_with("mcp_servers.") && prev == Some("-c") => {
                out.pop();
            }
            a => out.push(a),
        }
    }
    out
}

/// The run settings JSON without secrets (profile env, key helper) and
/// without the per-machine paths (hook commands, denied-read dirs).
fn stable_settings(settings: &serde_json::Value) -> String {
    let mut settings = settings.clone();
    if let Some(obj) = settings.as_object_mut() {
        for key in ["env", "apiKeyHelper", "hooks"] {
            obj.remove(key);
        }
        if let Some(sandbox) = obj.get_mut("sandbox").and_then(|s| s.as_object_mut()) {
            sandbox.remove("filesystem");
        }
        if let Some(deny) = obj
            .get_mut("permissions")
            .and_then(|p| p.get_mut("deny"))
            .and_then(|d| d.as_array_mut())
        {
            deny.retain(|entry| {
                !entry
                    .as_str()
                    .is_some_and(|rule| rule.starts_with("Read(//"))
            });
        }
    }
    settings.to_string()
}

/// The hash of the inputs orchd controls for one run. The task-specific
/// brief text is deliberately not part of it.
pub fn prompt_hash(argv: &[String], settings: Option<&serde_json::Value>) -> String {
    let version = crate::prompts::BRIEF_TEMPLATE_VERSION.to_string();
    let settings = settings.map(stable_settings).unwrap_or_default();
    let argv = stable_argv(argv).join("\u{1f}");
    short_hash(&[&version, &argv, &settings])
}

impl RunOutcome {
    /// Builds the run's fingerprint from what the harness reported. The
    /// requested alias is never a stand-in for a model the harness did not
    /// name: that would hide exactly the drift the fingerprint is for.
    pub fn build_fingerprint(
        &self,
        harness: Harness,
        harness_version: Option<String>,
        prompt_hash: String,
    ) -> Fingerprint {
        Fingerprint {
            models: self.real_models(),
            harness,
            harness_version: self.harness_version.clone().or(harness_version),
            prompt_hash,
        }
    }
}

#[cfg(test)]
#[path = "tests/fingerprint.rs"]
mod tests;
