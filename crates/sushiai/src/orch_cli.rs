//! `sushiai orch <hook|ab|eval|costs|failures|evolve|gc|register|unregister>`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sushiai_daemon::Home;

use crate::daemon_client::{orch_caller, Conn};

const SERVER: &str = "sushiai-orchestrator";

/// Where the orchestration module keeps its files.
pub fn data_dir(home: &Home) -> PathBuf {
    home.dir().join("orchestrator")
}

pub fn run(mut args: impl Iterator<Item = String>) -> i32 {
    let sub = args.next();
    let rest: Vec<String> = args.collect();
    let home = Home::from_env();
    match sub.as_deref() {
        Some("hook") => {
            hook(&home, &rest);
            0
        }
        Some("gc") => gc(&rest),
        Some("register") => report(register(&home)),
        Some("unregister") => report(unregister(&home)),
        Some("eval") => sushiai_orch::eval::run(&with_data(&home, rest), &orch_caller("orch-eval")),
        Some("evolve") => sushiai_orch::evolve::run(&rest, &orch_caller("orch-evolve")),
        Some(name @ ("ab" | "costs" | "failures")) => {
            let rest = with_data(&home, rest);
            match name {
                "ab" => sushiai_orch::ab::run(&rest),
                "costs" => sushiai_orch::costs::run(&rest),
                _ => sushiai_orch::timeline::run(&rest),
            }
        }
        _ => {
            eprintln!(
                "usage: sushiai orch hook|ab|eval|costs|failures|evolve|gc|register|unregister"
            );
            2
        }
    }
}

/// The offline commands read the orchestrator folder of this home unless told otherwise.
fn with_data(home: &Home, mut args: Vec<String>) -> Vec<String> {
    if !args.iter().any(|a| a == "--data") {
        args.push("--data".into());
        args.push(data_dir(home).to_string_lossy().into_owned());
    }
    args
}

/// `orch gc [--dry-run]`: asks the daemon to remove worktrees its tasks no longer need.
fn gc(args: &[String]) -> i32 {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    match orch_caller("orch-gc")("worktrees.gc", json!({"dryRun": dry_run})) {
        Ok(v) => {
            println!("{v}");
            0
        }
        Err(e) => {
            eprintln!("sushiai orch gc: {e}");
            1
        }
    }
}

/// `orch hook stop|edit`: an orchestrated agent's Stop / edit hook. It forwards the hook JSON
/// to the running daemon and fails open: no daemon, no token or any error prints `{}`.
fn hook(home: &Home, args: &[String]) {
    let method = match args.first().map(String::as_str) {
        Some("stop") => "orch.hook.stop",
        Some("edit") => "orch.hook.edit",
        _ => return println!("{{}}"),
    };
    let flag = args
        .windows(2)
        .find(|w| w[0] == "--token")
        .map(|w| w[1].clone());
    let token = flag.or_else(|| std::env::var("SUSHIAI_ORCH_TOKEN").ok());
    let answer = token.and_then(|token| {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).ok()?;
        let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
        let mut conn = Conn::direct(&home.socket(), "orch-hook").ok()?;
        let result = conn
            .call(method, json!({"token": token, "payload": payload}))
            .ok()?;
        Some(result.to_string())
    });
    println!("{}", answer.unwrap_or_else(|| "{}".into()));
}

fn report(result: anyhow::Result<Vec<String>>) -> i32 {
    match result {
        Ok(lines) => {
            lines.iter().for_each(|l| println!("{l}"));
            0
        }
        Err(e) => {
            eprintln!("sushiai orch: {e:#}");
            1
        }
    }
}

fn user_home() -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    Ok(PathBuf::from(
        std::env::var_os("HOME").context("HOME is not set")?,
    ))
}

fn codex_home(home: &Path) -> PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|d| !d.is_empty())
        .map_or_else(|| home.join(".codex"), PathBuf::from)
}

/// Writes `text` to `path` through a temp file, keeping the old file's mode, after saving the
/// old content next to it as `<name>.sushiai-bak-<ts>`.
fn replace_file(path: &Path, text: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    let existing = path.exists();
    if existing {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let backup = path.with_file_name(format!("{name}.sushiai-bak-{ts}"));
        std::fs::copy(path, &backup)
            .with_context(|| format!("cannot back up {}", path.display()))?;
    }
    let tmp = path.with_extension("sushiai-tmp");
    std::fs::write(&tmp, text).with_context(|| format!("cannot write {}", tmp.display()))?;
    if existing {
        std::fs::set_permissions(&tmp, std::fs::metadata(path)?.permissions())?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))
}

