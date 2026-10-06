//! `sushiai mcp [--task <id> | --read-only]`: the stdio MCP server an agent attaches to. The
//! tool surface and JSON-RPC handling live in `sushiai-orch`; this only supplies the
//! transport: each tool call becomes an `orch.*` request to this home's daemon, started on
//! first use like `sushiai proxy` does.

use std::cell::RefCell;

use crate::orch_cli::Conn;

pub fn run(args: impl Iterator<Item = String>) -> i32 {
    let args: Vec<String> = args.collect();
    let mut task = None;
    let mut read_only = false;
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
            other => {
                eprintln!("sushiai mcp: unknown argument {other}");
                return 2;
            }
        }
    }
    let home = sushiai_daemon::Home::from_env();
    sushiai_orch::prompts::set_data_dir(&crate::orch_cli::data_dir(&home));
    // Connect on the first tool call: `initialize` and `tools/list` need no daemon.
    let conn: RefCell<Option<Conn>> = RefCell::new(None);
    let call = Box::new(move |method: &str, params: serde_json::Value| {
        let method = format!("orch.{method}");
        let mut slot = conn.borrow_mut();
        // A dropped connection is retried once on a fresh one.
        for attempt in 0..2 {
            if slot.is_none() {
                *slot = Some(Conn::via_proxy("mcp")?);
            }
            let Some(live) = slot.as_mut() else { break };
            match live.call(&method, params.clone()) {
                Err(crate::orch_cli::CallError::Closed(e)) if attempt == 1 => return Err(e),
                Err(crate::orch_cli::CallError::Closed(_)) => *slot = None,
                Err(crate::orch_cli::CallError::Rpc(e)) => return Err(e),
                Ok(v) => return Ok(v),
            }
        }
        Err("the daemon connection failed".to_string())
    });
    sushiai_orch::mcp::serve_stdio(
        task,
        read_only,
        call,
        std::io::stdin().lock(),
        std::io::stdout(),
    );
    0
}
