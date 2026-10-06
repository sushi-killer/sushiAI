//! `sushiai orch <hook|ab|eval|costs|failures|evolve|gc|register|unregister>`, plus the small
//! daemon client the orchestration commands and `sushiai mcp` share.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sushiai_daemon::Home;
use sushiai_protocol::{encode, Decoder, Frame, Message, Request, PROTOCOL_VERSION};

const SERVER: &str = "sushiai-orchestrator";

/// Where the orchestration module keeps its files.
pub fn data_dir(home: &Home) -> PathBuf {
    home.dir().join("orchestrator")
}

pub enum CallError {
    /// The connection ended before an answer; a fresh one may work.
    Closed(String),
    /// The daemon answered with an error.
    Rpc(String),
}

impl CallError {
    pub fn message(self) -> String {
        match self {
            CallError::Closed(e) | CallError::Rpc(e) => e,
        }
    }
}

/// One blocking connection to the daemon, after `hello`.
pub struct Conn {
    reader: Box<dyn Read>,
    writer: Box<dyn Write>,
    decoder: Decoder,
    next_id: u64,
    child: Option<Child>,
}

impl Conn {
    /// Through `sushiai proxy`, which starts the daemon when nobody answers.
    pub fn via_proxy(client: &str) -> Result<Conn, String> {
        let exe = std::env::current_exe().map_err(|e| format!("cannot find sushiai: {e}"))?;
        let mut child = Command::new(exe)
            .arg("proxy")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("cannot start sushiai proxy: {e}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("sushiai proxy has no pipes".into());
        };
        Conn::hello(Box::new(stdout), Box::new(stdin), Some(child), client)
    }

    /// Straight to the socket; never starts a daemon.
    pub fn direct(socket: &Path, client: &str) -> Result<Conn, String> {
        let stream = UnixStream::connect(socket).map_err(|e| e.to_string())?;
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        Conn::hello(Box::new(reader), Box::new(stream), None, client)
    }

    fn hello(
        reader: Box<dyn Read>,
        writer: Box<dyn Write>,
        child: Option<Child>,
        client: &str,
    ) -> Result<Conn, String> {
        let mut conn = Conn {
            reader,
            writer,
            decoder: Decoder::new(),
            next_id: 1,
            child,
        };
        let params = json!({"protocol": PROTOCOL_VERSION, "client": client});
        match conn.call("hello", params) {
            Ok(_) => Ok(conn),
            Err(CallError::Closed(e) | CallError::Rpc(e)) => Err(e),
        }
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, CallError> {
        let id = self.next_id;
        self.next_id += 1;
        let frame = Request::new(id, method, params).frame();
        self.writer
            .write_all(&encode(&frame))
            .and_then(|()| self.writer.flush())
            .map_err(|e| CallError::Closed(format!("daemon write failed: {e}")))?;
        let mut buf = [0u8; 8192];
        loop {
            let n = match self.reader.read(&mut buf) {
                Ok(0) => return Err(CallError::Closed("the daemon closed the connection".into())),
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(CallError::Closed(format!("daemon read failed: {e}"))),
            };
            let frames = self
                .decoder
                .push(&buf[..n])
                .map_err(|e| CallError::Closed(format!("bad frame from the daemon: {e}")))?;
            for frame in frames {
                // Notifications and output frames are not answers.
                let Frame::Json(text) = frame else { continue };
                if let Ok(Message::Response(r)) = Message::parse(&text) {
                    if r.id == json!(id) {
                        return match r.error {
                            Some(e) => Err(CallError::Rpc(e.message)),
                            None => Ok(r.result.unwrap_or(Value::Null)),
                        };
                    }
                }
            }
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        // The proxy ends when its stdin does; do not leave it behind if it does not.
        if let Some(mut child) = self.child.take() {
            let _ = std::mem::replace(&mut self.writer, Box::new(std::io::sink()));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
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
        Some("unregister") => report(unregister()),
        Some(name @ ("ab" | "eval" | "costs" | "failures" | "evolve")) => {
            let rest = with_data(&home, rest);
            match name {
                "ab" => sushiai_orch::ab::run(&rest),
                "eval" => sushiai_orch::eval::run(&rest),
                "costs" => sushiai_orch::costs::run(&rest),
                "evolve" => sushiai_orch::evolve::run(&rest),
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
    let result = Conn::via_proxy("orch-gc").and_then(|mut conn| {
        conn.call("orch.worktrees.gc", json!({"dryRun": dry_run}))
            .map_err(CallError::message)
    });
    match result {
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
/// installed. Writes nothing else; every changed file is backed up first.
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
    let data = data_dir(home);
    for written in sushiai_orch::skill::install(&user, Path::new(bin), &data) {
        lines.push(format!("skill written to {}", written.display()));
    }
    Ok(lines)
}

/// `orch unregister`: removes what `register` wrote.
fn unregister() -> anyhow::Result<Vec<String>> {
    let user = user_home()?;
    let mut lines = Vec::new();
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
