//! Live check against the installed Codex, in a throwaway CODEX_HOME. Ignored
//! by default (needs the `codex` CLI, network and the owner's login):
//!   cargo test -p sushiai-agents --test codex_trust_live -- --ignored
//! It copies only `auth.json` into the temp home, never prints it, and
//! removes the temp home afterwards. Nothing global is changed. It runs
//! WITHOUT `--dangerously-bypass-hook-trust`.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use sushiai_agents::codex_trust::trust;

fn run_codex(home: &Path, work: &Path) {
    let status = Command::new("codex")
        .args(["exec", "--skip-git-repo-check", "reply ok"])
        .current_dir(work)
        .env("CODEX_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("codex is installed");
    assert!(status.success(), "codex exec failed");
}

fn setup(root: &Path, with_trust: bool) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = root.join(if with_trust {
        "home-trusted"
    } else {
        "home-untrusted"
    });
    let work = root.join(if with_trust { "work-t" } else { "work-u" });
    fs::create_dir_all(home.join("bin")).unwrap();
    fs::create_dir_all(&work).unwrap();
    let real = Path::new(&std::env::var("HOME").unwrap()).join(".codex/auth.json");
    fs::copy(real, home.join("auth.json")).expect("auth.json to copy");
    let bin = home.join("bin/sushiai");
    let marker = home.join("marker");
    fs::write(&bin, format!("#!/bin/sh\ncat > '{}'\n", marker.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let hooks = home.join("hooks.json");
    fs::write(
        &hooks,
        serde_json::json!({"hooks": {"SessionStart": [{"hooks": [{
            "type": "command",
            "command": format!("{} hook session-start --agent codex", bin.display()),
            "timeout": 10
        }]}]}})
        .to_string(),
    )
    .unwrap();
    fs::write(home.join("config.toml"), "# throwaway\n").unwrap();
    if with_trust {
        let out = trust(&home.join("config.toml"), &hooks, 1).unwrap();
        assert!(out.changed);
    }
    (home, work)
}

#[test]
#[ignore = "needs the codex CLI, network and a login"]
fn trusted_hook_fires_and_untrusted_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    let (home, work) = setup(&root, true);
    run_codex(&home, &work);
    assert!(home.join("marker").exists(), "trusted hook did not fire");
    let (home, work) = setup(&root, false);
    run_codex(&home, &work);
    assert!(!home.join("marker").exists(), "untrusted hook fired");
}
