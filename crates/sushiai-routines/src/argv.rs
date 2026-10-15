//! Maps a routine's agent to the headless command line the daemon passes to the harness
//! (`SessionCreate.extraArgs`). Pure: no IO.
//!
//! Both harnesses run non-interactively and force the final answer into the result schema.
//! Flags verified with `claude --help` (2.1.300) and `codex exec --help` (0.162.1).

use std::path::Path;

use crate::model::Agent;

/// What a Claude dry run may use: read tools, `git status`, and the two commands that talk
/// to the module (`ask` prints "skipped (dry run)" and asks nobody, `propose` only records).
/// Everything else, including every MCP tool, is not on the list and so is refused: a dry
/// run is fail-closed, not a list of known writes.
pub const DRY_RUN_ALLOW: &[&str] = &[
    "Read",
    "Grep",
    "Glob",
    // No `git log`, `diff` or `show`: they take `--output=<file>` and so can write. A dry run
    // reads history through the worktree's files.
    "Bash(git status:*)",
    "Bash(sushiai ask:*)",
    "Bash(sushiai propose:*)",
];

/// What every Claude run may use besides the owner's grants: read tools and the two commands
/// that talk to the module. Nothing else is on the list, so nothing else is allowed.
pub const RUN_ALLOW: &[&str] = &[
    "Read",
    "Grep",
    "Glob",
    "Bash(sushiai ask:*)",
    "Bash(sushiai propose:*)",
];

/// Denied on top of the allowlist, so a settings file that allows them cannot win.
pub const DRY_RUN_DENY: &[&str] = &["Edit", "Write", "NotebookEdit"];

/// The built-in tools a Claude dry run has at all (`--tools`).
const DRY_RUN_TOOLS: &str = "Read,Grep,Glob,Bash";

/// What the mapping needs besides the agent.
pub struct RunSpec<'a> {
    /// Compact JSON of the result schema (Claude takes it inline).
    pub schema_json: &'a str,
    /// The schema as a file (Codex reads it from disk).
    pub schema_file: &'a Path,
    /// Where Codex writes its last message.
    pub last_message: &'a Path,
    pub dry_run: bool,
    /// The tools the owner granted. A real Claude run is allowed [`RUN_ALLOW`] plus these and
    /// nothing else; a dry run never uses them.
    pub granted: &'a [String],
}

