//! `sushiai projects | hosts | routines | routine | ask | propose`: the commands a routine
//! builder and a routine run use. Everything that needs a routine goes to the daemon's
//! `routines.*` methods; reading projects and hosts uses the daemon's catalog. `skills` reads
//! this machine's skill folders and needs no daemon.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sushiai_daemon::Home;
use sushiai_protocol::{encode, method, Decoder, Frame, Hello, Message, Request, PROTOCOL_VERSION};
use sushiai_routines::help::HELP;
use sushiai_routines::service::ask_kind;
use sushiai_routines::store::ask_inbox;

use crate::daemon_client::Conn;
use crate::routines_module;

/// The text printed for an ask in a dry run.
pub const DRY_RUN_ASK: &str = "skipped (dry run)";

pub fn run(command: &str, args: impl Iterator<Item = String>) -> i32 {
    let rest: Vec<String> = args.collect();
    let result = match command {
        "projects" => projects(&rest),
        "hosts" => hosts(&rest),
        "skills" => skills(&rest),
        "routines" => routines(&rest),
        "routine" => routine(&rest),
        "ask" => ask(&rest),
        "propose" => propose(&rest),
        _ => Err(format!("unknown command {command}")),
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("sushiai {command}: {message}");
            1
        }
    }
}

// ----- daemon access -----

/// A connection to the daemon of this host: the socket a session was given, else through
/// `sushiai proxy`, which starts the daemon when nobody answers.
fn connect() -> Result<Conn, String> {
    match std::env::var_os("SUSHIAI_SOCKET").filter(|s| !s.is_empty()) {
        Some(socket) => Conn::direct(Path::new(&socket), "routines-cli"),
        None => Conn::via_proxy("routines-cli"),
    }
}

pub fn call(method: &str, params: Value) -> Result<Value, String> {
    connect()?
        .call(method, params)
        .map_err(|e| e.message())
        .map_err(|m| {
            if m.contains("unknown method") && method.starts_with("routines.") {
                "the routines module is not enabled on this host (run `sushiai routines register` \
                 and restart the daemon)"
                    .to_string()
            } else {
                m
            }
        })
}

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn value_of<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].as_str())
}

fn values_of(args: &[String], name: &str) -> Vec<String> {
    args.windows(2)
        .filter(|w| w[0] == name)
        .map(|w| w[1].clone())
        .collect()
}

// ----- projects and hosts -----

/// The projects of a `catalog.get` snapshot that are not deleted.
fn live_projects(snapshot: &Value) -> Vec<&Value> {
    snapshot["projects"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["deleted"] != true)
        .collect()
}

/// One project with the first lines of its README or AGENTS.md.
fn project_show(snapshot: &Value, id: &str) -> Result<Value, String> {
    let p = live_projects(snapshot)
        .into_iter()
        .find(|p| p["id"] == id || p["name"] == id)
        .ok_or_else(|| format!("no project {id}"))?;
    let mut out = p.clone();
    out["excerpt"] = json!(excerpt(p));
    Ok(out)
}

/// `sushiai projects [show ID]` as data: what the MCP door returns too.
pub fn projects_data(snapshot: &Value, show: Option<&str>) -> Result<Value, String> {
    match show {
        Some(id) => project_show(snapshot, id),
        None => Ok(json!(live_projects(snapshot))),
    }
}

fn projects(args: &[String]) -> Result<i32, String> {
    let snapshot = call("catalog.get", json!({}))?;
    let list = live_projects(&snapshot);
    let json_out = flag(args, "--json");
    if args.first().map(String::as_str) == Some("show") {
        let id = args.get(1).ok_or("usage: sushiai projects show ID")?;
        let out = project_show(&snapshot, id)?;
        if json_out {
            print_json(&out);
        } else {
            println!("id:      {}", out["id"].as_str().unwrap_or_default());
            println!("name:    {}", out["name"].as_str().unwrap_or_default());
            println!("purpose: {}", out["purpose"].as_str().unwrap_or("-"));
            for f in out["folders"].as_array().into_iter().flatten() {
                println!(
                    "folder:  {}  {}",
                    f["host"].as_str().unwrap_or_default(),
                    f["path"].as_str().unwrap_or_default()
                );
            }
            if let Some(e) = out["excerpt"].as_str().filter(|e| !e.is_empty()) {
                println!("\n{e}");
            }
        }
        return Ok(0);
    }
    if json_out {
        print_json(&json!(list));
        return Ok(0);
    }
    for p in list {
        let hosts: Vec<&str> = p["folders"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f["host"].as_str())
            .collect();
        println!(
            "{}\t{}\t{}\t{}",
            p["id"].as_str().unwrap_or_default(),
            p["name"].as_str().unwrap_or_default(),
            hosts.join(","),
            p["purpose"].as_str().unwrap_or("-")
        );
    }
    Ok(0)
}

