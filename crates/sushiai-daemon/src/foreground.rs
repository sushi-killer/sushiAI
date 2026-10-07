//! Finds an agent that the owner started by hand in a shell session: the process group that
//! owns the PTY's foreground is read from the shell's process entry, and its command line says
//! which agent it is. Detection only; the session stays a plain shell for hooks and status.
//! The agent's own conversation id is read from the files it keeps (Claude's session file,
//! Codex's rollout file), never guessed.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use crate::agent::AGENT_NAMES;

/// Programs that run an agent as a script (`node .../codex.js`).
const INTERPRETERS: &[&str] = &["node", "nodejs", "bun", "deno"];

/// The variables of the agent's process that say where its files are.
const WANTED_ENV: &[&str] = &["CLAUDE_CONFIG_DIR", "CODEX_HOME", "HOME"];

/// The npm package directory an agent's script lives in.
fn package_dir(agent: &str) -> Option<&'static str> {
    match agent {
        "claude" => Some("node_modules/@anthropic-ai/claude-code/"),
        "codex" => Some("node_modules/@openai/codex/"),
        "gemini" => Some("node_modules/@google/gemini-cli/"),
        _ => None,
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The agent a command line runs. The program name must be the agent's exactly. An interpreter
/// is looked through to its script: the script file is named like the agent (with or without
/// `.js`, `.mjs`, `.cjs`) or lies in the agent's npm package. Anything else is no agent, so
/// `claude-monitor` or `node claude-api-demo/server.js` are not.
pub fn agent_in(argv: &[String]) -> Option<&'static str> {
    let base = basename(argv.first()?);
    if let Some(found) = AGENT_NAMES.iter().copied().find(|a| *a == base) {
        return Some(found);
    }
    if !INTERPRETERS.contains(&base) {
        return None;
    }
    let mut args = argv.iter().skip(1).filter(|a| !a.starts_with('-'));
    let mut script = args.next()?;
    if base == "deno" && script == "run" {
        script = args.next()?;
    }
    let file = basename(script);
    AGENT_NAMES.iter().copied().find(|a| {
        file == *a
            || [".js", ".mjs", ".cjs"]
                .iter()
                .any(|ext| file.strip_suffix(ext) == Some(a))
            || package_dir(a).is_some_and(|dir| script.contains(dir))
    })
}

/// One field of `/proc/<pid>/stat` after the command name, counted from the state (0). The
/// name is in parentheses and may hold spaces or parentheses, so parsing starts after the
/// last `)`.
fn stat_field(stat: &str, n: usize) -> Option<&str> {
    stat[stat.rfind(')')? + 1..].split_whitespace().nth(n)
}

/// The foreground process group id (`tpgid`, field 8 of the stat line).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn tpgid_from_stat(stat: &str) -> Option<i64> {
    stat_field(stat, 5)?.parse().ok()
}

/// The process group id (`pgrp`, field 5 of the stat line).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn pgrp_from_stat(stat: &str) -> Option<u32> {
    stat_field(stat, 2)?.parse().ok()
}

/// NUL separated entries of a `/proc` file (`cmdline`, `environ`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn split_nul(raw: &[u8]) -> Vec<String> {
    let mut parts: Vec<&[u8]> = raw.split(|b| *b == 0).collect();
    if parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts
        .into_iter()
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

/// The argument vector and the environment from a macOS `KERN_PROCARGS2` buffer: `argc`
/// (native int), the executable path, NUL padding, `argc` NUL-terminated arguments, then the
/// environment entries.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn procargs2(raw: &[u8]) -> (Vec<String>, Vec<String>) {
    let Some(head) = raw.get(..4) else {
        return (Vec::new(), Vec::new());
    };
    let argc = i32::from_ne_bytes([head[0], head[1], head[2], head[3]]).max(0) as usize;
    let mut rest = &raw[4..];
    // Skip the executable path and the padding after it.
    let path_end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
    rest = &rest[path_end..];
    let start = rest.iter().position(|b| *b != 0).unwrap_or(rest.len());
    rest = &rest[start..];
    let mut entries = rest
        .split(|b| *b == 0)
        .map(|p| String::from_utf8_lossy(p).into_owned());
    let argv: Vec<String> = entries.by_ref().take(argc).collect();
    let env = entries.take_while(|e| !e.is_empty()).collect();
    (argv, env)
}

