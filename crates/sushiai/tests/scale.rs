//! Load run for session hibernation: 10 claude, 10 codex and 10 shell sessions, each printing
//! 2000 lines of 200 columns, in a real daemon under a temp home. It measures the daemon and
//! the holders awake, after the agents went to sleep, and after a hard restart, and checks the
//! step 3a budget (a holder <= 5 MB RSS, an idle daemon < 1 % CPU).
//!
//! Slow (about five minutes), so it is ignored by default:
//! `cargo test -p sushiai --test scale -- --ignored --nocapture`

mod common;

use std::fs;
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;

use common::rig::*;
use common::*;
use serde_json::{json, Value};

const PER_KIND: usize = 10;
const LINES: u32 = 2000;
const IDLE_SECS: u64 = 60;
const HOLDER_BUDGET_KB: u64 = 5 * 1024;
const DAEMON_CPU_BUDGET_PERCENT: f64 = 1.0;

/// 2000 lines of 200 columns, a marker, then a quiet long-running process.
const SHELL: &str = "i=0; while [ $i -lt 2000 ]; do printf 'shell-%04d %0190d\\n' $i 0; \
                     i=$((i+1)); done; echo shell-flood-done; exec sleep 100000";

fn ps_field(pid: u32, field: &str) -> String {
    let out = Command::new("ps")
        .args(["-o", &format!("{field}="), "-p", &pid.to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn rss_kb(pid: u32) -> u64 {
    ps_field(pid, "rss").parse().unwrap_or(0)
}

/// CPU seconds a process has used: `[[dd-]hh:]mm:ss[.cc]`.
fn cpu_secs(pid: u32) -> f64 {
    let text = ps_field(pid, "cputime");
    let text = text.split('-').next_back().unwrap_or_default();
    text.split(':')
        .try_fold(0.0, |acc, part| part.parse::<f64>().map(|n| acc * 60.0 + n))
        .unwrap_or(0.0)
}

fn open_fds(pid: u32) -> usize {
    if let Ok(dir) = fs::read_dir(format!("/proc/{pid}/fd")) {
        return dir.count();
    }
    let out = Command::new("lsof")
        .args(["-n", "-P", "-p", &pid.to_string()])
        .output()
        .expect("lsof");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .count()
        .saturating_sub(1)
}

struct Row {
    phase: String,
    holders: usize,
    daemon_rss_kb: u64,
    holder_sum_kb: u64,
    holder_max_kb: u64,
    fds: usize,
    cpu_percent: f64,
}

/// Samples the daemon and its holders, then the daemon's CPU over `IDLE_SECS` of nothing.
fn measure(phase: &str, sandbox: &Sandbox, daemon: u32) -> Row {
    let holders = sandbox.holders();
    let sizes: Vec<u64> = holders.iter().map(|pid| rss_kb(*pid)).collect();
    let before = cpu_secs(daemon);
    sleep(Duration::from_secs(IDLE_SECS));
    let used = cpu_secs(daemon) - before;
    Row {
        phase: phase.to_string(),
        holders: holders.len(),
        daemon_rss_kb: rss_kb(daemon),
        holder_sum_kb: sizes.iter().sum(),
        holder_max_kb: sizes.iter().copied().max().unwrap_or(0),
        fds: open_fds(daemon),
        cpu_percent: used * 100.0 / IDLE_SECS as f64,
    }
}

fn mb(kb: u64) -> String {
    format!("{:.1}", kb as f64 / 1024.0)
}

fn print_table(rows: &[Row]) {
    println!("\n| phase | holders | daemon RSS MB | holders sum MB | holder max MB | daemon fds | daemon CPU % (60 s idle) |");
    println!("|---|---:|---:|---:|---:|---:|---:|");
    for r in rows {
        println!(
            "| {} | {} | {} | {} | {} | {} | {:.2} |",
            r.phase,
            r.holders,
            mb(r.daemon_rss_kb),
            mb(r.holder_sum_kb),
            mb(r.holder_max_kb),
            r.fds,
            r.cpu_percent
        );
    }
}

fn statuses(client: &mut Client) -> Vec<String> {
    client
        .call("session.list", Value::Null)
        .as_array()
        .map(|list| {
            list.iter()
                .map(|s| s["status"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn count(client: &mut Client, status: &str) -> usize {
    statuses(client).iter().filter(|s| *s == status).count()
}

fn text_of(client: &mut Client, id: &str) -> String {
    client.call("session.read", json!({"id": id}))["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
#[ignore = "load run, about five minutes"]
fn thirty_sessions_awake_asleep_and_after_a_hard_restart() {
    let rig = Rig::flooding("claude", LINES);
    rig.install("codex", LINES);
    rig.install_codex_hooks();
    let mut sandbox = rig.sandbox(None);
    let daemon = sandbox.start_daemon();
    let mut client = sandbox.client();

    let mut agents = Vec::new();
    for kind in ["claude", "codex"] {
        for _ in 0..PER_KIND {
            agents.push((kind, rig.create_as(&mut client, kind)));
        }
    }
    let shells: Vec<String> = (0..PER_KIND)
        .map(|_| sandbox.create(&mut client, SHELL))
        .collect();
    for (_, id) in &agents {
        wait_idle(&mut client, id);
    }
    for id in &shells {
        wait_until("a shell to finish printing", 60, || {
            text_of(&mut client, id).contains("shell-flood-done")
        });
    }
    let mut rows = vec![measure("awake: 20 agents + 10 shells", &sandbox, daemon)];

    // From here an idle agent sleeps after two seconds.
    client.call("daemon.configure", json!({"hibernateAfterSecs": 2}));
    wait_until("20 sessions asleep", 120, || {
        count(&mut client, "hibernated") == 2 * PER_KIND
    });
    wait_until("only the shell holders left", 30, || {
        sandbox.holders().len() == PER_KIND
    });
    assert_eq!(count(&mut client, "running"), PER_KIND, "the shells run on");
    rows.push(measure(
        "asleep: 20 hibernated, 10 shells",
        &sandbox,
        daemon,
    ));

    // Typing wakes one agent and arrives in order. Nothing sleeps again from here.
    client.call("daemon.configure", json!({"hibernateAfterSecs": 0}));
    let (_, first) = &agents[0];
    client.type_text(first, "abc");
    wait_until("abc to arrive", 30, || rig.typed() == "abc");
    wait_status(&mut client, first, "running");

    // A reboot: every holder, its child and the daemon die at once.
    let all = processes();
    let holders = sandbox.holders();
    for holder in &holders {
        for (child, ppid, _) in &all {
            if ppid == holder {
                kill(*child, "-KILL");
            }
        }
        kill(*holder, "-KILL");
    }
    kill(daemon, "-KILL");
    wait_until("the processes to end", 15, || {
        !alive(daemon) && holders.iter().all(|h| !alive(*h))
    });
    let daemon = sandbox.start_daemon();
    let mut client = sandbox.client();
    wait_until("20 hibernated and 10 exited", 60, || {
        count(&mut client, "hibernated") == 2 * PER_KIND && count(&mut client, "exited") == PER_KIND
    });
    for id in [&agents[0].1, &agents[PER_KIND].1] {
        client.call("session.wake", json!({"id": id}));
        wait_status(&mut client, id, "running");
    }
    rows.push(measure("after a hard restart + 2 woken", &sandbox, daemon));

    print_table(&rows);
    for row in &rows {
        assert!(
            row.holder_max_kb <= HOLDER_BUDGET_KB,
            "{}: a holder uses {} MB",
            row.phase,
            mb(row.holder_max_kb)
        );
        assert!(
            row.cpu_percent < DAEMON_CPU_BUDGET_PERCENT,
            "{}: the idle daemon uses {:.2} % CPU",
            row.phase,
            row.cpu_percent
        );
    }
}
