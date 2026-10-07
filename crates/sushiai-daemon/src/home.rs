use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// The daemon's data directory: socket, lock, state file and holder sockets.
#[derive(Debug, Clone)]
pub struct Home {
    dir: PathBuf,
}

impl Home {
    pub fn new(dir: PathBuf) -> Self {
        Home { dir }
    }

    /// `$SUSHIAI_HOME`, else `~/.sushiai`.
    pub fn from_env() -> Self {
        if let Some(dir) = std::env::var_os("SUSHIAI_HOME") {
            return Home::new(dir.into());
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        Home::new(home.join(".sushiai"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("daemon.sock")
    }

    pub fn lock(&self) -> PathBuf {
        self.dir.join("daemon.lock")
    }

    pub fn state(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    /// The project and group replica, next to the state file.
    pub fn catalog(&self) -> PathBuf {
        self.dir.join("catalog.json")
    }

    /// Optional config: a first line naming this host in the desktop's catalog.
    pub fn host_file(&self) -> PathBuf {
        self.dir.join("host")
    }

    pub fn sessions(&self) -> PathBuf {
        self.dir.join("sessions")
    }

    /// The host name this daemon answers to for `projects.sync`: `$SUSHIAI_HOST`, else the first
    /// line of `<home>/host`, else the machine's hostname.
    pub fn host_name(&self) -> String {
        let named = |text: String| Some(text.trim().to_string()).filter(|t| !t.is_empty());
        std::env::var("SUSHIAI_HOST")
            .ok()
            .and_then(named)
            .or_else(|| {
                let text = fs::read_to_string(self.host_file()).ok()?;
                named(text.lines().next().unwrap_or_default().to_string())
            })
            .or_else(hostname)
            .unwrap_or_else(|| "localhost".into())
    }

    /// Creates missing directories with mode 0700. An existing one must be a real directory
    /// (not a symlink) owned by the current user; if group or others can enter it, it is
    /// tightened to 0700 (an installer may have made it under umask 022).
    pub fn ensure(&self) -> Result<()> {
        ensure_dir(&self.dir)?;
        ensure_dir(&self.sessions())
    }
}

fn ensure_dir(dir: &Path) -> Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) => {
            // SAFETY: getuid(2) has no preconditions.
            let uid = unsafe { libc::getuid() };
            if !meta.is_dir() || meta.uid() != uid {
                return Err(Error::UnsafeDir(dir.display().to_string()));
            }
            if meta.mode() & 0o077 != 0 {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname(2) writes at most `buf.len()` bytes into `buf`.
    let status = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if status != 0 {
        return None;
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..end]).into_owned()).filter(|h| !h.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn home_with_mode(mode: u32) -> (tempfile::TempDir, Home) {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(mode)).expect("chmod");
        let home = Home::new(dir.path().to_path_buf());
        (dir, home)
    }

    #[test]
    fn an_existing_home_open_to_group_or_others_is_tightened_to_0700() {
        for mode in [0o750, 0o705, 0o770, 0o755, 0o777] {
            let (dir, home) = home_with_mode(mode);
            fs::create_dir(home.sessions()).expect("sessions");
            fs::set_permissions(home.sessions(), fs::Permissions::from_mode(mode)).expect("chmod");
            home.ensure().expect("an owned directory is tightened");
            for d in [dir.path().to_path_buf(), home.sessions()] {
                let now = fs::metadata(&d).expect("meta").mode() & 0o777;
                assert_eq!(now, 0o700, "{} from {mode:o}", d.display());
            }
        }
    }

    #[test]
    fn a_home_that_is_a_symlink_or_a_file_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("target");
        fs::create_dir(&target).expect("mkdir");
        let before = fs::metadata(&target).expect("meta").mode();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        assert!(matches!(Home::new(link).ensure(), Err(Error::UnsafeDir(_))));
        let file = dir.path().join("file");
        fs::write(&file, "x").expect("write");
        assert!(matches!(Home::new(file).ensure(), Err(Error::UnsafeDir(_))));
        assert_eq!(
            fs::metadata(&target).expect("meta").mode(),
            before,
            "the target was chmod-ed"
        );
    }

    #[test]
    fn a_home_owned_by_someone_else_is_refused() {
        // Only root can chown; elsewhere /var/root-owned dirs are not creatable. /var/empty
        // (root-owned on macOS and most Linux systems) stands in when it exists.
        let foreign = Path::new("/var/empty");
        // SAFETY: getuid(2) has no preconditions.
        let uid = unsafe { libc::getuid() };
        match fs::metadata(foreign) {
            Ok(meta) if meta.uid() != uid && uid != 0 => {
                assert!(matches!(
                    Home::new(foreign.to_path_buf()).ensure(),
                    Err(Error::UnsafeDir(_))
                ));
            }
            _ => eprintln!("skipped: no foreign-owned directory to test with"),
        }
    }

    #[test]
    fn a_new_home_is_created_with_mode_0700() {
        let parent = tempfile::tempdir().expect("tempdir");
        let home = Home::new(parent.path().join("new"));
        home.ensure().expect("ensure");
        let mode = fs::metadata(home.dir()).expect("meta").mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn the_host_name_comes_from_the_config_file_and_falls_back_to_the_machine() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = Home::new(dir.path().to_path_buf());
        // Without the environment variable or the file: some non-empty name.
        if std::env::var_os("SUSHIAI_HOST").is_none() {
            assert!(!home.host_name().is_empty());
            fs::write(home.host_file(), "  devbox \nignored\n").expect("write");
            assert_eq!(home.host_name(), "devbox");
        }
    }
}
