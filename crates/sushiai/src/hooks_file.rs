//! `sushiai hooks install|uninstall`: our entries in Codex's `hooks.json`, the trust for them
//! in `config.toml`, and the stable binary link the entries point at.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use sushiai_agents::codex_hooks::{self, FileOutcome};
use sushiai_agents::codex_trust::{self, TrustOutcome};
use sushiai_daemon::{ensure_bin_link, Link, ASK_WAIT_SECS};

/// `$CODEX_HOME` resolved (Codex canonicalizes it and keys hook trust by the result), else
/// `~/.codex` as is (Codex does not resolve its default home). `create` makes the directory.
fn codex_home(home: &Path, create: bool) -> Result<PathBuf> {
    let Some(dir) = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) else {
        let dir = home.join(".codex");
        if create {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
        }
        return Ok(dir);
    };
    let dir = PathBuf::from(dir);
    if create {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("cannot create {}", dir.display()))?;
    }
    Ok(dir.canonicalize().unwrap_or(dir))
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
    let codex = codex_home(&home, action == Some("install"))?;
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
