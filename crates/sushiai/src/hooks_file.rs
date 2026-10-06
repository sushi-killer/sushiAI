//! `sushiai hooks install|uninstall`: our entries in Codex's `hooks.json`, the trust for them
//! in `config.toml`, and the stable binary link the entries point at.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use sushiai_agents::codex_hooks::{self, FileOutcome};
use sushiai_agents::codex_trust::{self, TrustOutcome};
use sushiai_daemon::{ensure_bin_link, Link, ASK_WAIT_SECS};

/// `$CODEX_HOME`, else `~/.codex`. Not canonicalized: Codex keys its trust by this path.
fn codex_home(home: &Path) -> PathBuf {
    match std::env::var_os("CODEX_HOME") {
        Some(dir) if !dir.is_empty() => dir.into(),
        _ => home.join(".codex"),
    }
}

fn hooks_line(verb: &str, file: &Path, outcome: &FileOutcome) -> String {
    match (outcome.changed, &outcome.backup) {
        (true, Some(b)) => format!("{verb} in {} (backup {})", file.display(), b.display()),
        (true, None) => format!("{verb} in {}", file.display()),
        (false, _) => format!("no change in {}", file.display()),
    }
}

fn trust_line(verb: &str, file: &Path, outcome: &TrustOutcome) -> String {
    match (outcome.changed, &outcome.backup) {
        (true, Some(b)) => format!("{verb} in {} (backup {})", file.display(), b.display()),
        (true, None) => format!("{verb} in {}", file.display()),
        (false, _) => format!("no change in {}", file.display()),
    }
}

pub fn run(action: Option<&str>) -> Result<()> {
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
    let codex = codex_home(&home);
    let hooks = codex.join("hooks.json");
    let config = codex.join("config.toml");
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    match action {
        Some("install") => {
            let base = home.join(".sushiai");
            let link = ensure_bin_link(&base)?;
            if link == Link::Kept {
                bail!(
                    "{} exists and is not a symlink; remove it and run again",
                    base.join("bin/sushiai").display()
                );
            }
            let bin = base.join("bin/sushiai");
            let bin = bin.to_str().context("HOME is not valid UTF-8")?;
            println!("bin link: {link:?} {bin}");
            std::fs::create_dir_all(&codex)
                .with_context(|| format!("cannot create {}", codex.display()))?;
            let installed = codex_hooks::install(&hooks, bin, ASK_WAIT_SECS, ts)?;
            println!(
                "hooks.json: {}",
                hooks_line("installed", &hooks, &installed)
            );
            let trusted = codex_trust::trust(&config, &hooks, ts)?;
            println!("config.toml: {}", trust_line("trusted", &config, &trusted));
        }
        Some("uninstall") => {
            // Trust first: it needs our handlers still in hooks.json to name its keys.
            if hooks.exists() {
                let untrusted = codex_trust::untrust(&config, &hooks, ts)?;
                println!(
                    "config.toml: {}",
                    trust_line("untrusted", &config, &untrusted)
                );
            }
            let removed = codex_hooks::uninstall(&hooks, ts)?;
            println!("hooks.json: {}", hooks_line("removed", &hooks, &removed));
        }
        _ => bail!("usage: sushiai hooks install | uninstall"),
    }
    Ok(())
}