/// The wanted variables out of `KEY=value` entries.
fn pick_env(entries: &[String]) -> HashMap<String, String> {
    entries
        .iter()
        .filter_map(|e| e.split_once('='))
        .filter(|(k, v)| WANTED_ENV.contains(k) && !v.is_empty())
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// An agent in the foreground of a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub agent: &'static str,
    /// Leader of the foreground process group.
    pub leader: u32,
    pub cwd: Option<String>,
    /// `CLAUDE_CONFIG_DIR`, `CODEX_HOME` and `HOME` of the leader.
    pub env: HashMap<String, String>,
}

impl Seen {
    /// Where the agent keeps its files: its own variable, else `~/<sub>`. The leader's
    /// environment comes first, then the daemon's.
    pub fn config_dir(&self, var: &str, sub: &str) -> Option<PathBuf> {
        let get = |key: &str| {
            self.env
                .get(key)
                .cloned()
                .or_else(|| std::env::var(key).ok())
                .filter(|v| !v.is_empty())
        };
        match get(var) {
            Some(dir) => Some(dir.into()),
            None => get("HOME").map(|home| Path::new(&home).join(sub)),
        }
    }
}

/// The agent in the foreground of the shell `shell_pid`; None when the shell itself owns the
/// terminal, the foreground is no agent, or nothing can be read.
pub fn foreground(shell_pid: u32) -> Option<Seen> {
    let tpgid = u32::try_from(read_tpgid(shell_pid)?).ok()?;
    if tpgid == 0 || tpgid == shell_pid {
        return None;
    }
    let (argv, env) = read_proc(tpgid)?;
    Some(Seen {
        agent: agent_in(&argv)?,
        leader: tpgid,
        cwd: read_cwd(tpgid),
        env: pick_env(&env),
    })
}

/// The PTY child pid: what the holder reported, else the only child of the holder (an older
/// holder does not report it).
pub fn child_pid(reported: Option<u32>, holder: Option<u32>) -> Option<u32> {
    reported.or_else(|| only_child(holder?))
}

fn is_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The session id in Claude's `sessions/<pid>.json`. The file must name `pid` and hold a uuid.
pub fn session_from_json(text: &str, pid: u32) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    if value.get("pid")?.as_u64()? != u64::from(pid) {
        return None;
    }
    let id = value.get("sessionId")?.as_str()?;
    is_uuid(id).then(|| id.to_string())
}

/// Claude's own session id for the foreground group led by `leader`: its own file first, then
/// the file of any process of that group (the leader may be a shim). `pgrp_of` says which
/// group a pid is in.
pub fn claude_session(
    config_dir: &Path,
    leader: u32,
    pgrp_of: impl Fn(u32) -> Option<u32>,
) -> Option<String> {
    let dir = config_dir.join("sessions");
    let read = |pid: u32| {
        std::fs::read_to_string(dir.join(format!("{pid}.json")))
            .ok()
            .and_then(|text| session_from_json(&text, pid))
    };
    if let Some(id) = read(leader) {
        return Some(id);
    }
    let mut members: Vec<u32> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let pid: u32 = name.strip_suffix(".json")?.parse().ok()?;
            (pgrp_of(pid) == Some(leader)).then_some(pid)
        })
        .collect();
    members.sort_unstable();
    members.into_iter().find_map(read)
}

/// What the first line of a rollout file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rollout {
    Id(String),
    /// A sub-agent's conversation (it has a parent): not the owner's.
    Sub,
    Unreadable,
}

/// Reads `session_meta`.
pub fn rollout_from_line(line: &str) -> Rollout {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return Rollout::Unreadable;
    };
    let Some(payload) = value.get("payload") else {
        return Rollout::Unreadable;
    };
    let is_sub = payload
        .get("parent_thread_id")
        .is_some_and(|p| !p.is_null())
        || payload
            .get("source")
            .and_then(|s| s.get("subagent"))
            .is_some();
    if is_sub {
        return Rollout::Sub;
    }
    match payload.get("id").and_then(|id| id.as_str()) {
        Some(id) if is_uuid(id) => Rollout::Id(id.to_string()),
        _ => Rollout::Unreadable,
    }
}

