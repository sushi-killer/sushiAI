//! CPU time of an agent's descendants from native process data.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

/// `[[dd-]hh:]mm:ss[.cc]` as milliseconds.
pub fn parse_cpu_time(text: &str) -> Option<u64> {
    let (days, clock) = match text.split_once('-') {
        Some((d, rest)) => (d.parse::<u64>().ok()?, rest),
        None => (0, text),
    };
    let mut seconds = 0.0_f64;
    for part in clock.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(((days * 86_400) as f64 * 1000.0 + seconds * 1000.0).round() as u64)
}

/// Total CPU ms of everything below `root` (not `root` itself) in a `pid ppid time` table.
pub fn descendants_cpu_ms(table: &str, root: u32) -> u64 {
    let mut children: HashMap<u32, Vec<(u32, u64)>> = HashMap::new();
    for line in table.lines() {
        let mut parts = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(time)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        if let (Ok(pid), Ok(ppid), Some(ms)) = (pid.parse(), ppid.parse(), parse_cpu_time(time)) {
            children.entry(ppid).or_default().push((pid, ms));
        }
    }
    let mut total = 0;
    let mut queue = VecDeque::from([root]);
    while let Some(pid) = queue.pop_front() {
        for (child, ms) in children.get(&pid).into_iter().flatten() {
            total += ms;
            queue.push_back(*child);
        }
    }
    total
}

/// CPU time of descendants, excluding the root itself.
pub async fn sample(root: u32) -> Option<u64> {
    let run = tokio::task::spawn_blocking(move || {
        let table = crate::process::list_pids()
            .into_iter()
            .filter_map(crate::process::info)
            .map(|p| format!("{} {} {}", p.pid, p.parent_pid, p.cpu_ms as f64 / 1000.0))
            .collect::<Vec<_>>()
            .join("\n");
        descendants_cpu_ms(&table, root)
    });
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .ok()?
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_times_parse_in_the_macos_and_linux_shapes() {
        assert_eq!(parse_cpu_time("0:00.03"), Some(30));
        assert_eq!(parse_cpu_time("1:02.50"), Some(62_500));
        assert_eq!(parse_cpu_time("00:00:07"), Some(7_000));
        assert_eq!(parse_cpu_time("1:00:00"), Some(3_600_000));
        assert_eq!(parse_cpu_time("2-00:00:01"), Some(172_801_000));
        assert_eq!(parse_cpu_time("junk"), None);
    }

    #[test]
    fn only_the_processes_below_the_root_count() {
        let table = "\
  10     1  0:09.00
  20    10  0:01.00
  21    10  0:00.50
  30    21  0:00.25
  40     1  5:00.00
";
        assert_eq!(descendants_cpu_ms(table, 10), 1_750);
        assert_eq!(descendants_cpu_ms(table, 20), 0);
        assert_eq!(descendants_cpu_ms(table, 99), 0);
    }
}
