//! The stable path agents put in their hook commands: `<home>/bin/sushiai`, a symlink to the
//! running binary. An upgrade moves the binary; the link keeps hook commands valid.

use std::fs;
use std::io;
use std::os::unix::fs::{symlink, DirBuilderExt};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Created,
    Replaced,
    Unchanged,
    /// Something that is not a symlink sits at the path. It is never overwritten.
    Kept,
}

/// The running binary with every symlink resolved. On macOS `current_exe` returns the path
/// the process was started through, which may be the stable link itself.
pub fn real_exe() -> io::Result<std::path::PathBuf> {
    fs::canonicalize(std::env::current_exe()?)
}

/// Makes `<base>/bin/sushiai` point at the current executable. A missing `bin` directory is
/// created with mode 0700; an existing one is left as it is. A stale symlink is replaced
/// atomically; a regular file is reported as `Kept`.
pub fn ensure_bin_link(base: &Path) -> io::Result<Link> {
    let dir = base.join("bin");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    let link = dir.join("sushiai");
    let exe = real_exe()?;
    // A binary that really lives in `bin` must not be replaced by a link to itself.
    if exe.starts_with(fs::canonicalize(&dir)?) {
        return Ok(Link::Kept);
    }
    let replacing = match fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if fs::canonicalize(&link).is_ok_and(|target| target == exe) {
                return Ok(Link::Unchanged);
            }
            true
        }
        Ok(_) => return Ok(Link::Kept),
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e),
    };
    let tmp = dir.join(format!("sushiai.tmp-{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    symlink(&exe, &tmp)?;
    fs::rename(&tmp, &link)?;
    Ok(if replacing {
        Link::Replaced
    } else {
        Link::Created
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_replaces_and_never_overwrites_a_file() {
        let base = tempfile::tempdir().expect("tempdir");
        assert_eq!(ensure_bin_link(base.path()).expect("link"), Link::Created);
        assert_eq!(ensure_bin_link(base.path()).expect("link"), Link::Unchanged);
        let link = base.path().join("bin/sushiai");
        fs::remove_file(&link).expect("rm");
        symlink("/nonexistent/old-sushiai", &link).expect("stale");
        assert_eq!(ensure_bin_link(base.path()).expect("link"), Link::Replaced);
        assert_eq!(
            fs::read_link(&link).expect("read"),
            real_exe().expect("exe")
        );
        fs::remove_file(&link).expect("rm");
        fs::write(&link, "mine").expect("file");
        assert_eq!(ensure_bin_link(base.path()).expect("link"), Link::Kept);
        assert_eq!(fs::read_to_string(&link).expect("read"), "mine");
    }
}