/// Our MCP entry in Claude's `~/.claude.json`. Everything else in the file stays as it is.
fn claude_entry(file: &Path, bin: Option<&str>) -> anyhow::Result<bool> {
    use anyhow::Context;
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let mut doc: Value = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON", file.display()))?
    };
    let root = doc
        .as_object_mut()
        .context("~/.claude.json is not an object")?;
    let changed = match bin {
        Some(bin) => {
            let entry = json!({"type": "stdio", "command": bin, "args": ["mcp"], "env": {}});
            let servers = root.entry("mcpServers").or_insert_with(|| json!({}));
            let servers = servers
                .as_object_mut()
                .context("mcpServers is not an object")?;
            servers.insert(SERVER.into(), entry.clone()) != Some(entry)
        }
        None => root
            .get_mut("mcpServers")
            .and_then(Value::as_object_mut)
            .is_some_and(|s| s.remove(SERVER).is_some()),
    };
    if changed {
        replace_file(file, &(serde_json::to_string_pretty(&doc)? + "\n"))?;
    }
    Ok(changed)
}

/// Our `[mcp_servers.sushiai-orchestrator]` table in Codex's `config.toml`.
fn codex_entry(file: &Path, bin: Option<&str>) -> anyhow::Result<bool> {
    use anyhow::Context;
    use toml_edit::{value, Array, DocumentMut, Item, Table};
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let mut doc: DocumentMut = text
        .parse()
        .with_context(|| format!("{} is not valid TOML", file.display()))?;
    match bin {
        Some(bin) => {
            let mut entry = Table::new();
            entry["command"] = value(bin);
            let mut args = Array::new();
            args.push("mcp");
            entry["args"] = value(args);
            if !doc.contains_key("mcp_servers") {
                let mut servers = Table::new();
                servers.set_implicit(true);
                doc["mcp_servers"] = Item::Table(servers);
            }
            let servers = doc["mcp_servers"]
                .as_table_mut()
                .context("mcp_servers is not a table")?;
            let same = servers
                .get(SERVER)
                .and_then(Item::as_table)
                .is_some_and(|t| {
                    t.get("command").and_then(Item::as_str) == Some(bin)
                        && t.get("args").and_then(Item::as_array).is_some_and(|a| {
                            a.len() == 1 && a.get(0).and_then(|v| v.as_str()) == Some("mcp")
                        })
                        && t.len() == 2
                });
            if same {
                return Ok(false);
            }
            servers.insert(SERVER, Item::Table(entry));
        }
        None => {
            let removed = doc
                .get_mut("mcp_servers")
                .and_then(Item::as_table_mut)
                .is_some_and(|s| s.remove(SERVER).is_some());
            if !removed {
                return Ok(false);
            }
        }
    }
    replace_file(file, &doc.to_string())?;
    Ok(true)
}

fn verb(changed: bool, file: &Path, done: &str) -> String {
    if changed {
        format!("{done} in {}", file.display())
    } else {
        format!("no change in {}", file.display())
    }
}

/// `orch register`: the `sushiai-orchestrator` MCP entry and skill, for each tool that is
/// installed, and the flag that makes the daemon host the module at its next start. Writes
/// nothing else; every changed file is backed up first.
fn register(home: &Home) -> anyhow::Result<Vec<String>> {
    use anyhow::{bail, Context};
    use sushiai_daemon::{ensure_bin_link, Link};
    let user = user_home()?;
    let base = user.join(".sushiai");
    if ensure_bin_link(&base)? == Link::Kept {
        bail!(
            "{} exists and is not a symlink",
            base.join("bin/sushiai").display()
        );
    }
    let bin = base.join("bin/sushiai");
    let bin = bin.to_str().context("HOME is not valid UTF-8")?;
    let mut lines = Vec::new();
    let claude = user.join(".claude.json");
    if claude.exists() || user.join(".claude").is_dir() {
        lines.push(verb(
            claude_entry(&claude, Some(bin))?,
            &claude,
            "registered",
        ));
    }
    let codex = codex_home(&user);
    if codex.is_dir() {
        let config = codex.join("config.toml");
        lines.push(verb(
            codex_entry(&config, Some(bin))?,
            &config,
            "registered",
        ));
    }
    crate::orch_module::set_enabled(home.dir(), true)?;
    lines.push(format!("orchestrator enabled in {}", home.dir().display()));
    let data = data_dir(home);
    for written in sushiai_orch::skill::install(&user, Path::new(bin), &data) {
        lines.push(format!("skill written to {}", written.display()));
    }
    Ok(lines)
}

/// `orch unregister`: removes what `register` wrote and stops hosting the module at the next
/// daemon start.
fn unregister(home: &Home) -> anyhow::Result<Vec<String>> {
    let user = user_home()?;
    let mut lines = Vec::new();
    crate::orch_module::set_enabled(home.dir(), false)?;
    let claude = user.join(".claude.json");
    if claude.exists() {
        lines.push(verb(claude_entry(&claude, None)?, &claude, "removed"));
    }
    let codex = codex_home(&user).join("config.toml");
    if codex.exists() {
        lines.push(verb(codex_entry(&codex, None)?, &codex, "removed"));
    }
    for tool in [user.join(".claude"), codex_home(&user)] {
        let dir = tool.join("skills").join(SERVER);
        if dir.is_dir() {
            std::fs::remove_dir_all(&dir)?;
            lines.push(format!("skill removed from {}", dir.display()));
        }
    }
    Ok(lines)
}
