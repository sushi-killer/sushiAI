use serde_json::Value;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Returns why a native Seatbelt profile cannot be applied in this process.
/// macOS refuses to nest `sandbox-exec` profiles, which can happen when a
/// task's native check runs a test binary that starts its own sandboxed checks.
pub fn native_sandbox_unavailable_reason() -> Option<&'static str> {
    static REASON: OnceLock<Option<&'static str>> = OnceLock::new();
    *REASON.get_or_init(|| {
        if !cfg!(target_os = "macos") {
            return None;
        }
        (!Command::new("sandbox-exec")
            .args(["-p", "(version 1)(allow default)", "/usr/bin/true"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success()))
        .then_some("sandbox-exec cannot apply a profile (nested or unavailable Seatbelt)")
    })
}

/// Switch a settings payload to host mode only when native Seatbelt cannot
/// apply. The integration and unit test fixtures share this single decision.
pub fn fit_sandbox(settings: &mut Value) {
    if let Some(reason) = native_sandbox_unavailable_reason() {
        eprintln!("Using host sandbox fixture because {reason}");
        settings["sandbox"] = serde_json::json!("host");
    }
}
