//! Atomic writes to a file the crate owns, and the timestamped backups of one.
//!
//! One temp file in the target's directory, renamed over it: a reader sees the
//! old content or the new one, never a half-written file. This is the crate's
//! only writer, so a config file, a `SKILL.md` and the usage cache all get the
//! same guarantees.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Write `bytes` to `path` through a unique temp file in the same directory,
/// then rename it over `path`.
///
/// The temp file is created with the existing file's mode (0600 when there is
/// none), so the content is never readable by others, and it is fsynced before
/// the rename. `create_parent` creates a missing parent directory: a skill file
/// and the usage cache need it, a config file in a tool's home does not. On any
/// error the temp file is removed.
///
/// A new `SKILL.md` and a new usage cache now get 0600 and the fsync they did
/// not have before; an existing file keeps its own mode.
pub fn write_atomic(path: &Path, bytes: &[u8], create_parent: bool) -> std::io::Result<()> {
    if create_parent {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
    }
    // Unique per process and call: two writers at once must not share a temp file.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_file_name(format!(
        ".{}.tmp-{}-{n}",
        file_name(path)?,
        std::process::id()
    ));
    let res = (|| {
        use std::os::unix::fs::PermissionsExt;
        let mut f = create_new(path, &tmp)?;
        // The creation mode is cut by the umask; the old file's mode is kept exactly.
        f.set_permissions(fs::Permissions::from_mode(created_mode(path)))?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res
}

/// The mode the temp file is created with: the target's own, 0600 when it is new.
fn created_mode(path: &Path) -> u32 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).map_or(0o600, |meta| meta.mode() & 0o7777)
}

/// Create the temp file, replacing one a killed writer left behind under a pid
/// the system has since handed to us.
fn create_new(path: &Path, tmp: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let open = || {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(created_mode(path))
            .open(tmp)
    };
    match open() {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(tmp);
            open()
        }
        other => other,
    }
}

/// Copy `path` to `<name>.bak-<ts>` and keep the newest three.
pub fn backup_file(path: &Path, ts: u64) -> std::io::Result<PathBuf> {
    let b = path.with_file_name(format!("{}.bak-{ts}", file_name(path)?));
    fs::copy(path, &b)?;
    prune_backups(path);
    Ok(b)
}

/// Keep the newest `KEEP_BACKUPS` files named `<name>.bak-<number>`.
const KEEP_BACKUPS: usize = 3;

/// Drop every backup of `path` beyond the newest `KEEP_BACKUPS`. Best effort: a
/// path with no directory keeps every file.
pub fn prune_backups(path: &Path) {
    let Ok(name) = file_name(path) else { return };
    let prefix = format!("{name}.bak-");
    let Some(dir) = path.parent() else { return };
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut found: Vec<(u64, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let ts = name.strip_prefix(&prefix)?.parse::<u64>().ok()?;
            Some((ts, e.path()))
        })
        .collect();
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (_, old) in found.into_iter().skip(KEEP_BACKUPS) {
        let _ = fs::remove_file(old);
    }
}

/// The last component of `path`. The temp and backup names are built from it,
/// so a path without one is an error rather than a write to a surprising name.
fn file_name(path: &Path) -> std::io::Result<String> {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{} has no file name", path.display()),
            )
        })
}
