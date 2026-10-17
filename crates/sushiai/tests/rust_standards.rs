//! ci-crates: crates-with:rust-standards.json
//! Generic crate-local Rust standards. Configuration lives beside each opted-in Cargo.toml.
#[path = "rust_standards/checker.rs"]
mod checker;
#[path = "rust_standards/tests.rs"]
mod synthetic;

use std::path::{Path, PathBuf};
use std::process::Command;

fn configured_crates() -> Result<Vec<PathBuf>, String> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("workspace root missing")?;
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args([
            "metadata",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .current_dir(workspace)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let members = metadata["workspace_members"]
        .as_array()
        .ok_or("metadata workspace_members missing")?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("metadata packages missing")?;
    let mut roots = Vec::new();
    for package in packages.iter().filter(|p| members.contains(&p["id"])) {
        let manifest = package["manifest_path"]
            .as_str()
            .ok_or("metadata manifest_path missing")?;
        let root = Path::new(manifest)
            .parent()
            .ok_or("manifest parent missing")?;
        if root.join("rust-standards.json").is_file() {
            roots.push(root.to_path_buf());
        }
    }
    if roots.is_empty() {
        return Err("no opted-in crate-local rust-standards.json files; workspace standards have not been proved".into());
    }
    roots.sort();
    Ok(roots)
}

#[test]
fn workspace_standards() {
    let roots = configured_crates().unwrap();
    let mut failures = Vec::new();
    for root in roots {
        let result = std::fs::read_to_string(root.join("rust-standards.json"))
            .map_err(|e| e.to_string())
            .and_then(|text| checker::config::Config::parse(&text, &root))
            .and_then(|config| checker::analyze(&root, Some(&config)));
        match result {
            Ok(report) => {
                for entry in report.review {
                    eprintln!("review: {entry}");
                }
                failures.extend(report.violations);
            }
            Err(error) => failures.push(format!("{}: {error}", root.display())),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "explicit report facility; never replaces workspace_standards"]
fn standards_report() {
    let root = std::env::var_os("RUST_STANDARDS_REPORT_ROOT")
        .expect("set RUST_STANDARDS_REPORT_ROOT to the source tree to measure");
    let report = checker::analyze(Path::new(&root), None).unwrap();
    let json = serde_json::to_string_pretty(&report).unwrap();
    if let Some(output) = std::env::var_os("RUST_STANDARDS_REPORT_OUTPUT") {
        std::fs::write(output, &json).unwrap();
    }
    println!("{json}");
}
