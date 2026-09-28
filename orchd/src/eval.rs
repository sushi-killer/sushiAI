//! `orchd eval run`: creates the tasks of an eval set (or one ad-hoc A/B
//! pair) through a running daemon's `task.create`, one per task and arm.
//! It only creates tasks; the daemon runs them as usual and `orchd ab
//! --eval <set>` reports them.
//!
//! ```text
//! orchd eval run --data <dir> --socket <sock> --set <file> \
//!     [--variant '<json>' | --arms '[<json>, ...]'] [--only a,b] [--repeat N] [--repo <path>]
//! orchd eval run --data <dir> --socket <sock> --request '<text>' \
//!     --arms '[<json>, <json>]' [--base <ref>] [--repo <path>]
//! ```

use crate::{git, mcp};
use serde_json::{json, Value};
use std::path::PathBuf;

struct Options {
    data: PathBuf,
    socket: PathBuf,
    repo: PathBuf,
    set: Option<PathBuf>,
    request: Option<String>,
    base: Option<String>,
    variant: Option<String>,
    arms: Option<String>,
    only: Vec<String>,
    repeat: u32,
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn parse(args: &[String]) -> Result<Options, String> {
    let need = |name: &str| flag(args, name).ok_or_else(|| format!("{name} is required"));
    let repeat = match flag(args, "--repeat") {
        None => 1,
        Some(n) => n
            .parse::<u32>()
            .ok()
            .filter(|n| *n >= 1)
            .ok_or_else(|| format!("--repeat must be a positive integer, got \"{n}\""))?,
    };
    let repo = match flag(args, "--repo") {
        Some(r) => PathBuf::from(r),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let o = Options {
        data: PathBuf::from(need("--data")?),
        socket: PathBuf::from(need("--socket")?),
        repo,
        set: flag(args, "--set").map(PathBuf::from),
        request: flag(args, "--request"),
        base: flag(args, "--base"),
        variant: flag(args, "--variant"),
        arms: flag(args, "--arms"),
        only: flag(args, "--only")
            .map(|s| {
                s.split(',')
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        repeat,
    };
    if o.set.is_some() == o.request.is_some() {
        return Err("give exactly one of --set <file> or --request <text>".into());
    }
    if o.request.is_some() && o.arms.is_none() && o.variant.is_none() {
        return Err("--request needs --arms '[<variant json>, ...]'".into());
    }
    if o.variant.is_some() && o.arms.is_some() {
        return Err("give --variant or --arms, not both".into());
    }
    Ok(o)
}

/// The variants to run: `--arms` (a JSON array), `--variant` (one), or none
/// (a single arm with the daemon's default flags).
fn arms(o: &Options) -> Result<Vec<Option<Value>>, String> {
    let object = |v: Value, what: &str| {
        if v.is_object() {
            Ok(v)
        } else {
            Err(format!("{what} must be a JSON object"))
        }
    };
    if let Some(text) = &o.arms {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("--arms: {e}"))?;
        let list = v
            .as_array()
            .filter(|l| !l.is_empty())
            .ok_or("--arms must be a non-empty JSON array")?;
        return list
            .iter()
            .map(|a| object(a.clone(), "each --arms entry").map(Some))
            .collect();
    }
    if let Some(text) = &o.variant {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("--variant: {e}"))?;
        return Ok(vec![Some(object(v, "--variant")?)]);
    }
    Ok(vec![None])
}

/// Every `task.create` param object to send, fully validated (set file
/// read, names known, bases resolved) before any of them is sent.
fn plan(o: &Options, adhoc_name: &str) -> Result<Vec<Value>, String> {
    let repo = git::repo_toplevel(&o.repo).map_err(|e| e.to_string())?;
    let repo_str = repo.to_string_lossy().to_string();
    let arms = arms(o)?;
    // (set, name, base, request)
    let mut jobs: Vec<(String, String, String, String)> = vec![];
    if let Some(request) = &o.request {
        let base = o.base.clone().unwrap_or_else(|| "HEAD".into());
        jobs.push(("adhoc".into(), adhoc_name.into(), base, request.clone()));
    } else if let Some(set) = &o.set {
        let stem = set
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("--set has no file name")?
            .to_string();
        let text = std::fs::read_to_string(set)
            .map_err(|e| format!("cannot read {}: {e}", set.display()))?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not valid JSON: {e}", set.display()))?;
        let tasks = v
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{} has no \"tasks\" array", set.display()))?;
        let field = |t: &Value, k: &str| -> Result<String, String> {
            t.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(String::from)
                .ok_or_else(|| format!("a task in {} has no \"{k}\"", set.display()))
        };
        let mut all = vec![];
        for t in tasks {
            all.push((
                stem.clone(),
                field(t, "name")?,
                field(t, "base")?,
                field(t, "request")?,
            ));
        }
        let unknown: Vec<&String> = o
            .only
            .iter()
            .filter(|n| !all.iter().any(|j| &j.1 == *n))
            .collect();
        if !unknown.is_empty() {
            let known: Vec<&str> = all.iter().map(|j| j.1.as_str()).collect();
            return Err(format!(
                "unknown task name in --only: {} (the set has: {})",
                unknown
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                known.join(", ")
            ));
        }
        jobs = all
            .into_iter()
            .filter(|j| o.only.is_empty() || o.only.contains(&j.1))
            .collect();
    }
    let mut out = vec![];
    // Resolve each base once so every arm and repeat starts from one commit.
    let mut resolved: Vec<String> = vec![];
    for (_, name, base, _) in &jobs {
        let sha = git::resolve_commit(&repo, base)
            .map_err(|e| format!("task \"{name}\": base \"{base}\" does not resolve: {e}"))?;
        resolved.push(sha);
    }
    for arm in &arms {
        for ((set, name, _, request), sha) in jobs.iter().zip(&resolved) {
            for _ in 0..o.repeat {
                let mut p = json!({
                    "repo": repo_str,
                    "base": sha,
                    "request": request,
                    "evalSet": set,
                    "evalName": name,
                });
                if let Some(v) = arm {
                    p["variant"] = v.clone();
                }
                out.push(p);
            }
        }
    }
    Ok(out)
}

fn create_all(o: &Options, adhoc_name: &str) -> Result<Value, String> {
    let params = plan(o, adhoc_name)?;
    let token = mcp::read_control_token(&o.data)?;
    let mut created = vec![];
    for p in params {
        let task = mcp::call_orchd(&o.socket, &token, "task.create", p.clone())?;
        created.push(json!({
            "id": task["id"],
            "evalSet": p["evalSet"],
            "evalName": p["evalName"],
            "variant": task["variant"],
        }));
    }
    Ok(json!({ "tasks": created }))
}

pub fn run(args: &[String]) -> i32 {
    if args.first().map(String::as_str) != Some("run") {
        eprintln!("orchd eval: usage: orchd eval run --data <dir> --socket <sock> (--set <file> | --request <text> --arms <json>) ...");
        return 2;
    }
    let o = match parse(&args[1..]) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("orchd eval run: {e}");
            return 2;
        }
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match create_all(&o, &format!("adhoc-{now}")) {
        Ok(v) => {
            println!("{v}");
            0
        }
        Err(e) => {
            eprintln!("orchd eval run: {e}");
            1
        }
    }
}