/// The first lines of the project's README or AGENTS.md, from a folder on this host.
fn excerpt(project: &Value) -> String {
    for folder in project["folders"].as_array().into_iter().flatten() {
        let Some(path) = folder["path"].as_str() else {
            continue;
        };
        for name in ["AGENTS.md", "README.md"] {
            if let Ok(text) = std::fs::read_to_string(Path::new(path).join(name)) {
                let cut: String = text.lines().take(40).collect::<Vec<_>>().join("\n");
                return cut.chars().take(2000).collect();
            }
        }
    }
    String::new()
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

// ----- skills -----

/// The first line of the `description:` in a SKILL.md frontmatter (a folded `>-` or `|` block
/// gives its first text line). Nothing else of the file is read out.
fn skill_description(text: &str) -> String {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    let mut found = false;
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let value = match line.strip_prefix("description:") {
            Some(v) => {
                found = true;
                v.trim()
            }
            None if found && line.starts_with([' ', '\t']) => line.trim(),
            None if found => break,
            None => continue,
        };
        if value.is_empty() || value.chars().all(|c| ">|-+".contains(c)) {
            continue;
        }
        let value = value.trim_matches(['"', '\'']);
        return value.chars().take(160).collect();
    }
    String::new()
}

/// Files under `dir` (at most three levels), absolute, sorted, at most 20.
fn script_files(dir: &Path, depth: u32, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if out.len() >= 20 {
            return;
        }
        if p.is_dir() {
            if depth < 3 {
                script_files(&p, depth + 1, out);
            }
        } else if p.is_file() {
            out.push(p.to_string_lossy().into_owned());
        }
    }
}

/// The skills found under `home` (`.claude`, `.codex` and `.agents` skill folders) and, with a
/// project folder, under its `.claude/skills` and `.agents/skills`. Plugin skills
/// (`plugin:skill`) are not listed: their folders have no fixed place.
pub fn skills_data(home: &Path, project: Option<&Path>) -> Vec<Value> {
    let mut roots: Vec<(&str, &str, PathBuf)> = vec![
        ("claude", "user", home.join(".claude/skills")),
        ("codex", "user", home.join(".codex/skills")),
        ("agents", "user", home.join(".agents/skills")),
    ];
    if let Some(p) = project {
        roots.push(("claude", "project", p.join(".claude/skills")));
        roots.push(("agents", "project", p.join(".agents/skills")));
    }
    let mut out = Vec::new();
    for (harness, scope, root) in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) else {
                continue;
            };
            let name = dir.file_name().unwrap_or_default().to_string_lossy();
            let mut scripts = Vec::new();
            script_files(&dir.join("scripts"), 1, &mut scripts);
            out.push(json!({
                "name": name,
                "harness": harness,
                "scope": scope,
                "path": dir.to_string_lossy(),
                "scripts": scripts,
                "description": skill_description(&text),
            }));
        }
    }
    out
}

