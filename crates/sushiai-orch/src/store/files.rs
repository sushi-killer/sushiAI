use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_atomic(path, &json_bytes(value)?, None)
}

/// Atomically replaces a plain-text file the owner alone reads (the overseer
/// log), mode 0600.
pub fn write_text_private(path: &Path, text: &str) -> io::Result<()> {
    write_atomic(path, text.as_bytes(), Some(0o600))
}

/// As `write_json_atomic`, for JSON that carries a secret: mode 0600 from the
/// first byte (a run's settings embed its hook token).
pub fn write_json_private<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_atomic(path, &json_bytes(value)?, Some(0o600))
}

fn json_bytes<T: serde::Serialize>(value: &T) -> io::Result<Vec<u8>> {
    serde_json::to_vec_pretty(value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Atomically write a secret-bearing file (a per-run API key) restricted to
/// owner read/write (mode 0600): per-run key files are deleted once their run
/// ends, so this is always a small, short-lived plain-text file, never JSON.
pub fn write_secret_file(path: &Path, contents: &str) -> io::Result<()> {
    write_atomic(path, contents.as_bytes(), Some(0o600))
}

/// Writes `bytes` to a uniquely named temp file next to `path` and renames it
/// over `path`. `mode` is the file's mode from creation on (`None`: the
/// umask's default); the temp name is unique per call, so overlapping writes
/// of one path never collide.
fn write_atomic(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("data");
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let tmp_path = dir.join(format!(
        ".{}.tmp-{}-{}",
        file_name,
        std::process::id(),
        WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        let mut f = options.open(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}