/// The uuid at the end of `rollout-<time>-<uuid>.jsonl`, when `path` is such a file under
/// `<codex_home>/sessions`. Pure: no file is read.
pub fn rollout_path_id(path: &Path, codex_home: &Path) -> Option<String> {
    if !path.starts_with(codex_home.join("sessions")) {
        return None;
    }
    let stem = path
        .file_name()?
        .to_str()?
        .strip_prefix("rollout-")?
        .strip_suffix(".jsonl")?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    is_uuid(id).then(|| id.to_string())
}

/// The conversation a Codex process holds open: the rollout file among the open `files`. The
/// first line of the file names it; the file name is the fallback. A sub-agent's rollout is
/// skipped. A Codex that talks to the shared app server holds none, so it has no id.
pub fn codex_session(codex_home: &Path, files: &[PathBuf]) -> Option<String> {
    // An open file is listed by its real path; the home may be given through a link.
    let real = std::fs::canonicalize(codex_home).unwrap_or_else(|_| codex_home.to_path_buf());
    files.iter().find_map(|path| {
        let named = rollout_path_id(path, codex_home).or_else(|| rollout_path_id(path, &real))?;
        match first_rollout(path) {
            Rollout::Id(id) => Some(id),
            Rollout::Sub => None,
            Rollout::Unreadable => Some(named),
        }
    })
}

fn first_rollout(path: &Path) -> Rollout {
    let Ok(file) = std::fs::File::open(path) else {
        return Rollout::Unreadable;
    };
    let mut line = String::new();
    // The first line carries the agent's instructions: allow it to be large, but bounded.
    let read = std::io::BufReader::new(std::io::Read::take(file, 1 << 20)).read_line(&mut line);
    match read {
        Ok(_) => rollout_from_line(&line),
        Err(_) => Rollout::Unreadable,
    }
}

/// The files every process of the foreground group `pgrp` holds open.
pub fn group_open_files(pgrp: u32) -> Vec<PathBuf> {
    os::group_pids(pgrp)
        .into_iter()
        .flat_map(os::open_files)
        .collect()
}

#[cfg(target_os = "linux")]
mod os {
    pub fn read_tpgid(pid: u32) -> Option<i64> {
        super::tpgid_from_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }

    pub fn read_pgrp(pid: u32) -> Option<u32> {
        super::pgrp_from_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }

    pub fn read_proc(pid: u32) -> Option<(Vec<String>, Vec<String>)> {
        let argv = super::split_nul(&std::fs::read(format!("/proc/{pid}/cmdline")).ok()?);
        if argv.is_empty() {
            return None;
        }
        let env = std::fs::read(format!("/proc/{pid}/environ"))
            .map(|raw| super::split_nul(&raw))
            .unwrap_or_default();
        Some((argv, env))
    }

    pub fn read_cwd(pid: u32) -> Option<String> {
        let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
        Some(path.to_string_lossy().into_owned())
    }

    pub fn open_files(pid: u32) -> Vec<std::path::PathBuf> {
        let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| std::fs::read_link(e.path()).ok())
            .collect()
    }

    pub fn group_pids(pgrp: u32) -> Vec<u32> {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| read_pgrp(*pid) == Some(pgrp))
            .collect()
    }

    pub fn only_child(pid: u32) -> Option<u32> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children")).ok()?;
        let mut kids = text.split_whitespace();
        let only = kids.next()?.parse().ok()?;
        kids.next().is_none().then_some(only)
    }
}