/// `sushiai skills [--project DIR] [--json]`: what the owner's machine offers a routine.
fn skills(args: &[String]) -> Result<i32, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    let rows = skills_data(&home, value_of(args, "--project").map(Path::new));
    if flag(args, "--json") {
        print_json(&json!(rows));
    } else {
        for r in &rows {
            println!(
                "{}\t{}\t{}\t{}\tscripts: {}",
                r["name"].as_str().unwrap_or_default(),
                r["harness"].as_str().unwrap_or_default(),
                r["path"].as_str().unwrap_or_default(),
                r["description"].as_str().unwrap_or_default(),
                r["scripts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
    }
    Ok(0)
}

/// The hosts this daemon knows, as `sushiai hosts --json` prints them.
pub fn hosts_data(snapshot: &Value) -> Vec<Value> {
    let this = snapshot["host"].as_str().unwrap_or("local").to_string();
    let mut names = vec![this.clone()];
    for p in snapshot["projects"].as_array().into_iter().flatten() {
        for f in p["folders"].as_array().into_iter().flatten() {
            let h = f["host"].as_str().unwrap_or_default().to_string();
            if !h.is_empty() && !names.contains(&h) {
                names.push(h);
            }
        }
    }
    let harnesses: Vec<&str> = ["claude", "codex"]
        .into_iter()
        .filter(|h| on_path(h))
        .collect();
    names
        .iter()
        .map(|h| {
            let projects: Vec<&str> = snapshot["projects"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| {
                    p["folders"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|f| f["host"] == h.as_str())
                })
                .filter_map(|p| p["name"].as_str())
                .collect();
            // Only this daemon's own machine is inspected; other hosts are known by name.
            let here = *h == this;
            json!({
                "id": h,
                "state": if here { "this host" } else { "known from projects" },
                "harnesses": if here { json!(harnesses) } else { Value::Null },
                "projects": projects,
            })
        })
        .collect()
}

fn hosts(args: &[String]) -> Result<i32, String> {
    let snapshot = call("catalog.get", json!({}))?;
    let rows = hosts_data(&snapshot);
    if flag(args, "--json") {
        print_json(&json!(rows));
    } else {
        for r in &rows {
            println!(
                "{}\t{}\tharnesses: {}\tprojects: {}",
                r["id"].as_str().unwrap_or_default(),
                r["state"].as_str().unwrap_or_default(),
                r["harnesses"]
                    .as_array()
                    .map_or("unknown".to_string(), |a| a
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")),
                r["projects"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
    }
    Ok(0)
}

// ----- routines -----

fn routines(args: &[String]) -> Result<i32, String> {
    match args.first().map(String::as_str) {
        Some("help") => {
            print!("{HELP}");
            Ok(0)
        }
        Some("register") | Some("unregister") => {
            let on = args[0] == "register";
            let home = Home::from_env();
            routines_module::set_enabled(home.dir(), on).map_err(|e| e.to_string())?;
            println!(
                "routines module {} for the next daemon start",
                if on { "enabled" } else { "disabled" }
            );
            Ok(0)
        }
        Some("show") => {
            let id = args.get(1).ok_or("usage: sushiai routines show ID")?;
            print_json(&call("routines.show", json!({"id": id}))?);
            Ok(0)
        }
        None => {
            let list = call("routines.list", json!({}))?;
            if flag(args, "--json") {
                print_json(&list);
                return Ok(0);
            }
            for r in list["routines"].as_array().into_iter().flatten() {
                let routine = &r["routine"];
                println!(
                    "{}\t{}\t{} {}\t{}\tlast: {}",
                    routine["id"].as_str().unwrap_or_default(),
                    routine["name"].as_str().unwrap_or_default(),
                    routine["when"]["cron"].as_str().unwrap_or_default(),
                    routine["when"]["tz"].as_str().unwrap_or_default(),
                    if routine["enabled"] == true {
                        "on"
                    } else {
                        "off"
                    },
                    r["lastRun"]["status"].as_str().unwrap_or("-"),
                );
            }
            Ok(0)
        }
        Some(other) => Err(format!(
            "unknown subcommand {other}; try `sushiai routines help`"
        )),
    }
}

fn routine(args: &[String]) -> Result<i32, String> {
    let rest = &args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("validate") => {
            let dir = rest.first().ok_or("usage: sushiai routine validate DIR")?;
            let report = validate(dir)?;
            print_report(&report);
            Ok(if report["ok"] == true { 0 } else { 1 })
        }
        Some("save") => {
            let dir = rest
                .first()
                .ok_or("usage: sushiai routine save DIR --host H")?;
            let saved = save(dir, value_of(rest, "--host"))?;
            println!(
                "saved {} as a draft (enabled: {}); hash {}",
                saved["id"].as_str().unwrap_or_default(),
                saved["enabled"],
                saved["hash"].as_str().unwrap_or_default()
            );
            if saved["pendingReview"] == true {
                println!(
                    "note: the routine was enabled and its content changed; it is now off \
                     until the owner reviews and enables it again"
                );
            }
            for w in saved["warnings"].as_array().into_iter().flatten() {
                println!(
                    "warning: {}: {}",
                    w["field"].as_str().unwrap_or_default(),
                    w["message"].as_str().unwrap_or_default()
                );
            }
            Ok(0)
        }
        Some("dry-run") => {
            let id = rest.first().ok_or("usage: sushiai routine dry-run ID")?;
            print_json(&run_dry(id, !flag(rest, "--no-wait"))?);
            Ok(0)
        }
        _ => Err("usage: sushiai routine save DIR --host H | validate DIR | dry-run ID".into()),
    }
}

/// Starts a dry run and, when `wait`, polls until its result is there.
pub fn run_dry(id: &str, wait: bool) -> Result<Value, String> {
    run_dry_with(&call, id, wait)
}

/// [`run_dry`] over any daemon transport.
pub fn run_dry_with(
    call: &dyn Fn(&str, Value) -> Result<Value, String>,
    id: &str,
    wait: bool,
) -> Result<Value, String> {
    let started = call("routines.dry_run", json!({"id": id}))?;
    if !wait {
        return Ok(started);
    }
    let n = started["run"].clone();
    let deadline = Instant::now() + Duration::from_secs(45 * 60);
    loop {
        let got = call("routines.result", json!({"id": id, "run": n}))?;
        if got["run"]["status"] != "running" {
            return Ok(got);
        }
        if Instant::now() >= deadline {
            return Err("the dry run did not finish in time".into());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// `routines.save` for a folder, as `sushiai routine save DIR --host H`.
pub fn save(dir: &str, host: Option<&str>) -> Result<Value, String> {
    save_with(&call, dir, host)
}

/// [`save`] over any daemon transport.
pub fn save_with(
    call: &dyn Fn(&str, Value) -> Result<Value, String>,
    dir: &str,
    host: Option<&str>,
) -> Result<Value, String> {
    let mut params = json!({"dir": absolute(dir)});
    if let Some(host) = host {
        params["host"] = json!(host);
    }
    call("routines.save", params)
}

/// `routines.validate` for a folder.
pub fn validate(dir: &str) -> Result<Value, String> {
    validate_with(&call, dir)
}

/// [`validate`] over any daemon transport.
pub fn validate_with(
    call: &dyn Fn(&str, Value) -> Result<Value, String>,
    dir: &str,
) -> Result<Value, String> {
    call("routines.validate", json!({"dir": absolute(dir)}))
}

fn absolute(dir: &str) -> String {
    let p = Path::new(dir);
    if p.is_absolute() {
        dir.to_string()
    } else {
        std::env::current_dir()
            .map(|c| c.join(p).to_string_lossy().into_owned())
            .unwrap_or_else(|_| dir.to_string())
    }
}

fn print_report(report: &Value) {
    for i in report["errors"].as_array().into_iter().flatten() {
        println!(
            "error: {}: {}",
            i["field"].as_str().unwrap_or_default(),
            i["message"].as_str().unwrap_or_default()
        );
    }
    for i in report["warnings"].as_array().into_iter().flatten() {
        println!(
            "warning: {}: {}",
            i["field"].as_str().unwrap_or_default(),
            i["message"].as_str().unwrap_or_default()
        );
    }
    if report["ok"] == true {
        println!("ok; hash {}", report["hash"].as_str().unwrap_or_default());
    }
}

// ----- ask -----

/// Set by the daemon's scheduler in a scheduled session: seconds until the entry's next slot.
const NEXT_SLOT_ENV: &str = "SUSHIAI_SCHEDULE_NEXT_IN_SEC";

/// The default life of an ask in a scheduled run: until the next slot would start, so an
/// unanswered question does not outlive the run that asked it.
fn ttl_from_schedule(value: Option<&str>) -> Option<u64> {
    value?.trim().parse::<u64>().ok().filter(|s| *s > 0)
}

/// What `sushiai ask` was given.
#[derive(Debug, PartialEq)]
struct AskArgs {
    text: String,
    options: Vec<String>,
    ttl: Option<u64>,
    /// The sort of write this asks about; a "yes" is counted per kind.
    kind: String,
}

fn parse_ask(args: &[String]) -> Result<AskArgs, String> {
    let text = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .filter(|a| args.first() == Some(a))
        .ok_or("usage: sushiai ask \"text\" [--option X]... [--ttl SECONDS]")?
        .clone();
    let ttl = match value_of(args, "--ttl") {
        Some(t) => Some(t.parse().map_err(|_| "--ttl is a number of seconds")?),
        None => ttl_from_schedule(std::env::var(NEXT_SLOT_ENV).ok().as_deref()),
    };
    Ok(AskArgs {
        text,
        options: values_of(args, "--option"),
        ttl,
        kind: ask_kind(value_of(args, "--kind")),
    })
}

fn ask(args: &[String]) -> Result<i32, String> {
    let parsed = parse_ask(args)?;
    // A dry run asks nobody.
    if dry_run() {
        println!("{DRY_RUN_ASK}");
        return Ok(0);
    }
    let session = std::env::var("SUSHIAI_SESSION_ID").map_err(|_| "not inside an agent session")?;
    let token =
        std::env::var("SUSHIAI_SESSION_TOKEN").map_err(|_| "not inside an agent session")?;
    let agent = std::env::var("SUSHIAI_AGENT").unwrap_or_else(|_| "claude".into());
    let options: Vec<Value> = parsed
        .options
        .iter()
        .map(|o| json!({"id": o, "label": o}))
        .collect();
    let mut params = json!({
        "session": session, "token": token, "agent": agent,
        "text": parsed.text, "options": options, "kind": parsed.kind,
    });
    if let Some(ttl) = parsed.ttl {
        params["ttlSec"] = json!(ttl);
    }
    // Inside a routine run the file channel is the only one: the run never opens the daemon
    // socket, and the module records the owner's "yes" itself when it forwards the answer.
    // Outside a routine the ask goes over the socket.
    let answer = match ask_channel(std::env::var("SUSHIAI_ROUTINE_DIR").ok().as_deref()) {
        Some(dir) => file_ask(&dir, &params, FILE_POLL)?,
        None => {
            let socket =
                std::env::var("SUSHIAI_SOCKET").map_err(|_| "not inside an agent session")?;
            let stream = UnixStream::connect(&socket)
                .map_err(|e| format!("cannot reach the daemon: {e}"))?;
            hook_ask_on(stream, params)?
        }
    };
    if answer.is_null() {
        println!("expired");
        return Ok(2);
    }
    let allowed = answer["decision"] == "allow";
    let said = answer["choice"]
        .as_str()
        .or_else(|| answer["message"].as_str())
        .unwrap_or(if allowed { "allow" } else { "deny" });
    println!("{said}");
    Ok(if allowed { 0 } else { 1 })
}

/// The routine folder whose file channel an ask uses, or `None` for the daemon socket. Inside
/// a routine run (`SUSHIAI_ROUTINE_DIR` set) it is always the folder, never the socket.
fn ask_channel(routine_dir: Option<&str>) -> Option<PathBuf> {
    routine_dir.filter(|d| !d.is_empty()).map(PathBuf::from)
}

fn dry_run() -> bool {
    std::env::var("SUSHIAI_DRY_RUN").is_ok_and(|v| v == "1")
}

/// `hook.ask` over a hook-role connection; blocks until the owner answers or it expires.
pub fn hook_ask(socket: &Path, params: Value) -> Result<Value, String> {
    let stream =
        UnixStream::connect(socket).map_err(|e| format!("cannot reach the daemon: {e}"))?;
    hook_ask_on(stream, params)
}

/// How often the answer file is looked at.
const FILE_POLL: Duration = Duration::from_millis(250);

static REQ_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The file path of `hook.ask`: writes `<dir>/work/asks/<id>.req`, then waits for
/// `<id>.ans`. Returns what the socket path would: the answer, or `null` when it expired.
fn file_ask(routine_dir: &Path, params: &Value, poll: Duration) -> Result<Value, String> {
    let inbox = ask_inbox(routine_dir);
    let id = format!(
        "{:x}-{:x}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
        std::process::id(),
        REQ_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    std::fs::create_dir_all(&inbox).map_err(|e| format!("cannot reach the daemon: {e}"))?;
    let (req, ans) = (
        inbox.join(format!("{id}.req")),
        inbox.join(format!("{id}.ans")),
    );
    let tmp = inbox.join(format!("{id}.tmp"));
    std::fs::write(&tmp, params.to_string())
        .and_then(|()| std::fs::rename(&tmp, &req))
        .map_err(|e| format!("cannot reach the daemon: {e}"))?;
    let deadline = params["ttlSec"]
        .as_u64()
        .map(|t| Instant::now() + Duration::from_secs(t));
    loop {
        if let Some(text) = std::fs::read_to_string(&ans)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        {
            let _ = std::fs::remove_file(&ans);
            return Ok(if text["expired"] == true {
                Value::Null
            } else {
                text
            });
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            let _ = std::fs::remove_file(&req);
            return Ok(Value::Null);
        }
        std::thread::sleep(poll);
    }
}

fn hook_ask_on(mut stream: UnixStream, params: Value) -> Result<Value, String> {
    let hello = Hello {
        protocol: PROTOCOL_VERSION,
        client: "ask".into(),
        role: Some("hook".into()),
    };
    stream
        .write_all(&encode(&Request::new(1, method::HELLO, hello).frame()))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(&encode(&Request::new(2, method::HOOK_ASK, params).frame()))
        .map_err(|e| e.to_string())?;
    let mut decoder = Decoder::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("the daemon closed the connection".into());
        }
        for frame in decoder.push(&buf[..n]).map_err(|e| e.to_string())? {
            let Frame::Json(text) = frame else { continue };
            if let Ok(Message::Response(r)) = Message::parse(&text) {
                if r.id == 2 {
                    if let Some(e) = r.error {
                        return Err(e.message);
                    }
                    let result = r.result.unwrap_or(Value::Null);
                    return Ok(result.get("answer").cloned().unwrap_or(Value::Null));
                }
            }
        }
    }
}

// ----- propose -----

/// A git write the run asks for. The module applies it after the run, except in a dry run.
fn parse_propose(args: &[String]) -> Result<Value, String> {
    let kind = args.first().map(String::as_str).unwrap_or_default();
    let rest = &args.get(1..).unwrap_or_default();
    let mut p = json!({"kind": kind});
    match kind {
        "pr" => {
            p["title"] = json!(value_of(rest, "--title").ok_or("pr needs --title")?);
            p["paths"] = json!(values_of(rest, "--path"));
            for (flag_name, key) in [
                ("--body", "body"),
                ("--branch", "branch"),
                ("--base", "base"),
            ] {
                if let Some(v) = value_of(rest, flag_name) {
                    p[key] = json!(v);
                }
            }
        }
        "commit" => {
            p["message"] = json!(value_of(rest, "--message").ok_or("commit needs --message")?);
            p["paths"] = json!(values_of(rest, "--path"));
            if let Some(v) = value_of(rest, "--branch") {
                p["branch"] = json!(v);
            }
        }
        "branch" => {
            let name = rest
                .first()
                .filter(|a| !a.starts_with("--"))
                .ok_or("branch needs a NAME")?;
            p["name"] = json!(name);
            if let Some(v) = value_of(rest, "--from") {
                p["from"] = json!(v);
            }
        }
        _ => return Err("usage: sushiai propose pr|commit|branch ...".into()),
    }
    Ok(p)
}

/// Where a session's proposals are queued: under the routine's `work/` folder (the one place
/// a sandboxed Codex run may write), one folder per session so runs never mix.
fn proposals_queue(routine_dir: &Path, session: Option<String>) -> std::path::PathBuf {
    let safe = session
        .filter(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
        .unwrap_or_else(|| "none".to_string());
    routine_dir.join("work").join("proposals").join(safe)
}

fn propose(args: &[String]) -> Result<i32, String> {
    let mut proposal = parse_propose(args)?;
    let dir = std::env::var("SUSHIAI_ROUTINE_DIR")
        .map_err(|_| "not inside a routine run (SUSHIAI_ROUTINE_DIR is not set)")?;
    let projects = std::env::var("SUSHIAI_ROUTINE_PROJECTS")
        .ok()
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0);
    if projects > 1 {
        return Err(sushiai_routines::model::MULTI_PROJECT_PROPOSE.to_string());
    }
    let dry = dry_run();
    proposal["dryRun"] = json!(dry);
    proposal["at"] = json!(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64));
    let queue = proposals_queue(Path::new(&dir), std::env::var("SUSHIAI_SESSION_ID").ok());
    let name = format!(
        "{}-{}.json",
        proposal["at"].as_u64().unwrap_or(0),
        std::process::id()
    );
    let written = std::fs::create_dir_all(&queue)
        .and_then(|()| std::fs::write(queue.join(name), proposal.to_string()));
    match written {
        Ok(()) if dry => println!("recorded, not applied (dry run)"),
        Ok(()) => println!("queued; the daemon applies it after the run"),
        // A read-only dry run cannot record; the answer's actions still say what it would do.
        Err(_) if dry => println!("not applied (dry run); describe it as an action instead"),
        Err(e) => return Err(format!("cannot queue the proposal: {e}")),
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_lists_names_scripts_and_only_the_description_line() {
        let home = tmp("skills-home");
        let skill = home.join(".claude/skills/make-digest");
        std::fs::create_dir_all(skill.join("scripts")).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: make-digest\ndescription: >-\n  Builds a weekly digest.\n  More text.\n---\nSECRET BODY\n",
        )
        .unwrap();
        std::fs::write(skill.join("scripts/run.py"), "print(1)").unwrap();
        let other = home.join(".codex/skills/brand");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("SKILL.md"),
            "---\ndescription: \"Brand rules\"\n---\n",
        )
        .unwrap();
        // A folder without SKILL.md is not a skill.
        std::fs::create_dir_all(home.join(".claude/skills/empty")).unwrap();
        let rows = skills_data(&home, None);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "make-digest");
        assert_eq!(rows[0]["harness"], "claude");
        assert_eq!(rows[0]["description"], "Builds a weekly digest.");
        assert_eq!(
            rows[0]["scripts"][0],
            skill.join("scripts/run.py").to_string_lossy().as_ref()
        );
        assert_eq!(rows[1]["name"], "brand");
        assert_eq!(rows[1]["harness"], "codex");
        assert_eq!(rows[1]["description"], "Brand rules");
        assert!(!json!(rows).to_string().contains("SECRET BODY"));
        // A project folder adds its own skills.
        let project = home.join("my-app");
        std::fs::create_dir_all(project.join(".agents/skills/local")).unwrap();
        std::fs::write(
            project.join(".agents/skills/local/SKILL.md"),
            "no frontmatter",
        )
        .unwrap();
        let rows = skills_data(&home, Some(&project));
        assert_eq!(rows.last().unwrap()["scope"], "project");
        assert_eq!(rows.last().unwrap()["description"], "");
        let _ = std::fs::remove_dir_all(&home);
    }

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ask_takes_text_options_and_ttl() {
        let a = parse_ask(&s(&[
            "Post the digest?",
            "--option",
            "yes",
            "--option",
            "no",
            "--ttl",
            "60",
        ]))
        .unwrap();
        assert_eq!(a.text, "Post the digest?");
        assert_eq!(a.options, ["yes", "no"]);
        assert_eq!(a.ttl, Some(60));
        assert_eq!(a.kind, "other");
        assert!(parse_ask(&s(&["--option", "x"])).is_err());
        let k = parse_ask(&s(&["Open a PR?", "--kind", "PR"])).unwrap();
        assert_eq!(k.kind, "pr");
        // A kind that is not one plain word is "other", never a path.
        let k = parse_ask(&s(&["x", "--kind", "../etc"])).unwrap();
        assert_eq!(k.kind, "other");
    }

    #[test]
    fn an_ask_inside_a_routine_uses_the_file_channel_never_the_socket() {
        assert_eq!(
            ask_channel(Some("/r/job")),
            Some(std::path::PathBuf::from("/r/job"))
        );
        assert_eq!(ask_channel(Some("")), None);
        assert_eq!(ask_channel(None), None);
    }

    #[test]
    fn the_cli_records_no_yes_of_its_own() {
        // The only record of a "yes" is the module's, under state/; a file ask leaves
        // nothing under the routine folder but its request and answer files.
        let dir = tmp("ask-no-yes");
        let daemon = fake_daemon(dir.clone(), json!({"decision": "allow", "choice": "yes"}));
        let params = json!({"session": "s1", "text": "Post?", "kind": "pr", "ttlSec": 30});
        file_ask(&dir, &params, Duration::from_millis(10)).unwrap();
        daemon.join().unwrap();
        assert!(!dir.join("state").exists());
        let inbox: Vec<_> = std::fs::read_dir(ask_inbox(&dir))
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .collect();
        assert!(inbox.is_empty(), "a record folder appeared in the inbox");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ask_expires_at_the_next_slot_unless_told_otherwise() {
        assert_eq!(ttl_from_schedule(Some("3600")), Some(3600));
        assert_eq!(ttl_from_schedule(Some("0")), None);
        assert_eq!(ttl_from_schedule(Some("soon")), None);
        assert_eq!(ttl_from_schedule(None), None);
    }

    #[test]
    fn proposals_are_queued_per_session_under_the_writable_work_folder() {
        let q = proposals_queue(Path::new("/r/job"), Some("sess-1".into()));
        assert_eq!(q, Path::new("/r/job/work/proposals/sess-1"));
        // A session id that is not a plain token cannot escape the queue.
        let q = proposals_queue(Path::new("/r/job"), Some("../../etc".into()));
        assert_eq!(q, Path::new("/r/job/work/proposals/none"));
    }

    #[test]
    fn propose_parses_each_git_write() {
        let p = parse_propose(&s(&["pr", "--title", "Fix", "--base", "main"])).unwrap();
        assert_eq!(p["kind"], "pr");
        assert_eq!(p["title"], "Fix");
        assert_eq!(p["base"], "main");
        let with_paths = parse_propose(&s(&[
            "pr", "--title", "Fix", "--path", "a.txt", "--path", "b/c",
        ]))
        .unwrap();
        assert_eq!(with_paths["paths"], json!(["a.txt", "b/c"]));
        let c = parse_propose(&s(&[
            "commit",
            "--message",
            "m",
            "--path",
            "a",
            "--path",
            "b",
        ]))
        .unwrap();
        assert_eq!(c["paths"], json!(["a", "b"]));
        let b = parse_propose(&s(&["branch", "fix-1", "--from", "main"])).unwrap();
        assert_eq!(b["name"], "fix-1");
        assert!(parse_propose(&s(&["pr"])).is_err());
        assert!(parse_propose(&s(&["merge"])).is_err());
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// A daemon stand-in: answers the first request file it sees.
    fn fake_daemon(dir: std::path::PathBuf, answer: Value) -> std::thread::JoinHandle<Value> {
        std::thread::spawn(move || {
            let inbox = ask_inbox(&dir);
            loop {
                for e in std::fs::read_dir(&inbox).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "req") {
                        let req: Value =
                            serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
                        std::fs::write(p.with_extension("ans"), answer.to_string()).unwrap();
                        std::fs::remove_file(&p).unwrap();
                        return req;
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    }

    #[test]
    fn a_file_ask_round_trips_a_request_and_an_answer() {
        let dir = tmp("ask-file");
        let daemon = fake_daemon(dir.clone(), json!({"decision": "allow", "choice": "yes"}));
        let params = json!({"session": "s1", "text": "Post?", "options": [], "ttlSec": 30});
        let answer = file_ask(&dir, &params, Duration::from_millis(10)).unwrap();
        assert_eq!(answer["decision"], "allow");
        assert_eq!(answer["choice"], "yes");
        let seen = daemon.join().unwrap();
        assert_eq!(seen["text"], "Post?");
        assert_eq!(seen["session"], "s1");
        // The answer file is consumed.
        let left: Vec<_> = std::fs::read_dir(ask_inbox(&dir))
            .unwrap()
            .flatten()
            .collect();
        assert!(left.iter().all(|e| e.path().extension().unwrap() != "ans"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_ask_expires_on_an_expired_answer_and_at_its_ttl() {
        let dir = tmp("ask-expired");
        let daemon = fake_daemon(dir.clone(), json!({"expired": true}));
        let params = json!({"session": "s1", "text": "Post?", "ttlSec": 30});
        assert!(file_ask(&dir, &params, Duration::from_millis(10))
            .unwrap()
            .is_null());
        daemon.join().unwrap();
        // Nobody answers: the ttl ends the wait and withdraws the request.
        let params = json!({"session": "s1", "text": "Again?", "ttlSec": 0});
        assert!(file_ask(&dir, &params, Duration::from_millis(10))
            .unwrap()
            .is_null());
        let reqs = std::fs::read_dir(ask_inbox(&dir))
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().unwrap() == "req")
            .count();
        assert_eq!(reqs, 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
