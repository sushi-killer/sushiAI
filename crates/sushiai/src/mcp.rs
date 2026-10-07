//! `sushiai mcp [--task <id> | --read-only]`: the stdio MCP server an agent attaches to. The
//! tool surface and JSON-RPC handling live in `sushiai-orch`; this only supplies the
//! transport: each tool call becomes an `orch.*` request to this home's daemon, started on
//! first use like `sushiai proxy` does.

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
