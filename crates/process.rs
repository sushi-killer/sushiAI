//! Native process inspection shared by the daemon and task engine, without a helper exec.

use std::path::PathBuf;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub pid: i32,
    pub parent_pid: i32,
    pub start_time: String,
    pub executable: Option<PathBuf>,
    pub zombie: bool,
    pub cpu_ms: u64,
    pub rss_bytes: u64,
}

pub fn alive(pid: i32) -> bool {
    info(pid).is_some_and(|p| !p.zombie)
}

#[cfg(target_os = "macos")]
pub fn list_pids() -> Vec<i32> {
    // SAFETY: libproc writes at most the supplied buffer size.
    unsafe {
        let count = libc::proc_listallpids(std::ptr::null_mut(), 0);
        if count <= 0 {
            return Vec::new();
        }
        let mut pids = vec![0i32; count as usize + 256];
        let got = libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(pids.as_slice()) as i32,
        );
        pids.truncate(got.max(0) as usize);
        pids
    }
}

#[cfg(target_os = "macos")]
pub fn info(pid: i32) -> Option<Info> {
    if pid <= 0 {
        return None;
    }
    let mut bsd = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: libproc initializes the struct; it is read only on a complete result.
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            bsd.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if got != size as i32 {
        return None;
    }
    let bsd = unsafe { bsd.assume_init() };
    let mut task = std::mem::MaybeUninit::<libc::proc_taskinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_taskinfo>();
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            task.as_mut_ptr().cast(),
            size as i32,
        )
    };
    let (cpu_ms, rss_bytes) = if got == size as i32 {
        let task = unsafe { task.assume_init() };
        (
            (task.pti_total_user + task.pti_total_system) / 1_000_000,
            task.pti_resident_size,
        )
    } else {
        (0, 0)
    };
    Some(Info {
        pid,
        parent_pid: bsd.pbi_ppid as i32,
        start_time: format!("{}:{:06}", bsd.pbi_start_tvsec, bsd.pbi_start_tvusec),
        executable: executable(pid),
        zombie: bsd.pbi_status == libc::SZOMB,
        cpu_ms,
        rss_bytes,
    })
}

#[cfg(target_os = "macos")]
pub fn parent_pid(pid: i32) -> Option<i32> {
    if pid <= 0 {
        return None;
    }
    let mut bsd = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: libproc initializes the struct; it is read only on a complete result.
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            bsd.as_mut_ptr().cast(),
            size as i32,
        )
    };
    (got == size as i32).then(|| unsafe { bsd.assume_init() }.pbi_ppid as i32)
}

#[cfg(target_os = "macos")]
pub fn executable(pid: i32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = vec![0u8; 4096];
    // SAFETY: the buffer has the supplied capacity.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf)))
}

#[cfg(target_os = "macos")]
pub fn argv(pid: i32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut buf = vec![0u8; 1024 * 1024];
    let mut size = buf.len();
    // SAFETY: sysctl writes at most size bytes and no new value is provided.
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    buf.truncate(size);
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    if argc < 0 {
        return None;
    }
    let mut tail = &buf[4..];
    tail = &tail[tail.iter().position(|b| *b == 0)? + 1..];
    while tail.first() == Some(&0) {
        tail = &tail[1..];
    }
    let mut args = Vec::new();
    for _ in 0..argc {
        let end = tail.iter().position(|b| *b == 0)?;
        args.push(String::from_utf8_lossy(&tail[..end]).into_owned());
        tail = &tail[end + 1..];
    }
    Some(args)
}

#[cfg(not(target_os = "macos"))]
pub fn list_pids() -> Vec<i32> {
    std::fs::read_dir("/proc")
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.file_name().to_str()?.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
pub fn parent_pid(pid: i32) -> Option<i32> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    text.get(text.rfind(')')? + 2..)?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

#[cfg(not(target_os = "macos"))]
pub fn executable(pid: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(not(target_os = "macos"))]
pub fn argv(pid: i32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        bytes
            .strip_suffix(&[0])
            .unwrap_or(&bytes)
            .split(|b| *b == 0)
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect(),
    )
}

#[cfg(not(target_os = "macos"))]
pub fn info(pid: i32) -> Option<Info> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<_> = text
        .get(text.rfind(')')? + 2..)?
        .split_whitespace()
        .collect();
    // SAFETY: sysconf has no pointer arguments.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if ticks <= 0 || page_size <= 0 {
        return None;
    }
    Some(Info {
        pid,
        parent_pid: fields.get(1)?.parse().ok()?,
        start_time: fields.get(19)?.to_string(),
        executable: executable(pid),
        zombie: *fields.first()? == "Z",
        cpu_ms: (fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?)
            * 1000
            / ticks as u64,
        rss_bytes: fields.get(21)?.parse::<u64>().ok()? * page_size as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspects_the_current_process_without_spawning_a_helper() {
        let pid = std::process::id() as i32;
        let before = info(pid).unwrap();
        assert!(!before.start_time.is_empty());
        assert!(!before.zombie);
        assert_eq!(
            before.executable.unwrap().canonicalize().unwrap(),
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
        assert_eq!(info(pid).unwrap().start_time, before.start_time);
        assert!(list_pids().contains(&pid));
        assert!(!argv(pid).unwrap().is_empty());
        assert!(info(i32::MAX).is_none());
        assert_eq!(
            parent_pid(pid),
            Some(std::os::unix::process::parent_id() as i32)
        );
        assert_eq!(parent_pid(i32::MAX), None);
    }
}
