use super::native_sandbox;
use std::path::{Path, PathBuf};

/// Whether this process cannot apply a `sandbox-exec` profile. macOS refuses
/// to nest one inside another (`sandbox_apply: Operation not permitted`),
/// which is exactly where this suite runs when the orchestrator's own Native-sandboxed
/// verify runs `cargo test`: every daemon here would then fail each
/// verify command it wraps in `sandbox-exec`, so no task could ever pass.
pub fn native_sandbox_unavailable() -> bool {
    native_sandbox::native_sandbox_unavailable_reason().is_some()
}

/// Disposable package-manager state with read-only access to the installed
/// toolchains and dependency sources. Native profiles allow writing the new
/// homes, never the symlink targets in the owner's homes.
pub fn sandbox_toolchain_homes(user: &Path) -> (PathBuf, PathBuf) {
    let owner_home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    let cargo_source = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| owner_home.join(".cargo"));
    let rustup_source = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| owner_home.join(".rustup"));
    let cargo = user.join(".cargo");
    let rustup = user.join(".rustup");
    std::fs::create_dir_all(&cargo).unwrap();
    std::fs::create_dir_all(&rustup).unwrap();
    for (source, target, names) in [
        (&cargo_source, &cargo, &["bin", "registry", "git"][..]),
        (&rustup_source, &rustup, &["toolchains"][..]),
    ] {
        for name in names {
            if source.join(name).exists() {
                std::os::unix::fs::symlink(source.join(name), target.join(name)).unwrap();
            }
        }
    }
    if rustup_source.join("settings.toml").is_file() {
        std::fs::copy(
            rustup_source.join("settings.toml"),
            rustup.join("settings.toml"),
        )
        .unwrap();
    }
    (cargo, rustup)
}
