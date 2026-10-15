//! `sushiai mcp --routines`: the routine builder's door. It serves the read and build tools
//! of `sushiai routines help` over MCP, with the CLI's own validation and host rules. The
//! in-run commands (`ask`, `propose`) are not tools: only a run has them.
//!
//! `sushiai mcp [--task <id> | --read-only]`: the stdio MCP server an agent attaches to. The
//! tool surface and JSON-RPC handling live in `sushiai-orch`; this only supplies the
//! transport: each tool call becomes an `orch.*` request to this home's daemon, started on
//! first use like `sushiai proxy` does.

use std::io::{BufRead, Write};

use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2024-11-05";

/// What a tool needs from the daemon: one request, one answer.
type Call<'a> = &'a dyn Fn(&str, Value) -> Result<Value, String>;

/// The tools of the routine builder: name, description, input properties, required.
fn tools() -> Value {
    let string = |d: &str| json!({"type": "string", "description": d});
    let tool = |name: &str, description: &str, props: Value, required: Value| {
        json!({"name": name, "description": description,
               "inputSchema": {"type": "object", "properties": props, "required": required}})
    };
    json!([
        tool(
            "routines_projects",
            "Projects: id, name, purpose and folders per host. With `id`, one project with an excerpt of its README.",
            json!({"id": string("a project id or name")}),
            json!([]),
        ),
        tool(
            "routines_hosts",
            "Hosts this daemon knows, with the harnesses on this one and their projects.",
            json!({}),
            json!([]),
        ),
        tool(
            "routines_list",
            "Existing routines: routine.json, access, needsAccess, last run.",
            json!({}),
            json!([]),
        ),
        tool(
            "routines_show",
            "One routine: routine.json, brief.md, access, last runs and result.",
            json!({"id": string("the routine id")}),
            json!(["id"]),
        ),
        tool(
            "routines_validate",
            "Check a routine folder (routine.json + brief.md); returns every error and warning.",
            json!({"dir": string("absolute path of the routine folder")}),
            json!(["dir"]),
        ),
        tool(
            "routines_save",
            "Store the routine folder on host `host` as a draft. This daemon must be that host.",
            json!({"dir": string("absolute path of the routine folder"),
                   "host": string("the host the routine runs on")}),
            json!(["dir", "host"]),
        ),
        tool(
            "routines_dry_run",
            "One run of a saved routine that writes nothing; returns its result. It can take minutes.",
            json!({"id": string("the routine id"),
                   "wait": {"type": "boolean", "description": "wait for the result (default true)"}}),
            json!(["id"]),
        ),
    ])
}

fn arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args[key]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

/// Runs one tool. An error is the tool's own failure (reported to the model as such).
fn run_tool(call: Call, name: &str, args: &Value) -> Result<Value, String> {
    use crate::routines_cli as cli;
    match name {
        "routines_projects" => {
            let snapshot = call("catalog.get", json!({}))?;
            cli::projects_data(&snapshot, args["id"].as_str().filter(|i| !i.is_empty()))
        }
        "routines_hosts" => {
            let snapshot = call("catalog.get", json!({}))?;
            Ok(json!(cli::hosts_data(&snapshot)))
        }
        "routines_list" => call("routines.list", json!({})),
        "routines_show" => call("routines.show", json!({"id": arg(args, "id")?})),
        "routines_validate" => cli::validate_with(call, arg(args, "dir")?),
        "routines_save" => cli::save_with(call, arg(args, "dir")?, Some(arg(args, "host")?)),
        "routines_dry_run" => cli::run_dry_with(
            call,
            arg(args, "id")?,
            args["wait"].as_bool().unwrap_or(true),
        ),
        other => Err(format!("unknown tool {other}")),
    }
}

/// One JSON-RPC message in, at most one out. Notifications get no answer.
fn handle(call: Call, message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message["method"].as_str().unwrap_or_default();
    let params = &message["params"];
    let ok = |result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result});
    Some(match method {
        "initialize" => ok(json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL_VERSION),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "sushiai-routines", "version": env!("CARGO_PKG_VERSION")},
            "instructions": "Build a routine: read projects and hosts, write the routine folder, \
                             then validate, save and dry-run it. Same rules as `sushiai routines help`.",
        })),
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({"tools": tools()})),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let known = tools()
                .as_array()
                .is_some_and(|t| t.iter().any(|t| t["name"] == name));
            if !known {
                return Some(json!({"jsonrpc": "2.0", "id": id,
                    "error": {"code": -32602, "message": format!("unknown tool {name}")}}));
            }
            let (text, is_error) = match run_tool(call, name, &params["arguments"]) {
                Ok(v) => (serde_json::to_string_pretty(&v).unwrap_or_default(), false),
                Err(e) => (e, true),
            };
            ok(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
        }
        other => json!({"jsonrpc": "2.0", "id": id,
            "error": {"code": -32601, "message": format!("method not found: {other}")}}),
    })
}