#[cfg(target_os = "macos")]
mod os {
    fn bsdinfo(pid: u32) -> Option<libc::proc_bsdinfo> {
        // SAFETY: `info` is a plain C struct of `size` bytes that the call fills in.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let got = unsafe {
            libc::proc_pidinfo(
                i32::try_from(pid).ok()?,
                libc::PROC_PIDTBSDINFO,
                0,
                std::ptr::addr_of_mut!(info).cast(),
                size,
            )
        };
        (got == size).then_some(info)
    }

    pub fn read_tpgid(pid: u32) -> Option<i64> {
        Some(i64::from(bsdinfo(pid)?.e_tpgid))
    }

    pub fn read_pgrp(pid: u32) -> Option<u32> {
        Some(bsdinfo(pid)?.pbi_pgid)
    }

    /// What `PROC_PIDFDVNODEPATHINFO` fills in for a vnode descriptor: `proc_fileinfo`
    /// (24 bytes), then the vnode's info and path.
    #[repr(C)]
    struct FdPath {
        file: [u8; 24],
        vnode: libc::vnode_info_path,
    }

    pub fn open_files(pid: u32) -> Vec<std::path::PathBuf> {
        const PROC_PIDFDVNODEPATHINFO: libc::c_int = 2;
        let Ok(pid) = i32::try_from(pid) else {
            return Vec::new();
        };
        // SAFETY: each buffer is a plain C structure or array of the size passed with it; the
        // path inside `FdPath` is a NUL-terminated C string.
        unsafe {
            let size = libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0);
            if size <= 0 {
                return Vec::new();
            }
            let mut fds = vec![
                libc::proc_fdinfo {
                    proc_fd: 0,
                    proc_fdtype: 0
                };
                size as usize / std::mem::size_of::<libc::proc_fdinfo>() + 8
            ];
            let got = libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr().cast(),
                (fds.len() * std::mem::size_of::<libc::proc_fdinfo>()) as libc::c_int,
            );
            let count = (got.max(0) as usize) / std::mem::size_of::<libc::proc_fdinfo>();
            let mut files = Vec::new();
            for fd in fds.iter().take(count) {
                if fd.proc_fdtype != libc::PROX_FDTYPE_VNODE as u32 {
                    continue;
                }
                let mut info: FdPath = std::mem::zeroed();
                let want = std::mem::size_of::<FdPath>() as libc::c_int;
                let got = libc::proc_pidfdinfo(
                    pid,
                    fd.proc_fd,
                    PROC_PIDFDVNODEPATHINFO,
                    std::ptr::addr_of_mut!(info).cast(),
                    want,
                );
                if got == want {
                    let path = std::ffi::CStr::from_ptr(info.vnode.vip_path.as_ptr().cast());
                    files.push(std::path::PathBuf::from(
                        path.to_string_lossy().into_owned(),
                    ));
                }
            }
            files
        }
    }

    pub fn group_pids(pgrp: u32) -> Vec<u32> {
        let mut pids = [0i32; 256];
        // SAFETY: the buffer holds `size` bytes; the call returns how many pids it wrote.
        let count = unsafe {
            libc::proc_listpgrppids(
                i32::try_from(pgrp).unwrap_or(0),
                pids.as_mut_ptr().cast(),
                std::mem::size_of_val(&pids) as libc::c_int,
            )
        };
        pids.iter()
            .take(count.clamp(0, 256) as usize)
            .filter_map(|p| u32::try_from(*p).ok())
            .collect()
    }

    pub fn read_proc(pid: u32) -> Option<(Vec<String>, Vec<String>)> {
        let mut size: libc::size_t = 0;
        let mut mib = [
            libc::CTL_KERN,
            libc::KERN_PROCARGS2,
            i32::try_from(pid).ok()?,
        ];
        // SAFETY: the first call only asks for the size; the second fills a buffer of that size.
        unsafe {
            if libc::sysctl(
                mib.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            ) != 0
                || size == 0
            {
                return None;
            }
            let mut buf = vec![0u8; size];
            if libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            ) != 0
            {
                return None;
            }
            buf.truncate(size);
            let (argv, env) = super::procargs2(&buf);
            (!argv.is_empty()).then_some((argv, env))
        }
    }

    pub fn read_cwd(pid: u32) -> Option<String> {
        // SAFETY: `info` is a plain C struct of `size` bytes that the call fills in; its path
        // is a NUL-terminated C string inside it.
        unsafe {
            let mut info: libc::proc_vnodepathinfo = std::mem::zeroed();
            let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
            let got = libc::proc_pidinfo(
                i32::try_from(pid).ok()?,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                std::ptr::addr_of_mut!(info).cast(),
                size,
            );
            if got != size {
                return None;
            }
            let path = std::ffi::CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast());
            Some(path.to_string_lossy().into_owned()).filter(|p| !p.is_empty())
        }
    }

    pub fn only_child(pid: u32) -> Option<u32> {
        let mut pids = [0i32; 16];
        // SAFETY: the buffer holds `size` bytes; the call returns how many pids it wrote.
        let count = unsafe {
            libc::proc_listchildpids(
                i32::try_from(pid).ok()?,
                pids.as_mut_ptr().cast(),
                std::mem::size_of_val(&pids) as libc::c_int,
            )
        };
        (count == 1).then(|| u32::try_from(pids[0]).ok()).flatten()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod os {
    pub fn read_tpgid(_pid: u32) -> Option<i64> {
        None
    }
    pub fn read_pgrp(_pid: u32) -> Option<u32> {
        None
    }
    pub fn open_files(_pid: u32) -> Vec<std::path::PathBuf> {
        Vec::new()
    }
    pub fn group_pids(_pgrp: u32) -> Vec<u32> {
        Vec::new()
    }
    pub fn read_proc(_pid: u32) -> Option<(Vec<String>, Vec<String>)> {
        None
    }
    pub fn read_cwd(_pid: u32) -> Option<String> {
        None
    }
    pub fn only_child(_pid: u32) -> Option<u32> {
        None
    }
}

use os::{only_child, read_cwd, read_proc, read_tpgid};

/// The process group of `pid`.
pub fn pgrp_of(pid: u32) -> Option<u32> {
    os::read_pgrp(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| (*p).to_string()).collect()
    }

    // Synthetic values only.
    const ID: &str = "00000000-0000-4000-8000-000000000001";
    const ID2: &str = "00000000-0000-4000-8000-000000000002";

    #[test]
    fn the_program_name_decides_exactly() {
        assert_eq!(agent_in(&argv(&["claude"])), Some("claude"));
        assert_eq!(
            agent_in(&argv(&["/usr/local/bin/codex", "-m", "x"])),
            Some("codex")
        );
        assert_eq!(agent_in(&argv(&["gemini"])), Some("gemini"));
        assert_eq!(
            agent_in(&argv(&["cursor-agent", "--x"])),
            Some("cursor-agent")
        );
        for other in ["claude-monitor", "codex-usage", "claudette", "claude-code"] {
            assert_eq!(agent_in(&argv(&[other])), None, "{other}");
        }
    }

    #[test]
    fn an_interpreter_is_looked_through_to_its_script() {
        let by_name = argv(&["node", "/opt/bin/codex"]);
        assert_eq!(agent_in(&by_name), Some("codex"));
        let by_ext = argv(&["node", "--no-warnings", "/x/gemini.mjs"]);
        assert_eq!(agent_in(&by_ext), Some("gemini"));
        let package = argv(&[
            "node",
            "/opt/lib/node_modules/@anthropic-ai/claude-code/cli.js",
        ]);
        assert_eq!(agent_in(&package), Some("claude"));
        let codex = argv(&["bun", "/opt/lib/node_modules/@openai/codex/bin/run.js", "x"]);
        assert_eq!(agent_in(&codex), Some("codex"));
        assert_eq!(
            agent_in(&argv(&["deno", "run", "/x/claude.js"])),
            Some("claude")
        );
    }

    #[test]
    fn other_programs_are_not_agents() {
        for line in [
            &["grep", "claude", "notes"][..],
            &["vim", "codex.md"],
            &["-zsh"],
            &["node", "server.js", "--agent", "codex"],
            &["node", "/home/o/work/claude-api-demo/server.js"],
            &["node", "/home/o/codex-usage/index.js"],
            &["python3", "/x/codex.py"],
            &["/bin/sh"],
            &[],
        ] {
            assert_eq!(agent_in(&argv(line)), None, "{line:?}");
        }
    }

    #[test]
    fn stat_fields_are_read_after_the_last_parenthesis() {
        let plain = "4242 (zsh) S 4200 4243 4244 34816 5151 4194304 1 0 0 0 0 0 0 0 20 0 1 0 777 1";
        assert_eq!(tpgid_from_stat(plain), Some(5151));
        assert_eq!(pgrp_from_stat(plain), Some(4243));
        let tricky = "4242 (a) b (c d) S 4200 4242 4242 34816 -1 4194304";
        assert_eq!(tpgid_from_stat(tricky), Some(-1));
        assert_eq!(tpgid_from_stat("4242 (zsh) S 4200"), None);
        assert_eq!(tpgid_from_stat("garbage"), None);
    }

    #[test]
    fn proc_files_split_on_nul() {
        assert_eq!(
            split_nul(b"node\0/x/codex.js\0--flag\0"),
            argv(&["node", "/x/codex.js", "--flag"])
        );
        assert!(split_nul(b"").is_empty());
    }

    #[test]
    fn procargs2_gives_the_arguments_and_the_environment() {
        let mut raw = 2i32.to_ne_bytes().to_vec();
        raw.extend_from_slice(
            b"/usr/bin/node\0\0\0node\0/x/codex.js\0HOME=/h\0CODEX_HOME=/c\0\0\0",
        );
        let (args, env) = procargs2(&raw);
        assert_eq!(args, argv(&["node", "/x/codex.js"]));
        assert_eq!(env, argv(&["HOME=/h", "CODEX_HOME=/c"]));
        let picked = pick_env(&argv(&[
            "HOME=/h",
            "PATH=/p",
            "CODEX_HOME=/c",
            "CLAUDE_CONFIG_DIR=",
        ]));
        assert_eq!(picked.len(), 2);
        assert_eq!(picked["CODEX_HOME"], "/c");
        assert!(procargs2(&[1, 0]).0.is_empty());
    }

    #[test]
    fn a_session_file_must_match_the_pid_and_hold_a_uuid() {
        let ok = format!(r#"{{"pid":4242,"sessionId":"{ID}","cwd":"/x"}}"#);
        assert_eq!(session_from_json(&ok, 4242).as_deref(), Some(ID));
        assert_eq!(session_from_json(&ok, 4243), None);
        let bad = r#"{"pid":4242,"sessionId":"../../etc"}"#;
        assert_eq!(session_from_json(bad, 4242), None);
        assert_eq!(session_from_json("{", 4242), None);
        assert_eq!(session_from_json(r#"{"sessionId":"x"}"#, 4242), None);
    }

    fn write_session(dir: &Path, pid: u32, id: &str) {
        std::fs::create_dir_all(dir.join("sessions")).expect("mkdir");
        std::fs::write(
            dir.join("sessions").join(format!("{pid}.json")),
            format!(r#"{{"pid":{pid},"sessionId":"{id}"}}"#),
        )
        .expect("write");
    }

    #[test]
    fn claude_is_found_by_the_leader_or_by_any_process_of_its_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_session(dir.path(), 77, ID);
        let nobody = |_: u32| None;
        assert_eq!(claude_session(dir.path(), 77, nobody).as_deref(), Some(ID));
        assert_eq!(claude_session(dir.path(), 78, nobody), None);
        // The leader (78) is a shim; the file is that of its child 77 in the same group.
        let group = |pid: u32| (pid == 77).then_some(78);
        assert_eq!(claude_session(dir.path(), 78, group).as_deref(), Some(ID));
        write_session(dir.path(), 90, ID2);
        assert_eq!(claude_session(dir.path(), 78, group).as_deref(), Some(ID));
    }

    // The first line of a rollout: the real shape with every value replaced.
    fn line(id: &str, sub: bool) -> String {
        let extra = if sub {
            r#","parent_thread_id":"00000000-0000-4000-8000-0000000000ff","source":{"subagent":{"other":"x"}}"#
        } else {
            ""
        };
        format!(
            r#"{{"timestamp":"2030-01-02T03:04:05.678Z","ordinal":0,"type":"session_meta","payload":{{"session_id":"{id}","id":"{id}","timestamp":"2030-01-02T03:04:05.678Z","cwd":"/w","originator":"o","cli_version":"0.0.0","model_provider":"p","base_instructions":{{"text":"t"}}{extra}}}}}"#
        )
    }

    #[test]
    fn a_rollout_first_line_is_parsed_and_a_subagent_is_skipped() {
        assert_eq!(rollout_from_line(&line(ID, false)), Rollout::Id(ID.into()));
        assert_eq!(rollout_from_line(&line(ID, true)), Rollout::Sub);
        assert_eq!(rollout_from_line("{"), Rollout::Unreadable);
        assert_eq!(
            rollout_from_line(&line("not-a-uuid", false)),
            Rollout::Unreadable
        );
    }

    #[test]
    fn only_a_rollout_file_under_the_codex_home_names_a_conversation() {
        let home = Path::new("/h/.codex");
        let file = |p: &str| rollout_path_id(Path::new(p), home);
        let ok = format!("/h/.codex/sessions/2030/01/02/rollout-2030-01-02T03-04-05-{ID}.jsonl");
        assert_eq!(file(&ok).as_deref(), Some(ID));
        assert_eq!(file(&ok.replace("/h/.codex", "/other")), None);
        assert_eq!(file(&ok.replace("sessions", "archive")), None);
        assert_eq!(file(&ok.replace(".jsonl", ".txt")), None);
        assert_eq!(file(&ok.replace("rollout-", "log-")), None);
        assert_eq!(file(&ok.replace(ID, "not-a-uuid-at-all")), None);
        assert_eq!(file("/h/.codex/sessions/2030/01/02/rollout-.jsonl"), None);
    }

    fn put_rollout(home: &Path, id: &str, first: &str) -> PathBuf {
        let dir = home.join("sessions/2030/01/02");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join(format!("rollout-2030-01-02T03-04-05-{id}.jsonl"));
        std::fs::write(&path, format!("{first}\n{{\"x\":1}}\n")).expect("write");
        path
    }

    #[test]
    fn codex_has_the_conversation_whose_rollout_it_holds_open() {
        let home = tempfile::tempdir().expect("tempdir");
        let other = tempfile::tempdir().expect("tempdir");
        let held = put_rollout(home.path(), ID, &line(ID, false));
        let sub = put_rollout(home.path(), ID2, &line(ID2, true));
        let foreign = put_rollout(other.path(), ID2, &line(ID2, false));
        let sock = PathBuf::from("/tmp/some.sock");
        assert_eq!(
            codex_session(home.path(), &[sock.clone(), sub.clone(), held.clone()]).as_deref(),
            Some(ID)
        );
        // Nothing held, a sub-agent's file, or a file of another home: no id.
        assert_eq!(
            codex_session(home.path(), std::slice::from_ref(&sock)),
            None
        );
        assert_eq!(codex_session(home.path(), &[sub]), None);
        assert_eq!(codex_session(home.path(), &[foreign]), None);
        // The file name names it when the first line cannot be read.
        let bare = put_rollout(home.path(), "00000000-0000-4000-8000-000000000003", "{");
        assert_eq!(
            codex_session(home.path(), &[bare]).as_deref(),
            Some("00000000-0000-4000-8000-000000000003")
        );
    }

    #[test]
    fn the_open_files_of_a_process_group_are_listed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("held.txt");
        std::fs::write(&path, "x").expect("write");
        let held = std::fs::File::open(&path).expect("open");
        let files = group_open_files(pgrp_of(std::process::id()).expect("pgrp"));
        let real = std::fs::canonicalize(&path).expect("canonical");
        assert!(
            files
                .iter()
                .any(|f| std::fs::canonicalize(f).ok().as_ref() == Some(&real)),
            "{files:?}"
        );
        drop(held);
    }

    #[test]
    fn the_only_child_of_a_process_is_found() {
        let mut parent = std::process::Command::new("sh")
            .args(["-c", "sleep 5 & wait"])
            .spawn()
            .expect("spawn");
        let mut kid = None;
        for _ in 0..40 {
            kid = only_child(parent.id());
            if kid.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let kid = kid.expect("child of the shell");
        assert_eq!(pgrp_of(kid), pgrp_of(parent.id()));
        assert_eq!(child_pid(Some(7), Some(parent.id())), Some(7));
        let _ = std::process::Command::new("kill")
            .arg(kid.to_string())
            .status();
        let _ = parent.kill();
        let _ = parent.wait();
    }
}
