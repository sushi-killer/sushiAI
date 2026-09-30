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
//! orchd eval harvest --data <dir> --repo <path> --out <file>
//! ```
//!
//! `harvest` writes candidate set entries from the repo's own finished tasks
//! that are not eval tasks. It never edits a set: a person (or the lead)
//! promotes a candidate into one, adding a `check` if it has a way to grade.

use crate::model::{Task, TaskStatus};
use crate::store::Store;
use crate::{git, mcp};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;

/// (set, name, base, request, check)
type Job = (String, String, String, String, Option<String>);

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
    // (set, name, base, request, check)
    let mut jobs: Vec<Job> = vec![];
    if let Some(request) = &o.request {
        let base = o.base.clone().unwrap_or_else(|| "HEAD".into());
        jobs.push((
            "adhoc".into(),
            adhoc_name.into(),
            base,
            request.clone(),
            None,
        ));
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
                t.get("check")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(String::from),
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
    for (_, name, base, _, _) in &jobs {
        let sha = git::resolve_commit(&repo, base)
            .map_err(|e| format!("task \"{name}\": base \"{base}\" does not resolve: {e}"))?;
        resolved.push(sha);
    }
    for arm in &arms {
        for ((set, name, _, request, check), sha) in jobs.iter().zip(&resolved) {
            for _ in 0..o.repeat {
                let mut p = json!({
                    "repo": repo_str,
                    "base": sha,
                    "request": request,
                    "evalSet": set,
                    "evalName": name,
                });
                if let Some(c) = check {
                    p["evalCheck"] = json!(c);
                }
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

/// Candidate entries for the finished, non-eval tasks of `repo`, skipping any
/// task without a base sha and any whose id is in `seen` (or repeats). The
/// reference is what `reference_of` finds for a done task.
fn harvest_candidates(
    tasks: &[Task],
    repo: &str,
    seen: &HashSet<String>,
    reference_of: &dyn Fn(&Task) -> Option<String>,
) -> Vec<Value> {
    let mut sources = seen.clone();
    let mut names: HashSet<String> = HashSet::new();
    let mut out = vec![];
    let mut ordered: Vec<&Task> = tasks.iter().collect();
    ordered.sort_by_key(|t| (t.created_at, t.id.clone()));
    for t in ordered {
        let outcome = match t.status {
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
            TaskStatus::Stopped => "stopped",
            _ => continue,
        };
        if t.repo != repo
            || t.eval_set.is_some()
            || t.base_sha.trim().is_empty()
            || !sources.insert(t.id.clone())
        {
            continue;
        }
        let request = t
            .request
            .clone()
            .filter(|r| !r.trim().is_empty())
            .unwrap_or_else(|| t.goal.clone());
        let mut name = git::slugify(&t.title, 40);
        if !names.insert(name.clone()) {
            name = format!("{name}-{}", t.id.chars().take(8).collect::<String>());
            names.insert(name.clone());
        }
        out.push(json!({
            "name": name,
            "base": t.base_sha,
            "request": request,
            "reference": if t.status == TaskStatus::Done { reference_of(t) } else { None },
            "source": t.id,
            "outcome": outcome,
        }));
    }
    out
}

fn harvest(args: &[String]) -> Result<usize, String> {
    let need = |name: &str| flag(args, name).ok_or_else(|| format!("{name} is required"));
    let data = PathBuf::from(need("--data")?);
    let repo = PathBuf::from(need("--repo")?);
    let out = PathBuf::from(need("--out")?);
    let root = git::repo_toplevel(&repo).map_err(|e| e.to_string())?;
    if !data.join("tasks").is_dir() {
        return Err(format!("no task store at {}", data.display()));
    }
    let store = Store::new(&data).map_err(|e| e.to_string())?;
    let tasks = store.list_tasks().map_err(|e| e.to_string())?;
    // Candidates already in the file stay, and their sources are not added again.
    let mut entries: Vec<Value> = match std::fs::read_to_string(&out) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|e| format!("{} is not valid JSON: {e}", out.display()))?
            .get("tasks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        Err(_) => vec![],
    };
    let seen: HashSet<String> = entries
        .iter()
        .filter_map(|e| e.get("source").and_then(Value::as_str).map(String::from))
        .collect();
    let repo_str = root.to_string_lossy().to_string();
    let landed = |t: &Task| git::resolve_commit(&root, &t.branch).ok();
    let fresh = harvest_candidates(&tasks, &repo_str, &seen, &landed);
    let added = fresh.len();
    entries.extend(fresh);
    let doc = json!({
        "about": "Candidates harvested from finished tasks; promote by hand into a set (add a `check` where a command can grade the result).",
        "tasks": entries,
    });
    let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&out, text + "\n")
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(added)
}

pub fn run(args: &[String]) -> i32 {
    if args.first().map(String::as_str) == Some("harvest") {
        return match harvest(&args[1..]) {
            Ok(n) => {
                println!("{{\"added\":{n}}}");
                0
            }
            Err(e) => {
                eprintln!("orchd eval harvest: {e}");
                1
            }
        };
    }
    if args.first().map(String::as_str) != Some("run") {
        eprintln!("orchd eval: usage: orchd eval run --data <dir> --socket <sock> (--set <file> | --request <text> --arms <json>) ... | orchd eval harvest --data <dir> --repo <path> --out <file>");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, title: &str, status: TaskStatus, created: i64) -> Task {
        let mut t: Task = serde_json::from_value(json!({
            "id": id, "title": title, "goal": "the goal", "criteria": [], "verify": [],
            "repo": "/r", "worktree": "/w", "branch": format!("task/{id}"), "baseSha": "b1",
            "status": "queued", "tier": "standard", "createdAt": created, "updatedAt": created
        }))
        .unwrap();
        t.status = status;
        t
    }

    #[test]
    fn harvest_keeps_finished_non_eval_tasks_of_the_repo_once() {
        let mut with_request = task("a1", "Add CSV export!", TaskStatus::Done, 1);
        with_request.request = Some("export csv please".into());
        let failed = task("a2", "Fix login", TaskStatus::Failed, 2);
        let stopped = task("a3", "Try a thing", TaskStatus::Stopped, 3);
        let running = task("a4", "Still going", TaskStatus::Running, 4);
        let mut eval = task("a5", "An eval run", TaskStatus::Done, 5);
        eval.eval_set = Some("set-1".into());
        let mut no_base = task("a6", "No base", TaskStatus::Done, 6);
        no_base.base_sha = String::new();
        let mut other_repo = task("a7", "Elsewhere", TaskStatus::Done, 7);
        other_repo.repo = "/other".into();
        let already = task("a8", "Already there", TaskStatus::Done, 8);
        let twin = task("a9", "Add CSV export", TaskStatus::Done, 9);
        let tasks = vec![
            twin,
            already,
            other_repo,
            no_base,
            eval,
            running,
            stopped,
            failed,
            with_request,
        ];
        let seen: HashSet<String> = ["a8".to_string()].into();
        let landed = |t: &Task| Some(format!("sha-of-{}", t.id));
        let got = harvest_candidates(&tasks, "/r", &seen, &landed);
        let sources: Vec<&str> = got.iter().map(|c| c["source"].as_str().unwrap()).collect();
        assert_eq!(sources, ["a1", "a2", "a3", "a9"], "{got:?}");
        assert_eq!(
            got[0],
            json!({
                "name": "add-csv-export", "base": "b1", "request": "export csv please",
                "reference": "sha-of-a1", "source": "a1", "outcome": "done",
            })
        );
        // No request: the goal. Only a done task has a reference.
        assert_eq!(got[1]["request"], "the goal");
        assert_eq!(got[1]["reference"], Value::Null);
        assert_eq!(got[1]["outcome"], "failed");
        assert_eq!(got[2]["outcome"], "stopped");
        // A second task with the same title gets a distinct name.
        assert_eq!(got[3]["name"], "add-csv-export-a9");
        // Running it again over the same sources adds nothing.
        let mut again = seen.clone();
        again.extend(sources.iter().map(|s| s.to_string()));
        assert!(harvest_candidates(&tasks, "/r", &again, &landed).is_empty());
    }
}
