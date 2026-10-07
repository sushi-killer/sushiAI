//! `sushiai orch evolve [--adopt <id>]`: asks the daemon to cluster the recorded signals and
//! start proposer runs (`evolution.run`), or with `--adopt` to mark a proposal adopted
//! (`evolution.adopt`). Prints the daemon's JSON answer; exits 1 on an error and 2 on bad
//! usage.

use crate::mcp::Caller;
use serde_json::json;

fn parse(args: &[String]) -> Result<Option<String>, String> {
    match args {
        [] => Ok(None),
        [flag, id] if flag == "--adopt" => Ok(Some(id.clone())),
        [flag] => Err(format!("{flag} needs a value")),
        [other, ..] => Err(format!("unknown argument {other}")),
    }
}

/// `call` sends one `orch.*` request (method without the prefix) to the daemon.
pub fn run(args: &[String], call: &Caller) -> i32 {
    let adopt = match parse(args) {
        Ok(adopt) => adopt,
        Err(e) => {
            eprintln!("sushiai orch evolve: {e}");
            eprintln!("usage: sushiai orch evolve [--adopt <id>]");
            return 2;
        }
    };
    let result = match adopt {
        Some(id) => call("evolution.adopt", json!({"id": id})),
        None => call("evolution.run", json!({})),
    };
    match result {
        Ok(v) => {
            println!("{v}");
            0
        }
        Err(e) => {
            eprintln!("sushiai orch evolve: {e}");
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
    fn adopt_is_optional() {
        assert_eq!(parse(&args(&[])), Ok(None));
        assert_eq!(parse(&args(&["--adopt", "x"])), Ok(Some("x".to_string())));
    }

    #[test]
    fn bad_usage_is_an_error() {
        assert!(parse(&args(&["--adopt"])).is_err());
        assert!(parse(&args(&["--bogus", "1"])).is_err());
        assert!(parse(&args(&["--data", "/d"])).is_err());
    }
}
