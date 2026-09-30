//! `orchd evolve --data <dir> [--socket <sock>] [--adopt <id>]`: asks a
//! running daemon to cluster the recorded signals and start proposer runs
//! (`evolution.run`), or with `--adopt` to mark a proposal adopted
//! (`evolution.adopt`). The socket defaults to `<data>/orchd.sock`. Prints
//! the daemon's JSON answer; exits 1 on an error and 2 on bad usage.

use crate::mcp;
use serde_json::{json, Value};
use std::path::PathBuf;

struct Options {
    data: PathBuf,
    socket: PathBuf,
    adopt: Option<String>,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut data = None;
    let mut socket = None;
    let mut adopt = None;
    let mut i = 0;
    while i < args.len() {
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{} needs a value", args[i]))?;
        match args[i].as_str() {
            "--data" => data = Some(PathBuf::from(value)),
            "--socket" => socket = Some(PathBuf::from(value)),
            "--adopt" => adopt = Some(value.clone()),
            other => return Err(format!("unknown argument {other}")),
        }
        i += 2;
    }
    let data = data.ok_or("--data is required")?;
    let socket = socket.unwrap_or_else(|| data.join("orchd.sock"));
    Ok(Options {
        data,
        socket,
        adopt,
    })
}

fn call(o: &Options) -> Result<Value, String> {
    let token = mcp::read_control_token(&o.data)?;
    match &o.adopt {
        Some(id) => mcp::call_orchd(&o.socket, &token, "evolution.adopt", json!({"id": id})),
        None => mcp::call_orchd(&o.socket, &token, "evolution.run", json!({})),
    }
}

pub fn run(args: &[String]) -> i32 {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("orchd evolve: {e}");
            eprintln!("usage: orchd evolve --data <dir> [--socket <sock>] [--adopt <id>]");
            return 2;
        }
    };
    match call(&o) {
        Ok(v) => {
            println!("{v}");
            0
        }
        Err(e) => {
            eprintln!("orchd evolve: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn evolve_socket_defaults_to_the_data_dir_and_adopt_is_optional() {
        let o = parse(&args(&["--data", "/d"])).unwrap();
        assert_eq!(o.socket, PathBuf::from("/d/orchd.sock"));
        assert!(o.adopt.is_none());
        let o = parse(&args(&["--data", "/d", "--socket", "/s", "--adopt", "x"])).unwrap();
        assert_eq!(o.socket, PathBuf::from("/s"));
        assert_eq!(o.adopt.as_deref(), Some("x"));
    }

    #[test]
    fn evolve_bad_usage_is_an_error() {
        assert!(parse(&args(&[])).is_err());
        assert!(parse(&args(&["--data"])).is_err());
        assert!(parse(&args(&["--data", "/d", "--bogus", "1"])).is_err());
    }
}
