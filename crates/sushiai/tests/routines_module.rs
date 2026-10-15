//! The routines module in the real daemon binary, with a fake `claude` on PATH: a routine is
//! saved and enabled, the daemon's scheduler runs it, the fake prints a valid result and
//! `routines.show` returns it.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use common::*;
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_sushiai");

fn fake_claude(dir: &Path, body: &str) {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let path = bin.join("claude");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn daemon_with(body: &str) -> Sandbox {
    daemon_with_env(body, &[])
}

fn daemon_with_env(body: &str, vars: &[(&str, String)]) -> Sandbox {
    let mut sandbox = Sandbox::new();
    for (k, v) in vars {
        sandbox.env.push((k.to_string(), v.clone()));
    }
    let modules = sandbox.home().join("modules");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::write(modules.join("routines.enabled"), b"").unwrap();
    fake_claude(sandbox.home(), body);
    let path = std::env::var("PATH").unwrap_or_default();
    sandbox.env.push((
        "PATH".into(),
        format!("{}:{path}", sandbox.home().join("bin").display()),
    ));
    sandbox.start_daemon();
    sandbox
}

fn routine() -> Value {
    json!({
        "id": "commit-digest",
        "name": "Commit digest",
        "where": {"host": "local"},
        "when": {"cron": "0 9 * * 1-5", "tz": "Europe/Berlin"},
        "agent": {"harness": "claude", "model": "sonnet", "effort": "low", "extraArgs": []},
        "limits": {"timeoutMin": 5},
    })
}

fn catalog() -> Value {
    json!({"hosts": [{"id": "local"}], "projects": []})
}

fn wait_result(client: &mut Client, id: &str) -> Value {
    let mut found = Value::Null;
    wait_until("the run to finish", || {
        match client.try_call("routines.result", json!({"id": id})) {
            Ok(v) if v["run"]["status"] != "running" => {
                found = v;
                true
            }
            _ => false,
        }
    });
    found
}

#[test]
fn a_saved_routine_runs_on_the_daemon_scheduler_and_ends_in_a_valid_result() {
    let answer = json!({"status": "done", "title": "Digest", "summary": "5 commits",
                        "actions": [{"kind": "slack.post", "text": "x", "applied": true}]});
    let envelope = json!({"type": "result", "structured_output": answer}).to_string();
    let sandbox = daemon_with(&format!("sleep 2\ncat <<'JSON'\n{envelope}\nJSON"));
    let mut client = sandbox.client();

    let caps = client.call("hello", json!({"protocol": 1, "client": "t"}));
    assert!(caps["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "routines"));

    let saved = client.call(
        "routines.save",
        json!({"routine": routine(), "brief": "Summarise the last 5 commits.",
               "catalog": catalog()}),
    );
    assert_eq!(saved["enabled"], false);
    client.call(
        "routines.set_enabled",
        json!({"id": "commit-digest", "enabled": true, "catalog": catalog()}),
    );
    // The entry is in the daemon's own schedule, with the key secrets never listed.
    let entries = client.call("schedule.list", json!({}));
    let entry = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "routine.commit-digest")
        .expect("schedule entry")
        .clone();
    assert_eq!(entry["cron"], "0 9 * * 1-5");
    assert_eq!(entry["create"]["background"], true);

    let fired = client.call("routines.run_now", json!({"id": "commit-digest"}));
    assert!(fired["session"].is_string(), "{fired}");

    let got = wait_result(&mut client, "commit-digest");
    assert_eq!(got["run"]["status"], "done", "{got}");
    assert_eq!(got["result"]["title"], "Digest");
    let shown = client.call("routines.show", json!({"id": "commit-digest"}));
    assert_eq!(shown["latest"]["status"], "done");
    // The result file is on disk, as the UI and the CLI read it.
    let file = sandbox
        .home()
        .join("routines/commit-digest/runs/1/result.json");
    let on_disk: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    assert_eq!(on_disk["summary"], "5 commits");
}

#[test]
fn the_session_is_a_background_session() {
    let sandbox = daemon_with("sleep 2\necho '{}'");
    let mut client = sandbox.client();
    client.call(
        "routines.save",
        json!({"routine": routine(), "brief": "b", "catalog": catalog()}),
    );
    client.call(
        "routines.set_enabled",
        json!({"id": "commit-digest", "enabled": true, "catalog": catalog()}),
    );
    client.call("routines.run_now", json!({"id": "commit-digest"}));
    wait_until("a background session", || {
        client
            .call("session.list", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["background"] == true)
    });
}

#[test]
fn cli_help_is_the_reference_and_dry_run_ask_asks_nobody() {
    let sandbox = Sandbox::new();
    let help = Command::new(BIN)
        .args(["routines", "help"])
        .env("SUSHIAI_HOME", sandbox.home())
        .output()
        .unwrap();
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    assert_eq!(text, sushiai_routines::help::HELP);

    // No daemon, no session variables: a dry run still answers and exits 0.
    let ask = Command::new(BIN)
        .args(["ask", "Post it?", "--option", "yes"])
        .env("SUSHIAI_HOME", sandbox.home())
        .env("SUSHIAI_DRY_RUN", "1")
        .output()
        .unwrap();
    assert!(ask.status.success());
    assert_eq!(
        String::from_utf8(ask.stdout).unwrap().trim(),
        "skipped (dry run)"
    );
}

#[test]
fn propose_queues_a_git_write_per_session_and_records_only_in_a_dry_run() {
    let sandbox = Sandbox::new();
    let dir = sandbox.home().join("routines/r1");
    let run = |dry: bool| {
        let mut c = Command::new(BIN);
        c.args(["propose", "commit", "--message", "fix", "--path", "a.txt"])
            .env("SUSHIAI_HOME", sandbox.home())
            .env("SUSHIAI_ROUTINE_DIR", &dir)
            .env("SUSHIAI_SESSION_ID", "sess-7");
        if dry {
            c.env("SUSHIAI_DRY_RUN", "1");
        }
        c.output().unwrap()
    };
    assert!(run(true).status.success());
    assert!(run(false).status.success());
    // Under work/, the one folder a sandboxed Codex run may write, one folder per session.
    let queue = dir.join("work/proposals/sess-7");
    let mut files: Vec<_> = std::fs::read_dir(&queue).unwrap().flatten().collect();
    files.sort_by_key(|f| f.path());
    assert_eq!(files.len(), 2);
    assert!(!dir.join("proposals").exists());
    let kinds: Vec<bool> = files
        .iter()
        .map(|f| {
            let v: Value = serde_json::from_slice(&std::fs::read(f.path()).unwrap()).unwrap();
            v["dryRun"].as_bool().unwrap()
        })
        .collect();
    assert!(kinds.contains(&true) && kinds.contains(&false));
    // A pr keeps its --path list, so the daemon can stage exactly those files.
    let pr = Command::new(BIN)
        .args([
            "propose", "pr", "--title", "T", "--path", "a.txt", "--path", "b/c.txt",
        ])
        .env("SUSHIAI_HOME", sandbox.home())
        .env("SUSHIAI_ROUTINE_DIR", &dir)
        .env("SUSHIAI_SESSION_ID", "sess-8")
        .output()
        .unwrap();
    assert!(pr.status.success());
    let queued = std::fs::read_dir(dir.join("work/proposals/sess-8"))
        .unwrap()
        .flatten()
        .next()
        .unwrap();
    let v: Value = serde_json::from_slice(&std::fs::read(queued.path()).unwrap()).unwrap();
    assert_eq!(v["paths"], json!(["a.txt", "b/c.txt"]));
    // Outside a run there is nothing to queue into.
    let out = Command::new(BIN)
        .args(["propose", "commit", "--message", "x"])
        .env("SUSHIAI_HOME", sandbox.home())
        .env_remove("SUSHIAI_ROUTINE_DIR")
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn a_scheduled_slot_found_by_the_daemon_clock_runs_the_routine_without_run_now() {
    // 2031-03-10 09:00 in Berlin (a Monday) is 08:00Z. The daemon's clock is moved to just
    // before it; the daemon's own tick then fires the slot, plus the entry's fixed jitter.
    let slot: u64 = 1_930_896_000;
    let id = (0..)
        .map(|i| format!("digest-{i}"))
        // The jitter is keyed by the schedule entry id, `routine.<id>`.
        .find(|id| fnv_jitter(&format!("routine.{id}"), 60) <= 2)
        .unwrap();
    let real = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let offset = (slot - 10) as i64 - real as i64;
    let envelope = |summary: &str| {
        json!({"type": "result", "structured_output": {
            "status": "done", "title": "Digest", "summary": summary,
            "actions": [{"kind": "k", "text": "t", "applied": true}]}})
        .to_string()
    };
    // The fake harness reports the ask lifetime the scheduler put in its environment. It
    // waits first, like a real harness: output printed before the module's attach is lost.
    let script = format!(
        "S=\"$SUSHIAI_SCHEDULE_NEXT_IN_SEC\"\nsleep 2\ncat <<JSON\n{}\nJSON",
        envelope("next in $S")
    );
    let sandbox = daemon_with_env(
        &script,
        &[("SUSHIAI_SCHEDULE_CLOCK_OFFSET_SECS", offset.to_string())],
    );
    let mut client = sandbox.client();
    let mut r = routine();
    r["id"] = json!(id);
    client.call(
        "routines.save",
        json!({"routine": r, "brief": "Summarise.", "catalog": catalog()}),
    );
    client.call(
        "routines.set_enabled",
        json!({"id": id, "enabled": true, "catalog": catalog()}),
    );
    // No run_now: nothing runs until the clock reaches the slot.
    let got = wait_result(&mut client, &id);
    assert_eq!(got["run"]["status"], "done", "{got}");
    assert_eq!(got["run"]["kind"], "scheduled");
    assert_eq!(got["run"]["scheduledFor"], slot * 1000);
    // The next weekday slot is a day away (the jitter of the entry shifts it by < 3 s).
    let summary = got["result"]["summary"].as_str().unwrap();
    let secs: u64 = summary
        .trim_start_matches("next in ")
        .trim()
        .parse()
        .unwrap();
    assert!((86_390..=86_400).contains(&secs), "{summary}");
}

/// The daemon's per-entry jitter: FNV-1a of the id modulo `cap + 1`.
fn fnv_jitter(id: &str, cap: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash % (cap + 1)
}

#[test]
fn access_is_granted_over_the_daemon_and_reaches_the_schedule_entry() {
    let sandbox = daemon_with("sleep 2\necho '{}'");
    let mut client = sandbox.client();
    let mut r = routine();
    r["access"] = json!([
        {"tool": "mcp__tracker__comment", "label": "Comment", "kind": "write", "granted": true},
        {"tool": "Bash(gh issue list:*)", "label": "List issues", "kind": "read"},
    ]);
    let saved = client.call(
        "routines.save",
        json!({"routine": r, "brief": "b", "catalog": catalog()}),
    );
    assert_eq!(saved["needsAccess"], 2, "{saved}");
    client.call(
        "routines.set_enabled",
        json!({"id": "commit-digest", "enabled": true, "catalog": catalog()}),
    );
    let granted = client.call(
        "routines.grant",
        json!({"id": "commit-digest", "tools": ["Bash(gh issue list:*)"]}),
    );
    assert_eq!(granted["needsAccess"], 1);
    let entries = client.call("schedule.list", json!({}));
    let entry = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "routine.commit-digest")
        .unwrap();
    let args: Vec<&str> = entry["create"]["extraArgs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let allow = args[args.iter().position(|a| *a == "--allowedTools").unwrap() + 1];
    assert!(allow.contains("Bash(gh issue list:*)"), "{allow}");
    assert!(!allow.contains("tracker"), "{allow}");
    let shown = client.call("routines.show", json!({"id": "commit-digest"}));
    assert_eq!(shown["needsAccess"], 1);
    assert_eq!(shown["routine"]["howItRuns"]["mode"], "ask-first");
}

#[test]
fn the_mcp_door_builds_a_routine_with_the_cli_rules() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;

    let sandbox = daemon_with("sleep 2\necho '{}'");
    let folder = sandbox.home().join("draft");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("routine.json"), routine().to_string()).unwrap();
    std::fs::write(folder.join("brief.md"), "Summarise the commits.").unwrap();

    let mut child = Command::new(BIN)
        .args(["mcp", "--routines"])
        .env("SUSHIAI_HOME", sandbox.home())
        .env("SUSHIAI_SOCKET", sandbox.socket())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut rpc = |id: u64, method: &str, params: Value| -> Value {
        let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(stdin, "{line}").unwrap();
        let mut answer = String::new();
        out.read_line(&mut answer).unwrap();
        serde_json::from_str(&answer).unwrap()
    };
    let tool = |name: &str, args: Value| json!({"name": name, "arguments": args});

    let init = rpc(1, "initialize", json!({}));
    assert_eq!(init["result"]["serverInfo"]["name"], "sushiai-routines");
    let listed = rpc(2, "tools/list", json!({}));
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 7);

    let report = rpc(
        3,
        "tools/call",
        tool("routines_validate", json!({"dir": folder})),
    );
    assert_eq!(report["result"]["isError"], false, "{report}");
    let body: Value =
        serde_json::from_str(report["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["ok"], true, "{body}");

    // The CLI's host rule: this daemon must be the host it is saved for.
    let wrong = rpc(
        4,
        "tools/call",
        tool(
            "routines_save",
            json!({"dir": folder, "host": "somewhere-else"}),
        ),
    );
    assert_eq!(wrong["result"]["isError"], true, "{wrong}");
    let saved = rpc(
        5,
        "tools/call",
        tool("routines_save", json!({"dir": folder, "host": "local"})),
    );
    assert_eq!(saved["result"]["isError"], false, "{saved}");
    let listed = rpc(6, "tools/call", tool("routines_list", json!({})));
    let text = listed["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("commit-digest"), "{text}");
    let hosts = rpc(7, "tools/call", tool("routines_hosts", json!({})));
    assert_eq!(hosts["result"]["isError"], false);
    drop(stdin);
    let _ = child.wait();
}