/// The argument list placed after the harness name, before the prompt.
pub fn harness_argv(agent: &Agent, spec: &RunSpec) -> Result<Vec<String>, String> {
    let effort = agent.effort.as_deref();
    let mut argv: Vec<String> = Vec::new();
    match agent.harness.as_str() {
        "claude" => {
            if let Some(e) = effort {
                if !["low", "medium", "high", "xhigh", "max"].contains(&e) {
                    return Err(format!("effort {e} is not a Claude level"));
                }
            }
            argv.extend(
                [
                    "-p",
                    "--output-format",
                    "json",
                    "--json-schema",
                    spec.schema_json,
                ]
                .map(String::from),
            );
            if let Some(m) = &agent.model {
                argv.extend(["--model".into(), m.clone()]);
            }
            if let Some(e) = effort {
                argv.extend(["--effort".into(), e.into()]);
            }
            // Always explicit: the owner's `defaultMode` must not widen an unattended run.
            // Under `dontAsk` a tool that is not allowed is refused, never asked about.
            argv.extend(["--permission-mode".into(), "dontAsk".into()]);
            if !spec.dry_run {
                let mut allow: Vec<String> = RUN_ALLOW.iter().map(|t| t.to_string()).collect();
                for tool in spec.granted {
                    if !allow.contains(tool) {
                        allow.push(tool.clone());
                    }
                }
                argv.extend(["--allowedTools".into(), allow.join(",")]);
            }
            if spec.dry_run {
                argv.extend([
                    "--tools".into(),
                    DRY_RUN_TOOLS.into(),
                    "--allowedTools".into(),
                    DRY_RUN_ALLOW.join(","),
                    "--disallowedTools".into(),
                    DRY_RUN_DENY.join(","),
                ]);
            }
        }
        "codex" => {
            if let Some(e) = effort {
                if !["low", "medium", "high"].contains(&e) {
                    return Err(format!("effort {e} is not offered for Codex"));
                }
            }
            argv.extend([
                "exec".into(),
                "--output-schema".into(),
                spec.schema_file.to_string_lossy().into_owned(),
                "-o".into(),
                spec.last_message.to_string_lossy().into_owned(),
            ]);
            if let Some(m) = &agent.model {
                argv.extend(["-m".into(), m.clone()]);
            }
            if let Some(e) = effort {
                argv.extend(["-c".into(), format!("model_reasoning_effort={e}")]);
            }
            // The owner's MCP servers (config.toml) are never started: grants cannot name an
            // MCP tool for Codex, and a dry run writes nothing.
            argv.extend(["-c".into(), "mcp_servers={}".into()]);
            if spec.dry_run {
                // No network and no writes: `sushiai ask` prints "skipped (dry run)" without
                // opening anything, so the read-only sandbox is enough.
                argv.extend([
                    "-s".into(),
                    "read-only".into(),
                    "--skip-git-repo-check".into(),
                ]);
            } else {
                // Network stays off, so the run cannot reach the daemon socket. `sushiai ask`
                // inside a routine always uses the file channel under `work/asks/`; the
                // working root is the routine's `work/`, the only place a run may write.
                argv.extend([
                    "-s".into(),
                    "workspace-write".into(),
                    "--skip-git-repo-check".into(),
                ]);
            }
        }
        other => return Err(format!("unknown harness {other}")),
    }
    if spec.dry_run {
        argv.extend(dry_run_extra_args(&agent.extra_args));
    } else {
        // The owner's grants are the allowlist: a routine's own `--allowedTools` is dropped.
        argv.extend(without_allowed_tools(&agent.extra_args));
    }
    Ok(argv)
}

/// What a dry run keeps of a routine's `extraArgs`: only flags that cannot grant anything
/// (`--allowedTools`, `--add-dir` and the rest are dropped, with their values).
fn dry_run_extra_args(args: &[String]) -> Vec<String> {
    const KEEP: &[&str] = &["--fallback-model", "--max-turns", "--disallowedTools"];
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let name = a.split('=').next().unwrap_or(a);
        let keep = KEEP.contains(&name);
        if keep {
            out.push(a.clone());
        }
        i += 1;
        // A flag's values are the arguments up to the next flag (`--x=v` carries its own).
        if !a.contains('=') {
            while i < args.len() && !args[i].starts_with('-') {
                if keep {
                    out.push(args[i].clone());
                }
                i += 1;
            }
        }
    }
    out
}

/// `args` without `--allowedTools` and its values.
fn without_allowed_tools(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let name = a.split('=').next().unwrap_or(a);
        i += 1;
        if name == "--allowedTools" || name == "--allowed-tools" {
            if !a.contains('=') {
                while i < args.len() && !args[i].starts_with('-') {
                    i += 1;
                }
            }
        } else {
            out.push(a.clone());
        }
    }
    out
}

/// Interpreters and launchers a `Bash(<cmd>:*)` allowance may not name: they run anything.
/// Compared by the basename of the command word, so `/bin/sh` is `sh`.
const OPEN_COMMANDS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "fish",
    "ksh",
    "env",
    "sudo",
    "doas",
    "su",
    "xargs",
    "timeout",
    "nice",
    "nohup",
    "time",
    "stdbuf",
    "exec",
    "eval",
    "command",
    "builtin",
    "npx",
    "npm",
    "pnpm",
    "yarn",
    "bunx",
    "node",
    "deno",
    "bun",
    "python",
    "python3",
    "ruby",
    "perl",
    "php",
    "osascript",
    "open",
    "curl",
    "wget",
    "ssh",
    "scp",
    "nc",
    "ncat",
    "find",
    "awk",
    "sed",
    "tee",
    "dd",
];

