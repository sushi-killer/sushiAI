//! `sushiai skills [--project DIR] [--json] [--catalog]` and `sushiai skills install|remove
//! NAME`: wires `sushiai_agents::skills` to this host's folders. The tool folders come from
//! `CLAUDE_CONFIG_DIR` and `CODEX_HOME` here, nowhere else. Needs no daemon.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sushiai_agents::skills::{self, Change, Homes, Source};

/// The skill folders of the user home `home`, with the tools' own settings applied.
pub fn homes_in(home: &Path) -> Homes {
    let var = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    Homes::new(home, var("CLAUDE_CONFIG_DIR"), var("CODEX_HOME"))
}

pub fn homes() -> Result<Homes, String> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .ok_or("HOME is not set")?;
    Ok(homes_in(Path::new(&home)))
}

/// What a routine may use: the active folder skills (plugin skills have no fixed place), each
/// with its harness, folder, scripts and the first line of its description.
pub fn list_data(homes: &Homes, project: Option<&Path>) -> Vec<Value> {
    skills::scan(homes, project)
        .into_iter()
        .filter(|s| s.source == Source::Local && s.availability == skills::Availability::Active)
        .map(|s| {
            json!({
                "name": s.name,
                "harness": s.harness.id(),
                "scope": s.scope,
                "path": s.dir,
                "scripts": s.scripts,
                "description": s.summary(),
            })
        })
        .collect()
}

pub fn run(args: impl Iterator<Item = String>) -> i32 {
    let args: Vec<String> = args.collect();
    let result = match args.first().map(String::as_str) {
        Some(verb @ ("install" | "remove")) => change(verb, &args[1..]),
        _ => list(&args),
    };
    result.unwrap_or_else(|message| {
        eprintln!("sushiai skills: {message}");
        1
    })
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// `skills install NAME` (the SKILL.md text on stdin) or `skills remove NAME`.
fn change(verb: &str, args: &[String]) -> Result<i32, String> {
    let name = args
        .first()
        .ok_or_else(|| format!("usage: sushiai skills {verb} NAME"))?;
    let homes = homes()?;
    let changes = if verb == "install" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| format!("cannot read the skill text from stdin: {e}"))?;
        skills::install(&homes, name, &text)
    } else {
        skills::remove(&homes, name)
    }
    .map_err(|e| e.to_string())?;
    let mut failed = false;
    for c in &changes {
        if matches!(c, Change::Failed(..)) {
            failed = true;
            eprintln!("sushiai skills: {c}");
        } else {
            println!("{c}");
        }
    }
    Ok(i32::from(failed))
}

fn list(args: &[String]) -> Result<i32, String> {
    let homes = homes()?;
    let project = args
        .windows(2)
        .find(|w| w[0] == "--project")
        .map(|w| Path::new(&w[1]));
    if flag(args, "--catalog") {
        let rows = skills::scan(&homes, project);
        if flag(args, "--json") {
            println!(
                "{}",
                serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?
            );
        } else {
            for s in &rows {
                println!(
                    "{}\t{}\t{:?}\t{:?}\t{}{}",
                    s.name,
                    s.harness.id(),
                    s.source,
                    s.availability,
                    s.file,
                    if s.is_duplicate { "\tduplicate" } else { "" }
                );
            }
        }
        return Ok(0);
    }
    let rows = list_data(&homes, project);
    if flag(args, "--json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?
        );
        return Ok(0);
    }
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
    Ok(0)
}