/// The stdio loop of `--routines`: one JSON-RPC line in, at most one line out.
fn serve_routines(call: Call, input: impl BufRead, mut output: impl Write) {
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(call, &message),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": Value::Null,
                "error": {"code": -32700, "message": format!("parse error: {e}")}})),
        };
        if let Some(response) = response {
            let mut out = response.to_string();
            out.push('\n');
            if output.write_all(out.as_bytes()).is_err() || output.flush().is_err() {
                break;
            }
        }
    }
}

pub fn run(args: impl Iterator<Item = String>) -> i32 {
    let args: Vec<String> = args.collect();
    let mut task = None;
    let mut read_only = false;
    let mut routines = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--task" if i + 1 < args.len() => {
                task = Some(args[i + 1].clone());
                i += 2;
            }
            "--read-only" => {
                read_only = true;
                i += 1;
            }
            "--routines" => {
                routines = true;
                i += 1;
            }
            other => {
                eprintln!("sushiai mcp: unknown argument {other}");
                return 2;
            }
        }
    }
    if routines {
        if task.is_some() || read_only {
            eprintln!("sushiai mcp: --routines takes no other option");
            return 2;
        }
        let call = |method: &str, params: Value| crate::routines_cli::call(method, params);
        serve_routines(&call, std::io::stdin().lock(), std::io::stdout());
        return 0;
    }
    let home = sushiai_daemon::Home::from_env();
    sushiai_orch::prompts::set_data_dir(&crate::orch_cli::data_dir(&home));
    // The daemon is reached on the first tool call: `initialize` and `tools/list` need none.
    let call = crate::daemon_client::orch_caller("mcp");
    sushiai_orch::mcp::serve_stdio(
        task,
        read_only,
        call,
        std::io::stdin().lock(),
        std::io::stdout(),
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(call: Call, message: Value) -> Value {
        handle(call, &message).expect("an answer")
    }

    #[test]
    fn the_door_lists_exactly_the_read_and_build_tools() {
        let none = |_: &str, _: Value| -> Result<Value, String> { Err("no daemon".into()) };
        let listed = ask(
            &none,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
        );
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "routines_projects",
                "routines_hosts",
                "routines_list",
                "routines_show",
                "routines_validate",
                "routines_save",
                "routines_dry_run"
            ]
        );
        // `ask` and `propose` belong to a run, not to the builder.
        let called = ask(
            &none,
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                   "params": {"name": "routines_ask", "arguments": {}}}),
        );
        assert!(called["error"].is_object());
    }

    #[test]
    fn a_save_goes_through_routines_save_with_the_host_and_reports_the_daemons_refusal() {
        let seen = std::sync::Mutex::new(Vec::new());
        let call = |method: &str, params: Value| -> Result<Value, String> {
            seen.lock()
                .unwrap()
                .push((method.to_string(), params.clone()));
            if method == "routines.save" {
                Err("this daemon is host build-host, not other-host".into())
            } else {
                Ok(json!({}))
            }
        };
        let out = ask(
            &call,
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                   "params": {"name": "routines_save",
                              "arguments": {"dir": "/tmp/r1", "host": "other-host"}}}),
        );
        assert_eq!(out["result"]["isError"], true);
        assert!(out["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not other-host"));
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].0, "routines.save");
        assert_eq!(seen[0].1["host"], "other-host");
        assert_eq!(seen[0].1["dir"], "/tmp/r1");
        // The host is required, like `--host` of the CLI.
        let missing = ask(
            &call,
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                   "params": {"name": "routines_save", "arguments": {"dir": "/tmp/r1"}}}),
        );
        assert_eq!(missing["result"]["isError"], true);
    }

    #[test]
    fn the_stdio_loop_answers_requests_and_ignores_notifications() {
        let none = |_: &str, _: Value| -> Result<Value, String> { Ok(json!({})) };
        let input = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n\
                     {\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n";
        let mut out = Vec::new();
        serve_routines(&none, input.as_bytes(), &mut out);
        let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(serde_json::from_str::<Value>(lines[0]).unwrap()["id"], 1);
    }
}