/// File tools that need a path scope, never the bare name. Claude matches file rules only as
/// `Edit(<path>)`, which covers every file-editing tool, so the others are refused.
const SCOPED_TOOLS: &[&str] = &["Edit"];
const FILE_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// `git` and `gh` subcommands that change config, credentials or define new commands.
const GIT_REFUSED: &[&str] = &["config", "-c", "alias", "daemon", "credential"];
const GH_REFUSED: &[&str] = &["api", "auth", "alias", "extension", "config", "secret"];

/// One tool pattern of `--allowedTools`: a plain tool name, an MCP tool of a named server, or
/// a `Bash(<command prefix>:*)` allowance for a command that is not an interpreter. Bare
/// `Bash`, `Bash(*)` and wildcard MCP patterns are unbounded and refused.
fn check_allowed_tool(tool: &str) -> Result<(), String> {
    let plain = |t: &str| {
        !t.is_empty()
            && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && t.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
    };
    if let Some(rest) = tool.strip_prefix("Bash(") {
        let inner = rest
            .strip_suffix(')')
            .ok_or_else(|| format!("{tool} is not a tool pattern"))?;
        let prefix = inner
            .strip_suffix(":*")
            .ok_or_else(|| format!("{tool}: use Bash(<command prefix>:*)"))?;
        let word = prefix.split_whitespace().next().unwrap_or_default();
        let first = word.rsplit('/').next().unwrap_or_default();
        let safe = !word.is_empty()
            && !first.is_empty()
            && prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || " ._-/".contains(c));
        if !safe {
            return Err(format!("{tool} is an unbounded Bash allowance"));
        }
        if OPEN_COMMANDS.contains(&first) {
            return Err(format!("{tool} allows a command that runs anything"));
        }
        // `git log:*` is fine; `git:*` or `gh:*` is every git/gh command.
        let mut words = prefix.split_whitespace();
        words.next();
        let sub = words.next().unwrap_or_default();
        if first == "git" || first == "gh" {
            let named_subcommand = sub.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
            let refused = if first == "git" {
                GIT_REFUSED
            } else {
                GH_REFUSED
            };
            if !named_subcommand || refused.contains(&sub) {
                return Err(format!(
                    "{tool}: `{first} {sub}` can change settings or run anything"
                ));
            }
        }
        return Ok(());
    }
    if let Some(rest) = tool.strip_prefix("mcp__") {
        let ok = !rest.is_empty()
            && rest.split("__").all(|p| {
                !p.is_empty()
                    && p.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
            && rest.split("__").count() <= 2;
        return if ok {
            Ok(())
        } else {
            Err(format!("{tool} is not an exact MCP tool or server name"))
        };
    }
    if tool == "Bash" {
        return Err("a bare Bash allowance is unbounded; use Bash(<command prefix>:*)".into());
    }
    if FILE_TOOLS.contains(&tool) {
        return Err(format!("{tool} needs a path scope, as Edit(<path>)"));
    }
    if let Some((name, rest)) = tool.split_once('(') {
        if FILE_TOOLS.contains(&name) && !SCOPED_TOOLS.contains(&name) {
            return Err(format!("{tool}: file rules are written Edit(<path>)"));
        }
        let scope = rest.strip_suffix(')').unwrap_or_default();
        let ok = SCOPED_TOOLS.contains(&name)
            && scope.len() <= 200
            && scope.chars().any(|c| c.is_ascii_alphanumeric())
            && !scope.split('/').any(|c| c == "..")
            && scope
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || " ._-/*".contains(c));
        return if ok {
            Ok(())
        } else {
            Err(format!("{tool} is not a scoped file tool"))
        };
    }
    if plain(tool) {
        Ok(())
    } else {
        Err(format!("{tool} is not a tool name"))
    }
}

/// Why `tool` may not be an access entry of a routine: an exact tool name, a
/// `Bash(<command prefix>:*)` allowance or one exact `mcp__<server>__<tool>`. Every other
/// wildcard is refused, and so is a whole MCP server.
pub fn check_access_tool(tool: &str) -> Result<(), String> {
    if let Some(rest) = tool.strip_prefix("mcp__") {
        if rest.split("__").count() != 2 {
            return Err(format!("{tool}: name one tool as mcp__<server>__<tool>"));
        }
    }
    check_allowed_tool(tool)
}

/// Splits one `--allowedTools` argument on commas and spaces outside parentheses.
fn split_tools(arg: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in arg.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' | ' ' | '\t' if depth <= 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_model_name(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 100
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:-/".contains(c))
}

/// Why `args` may not be passed to a harness, if so. This is an allowlist: a flag that is not
/// named here is refused, which also covers attached short forms (`-cNAME=v`, `-sX`),
/// `--mcp-config` and every config or sandbox override. Allowed: `--fallback-model`,
/// `--max-turns` (Claude), `--allowedTools` (bounded tool patterns only),
/// `--disallowedTools`, `--ephemeral` (Codex) and `--add-dir` inside `routine_dir/work`.
pub fn check_extra_args(args: &[String], routine_dir: &Path) -> Result<(), String> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let (name, attached) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (a.as_str(), None),
        };
        // The value: attached, or the next argument.
        let mut take_value = |what: &str| -> Result<String, String> {
            if let Some(v) = &attached {
                return Ok(v.clone());
            }
            i += 1;
            args.get(i)
                .filter(|v| !v.starts_with('-'))
                .cloned()
                .ok_or_else(|| format!("{name} needs a {what}"))
        };
        match name {
            "--fallback-model" => {
                let v = take_value("model name")?;
                if !is_model_name(&v) {
                    return Err(format!("{v} is not a model name"));
                }
            }
            "--max-turns" => {
                let v = take_value("number")?;
                if !v.parse::<u32>().is_ok_and(|n| (1..=500).contains(&n)) {
                    return Err("--max-turns is a number from 1 to 500".into());
                }
            }
            "--add-dir" => {
                let v = take_value("folder")?;
                let p = Path::new(&v);
                let inside = p.is_absolute()
                    && p.starts_with(routine_dir.join("work"))
                    && !p.components().any(|c| c.as_os_str() == "..");
                if !inside {
                    return Err(
                        "--add-dir may name only a folder inside the routine's work/ folder".into(),
                    );
                }
            }
            "--ephemeral" => {
                if attached.is_some() {
                    return Err("--ephemeral takes no value".into());
                }
            }
            "--allowedTools" | "--allowed-tools" | "--disallowedTools" | "--disallowed-tools" => {
                let allow = name.starts_with("--allowed");
                // Variadic: the values run up to the next flag.
                let mut values: Vec<String> = Vec::new();
                if let Some(v) = &attached {
                    values.push(v.clone());
                } else {
                    while args.get(i + 1).is_some_and(|v| !v.starts_with('-')) {
                        i += 1;
                        values.push(args[i].clone());
                    }
                }
                if values.is_empty() {
                    return Err(format!("{name} needs at least one tool"));
                }
                for tool in values.iter().flat_map(|v| split_tools(v)) {
                    if allow {
                        check_allowed_tool(&tool)?;
                    } else if tool.is_empty() || tool.len() > 200 || tool.contains(['\n', '\0']) {
                        return Err(format!("{tool} is not a tool pattern"));
                    }
                }
            }
            "-c" | "--config" => {
                return Err("a config override (-c/--config) is not allowed in extraArgs".into())
            }
            "--mcp-config" | "--strict-mcp-config" => {
                return Err("MCP configuration is not allowed in extraArgs".into())
            }
            other if other.starts_with("-c") || other.starts_with("-s") => {
                return Err(format!(
                    "{other}: attached short options are not allowed in extraArgs"
                ))
            }
            other => {
                return Err(format!(
                    "{other} is not allowed in a routine's extraArgs; allowed: \
                     --fallback-model, --max-turns, --allowedTools, --disallowedTools, \
                     --add-dir (inside the routine's work/ folder), --ephemeral"
                ))
            }
        }
        i += 1;
    }
    Ok(())
}
