//! The launch of an agent session (its variables, `claudeSettings`, model and extra
//! arguments), sealed on disk so the session can be woken after its process is gone. The file
//! is `sessions/<id>.launch` (0600): `SLK1`, a random 12-byte nonce, then ChaCha20-Poly1305
//! text with the session id as associated data (a file moved to another id does not open).
//!
//! The key lives in the macOS login keychain (service `sushiai-launch-key`) for the default
//! home, else in a key file (0600): `SUSHIAI_LAUNCH_KEY_FILE`, or `<home>/keys/launch.key`. A
//! daemon whose home is not `~/.sushiai` never touches the keychain. Headless hosts have no
//! keyring, so the key file sits beside the data it protects: a stolen copy of the file alone
//! stays sealed, a stolen home does not. Nothing here is ever logged.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use sushiai_protocol::SessionCreate;

use crate::agent::random_hex;
use crate::home::Home;
use crate::registry::unhex;

const MAGIC: &[u8; 4] = b"SLK1";
const NONCE_LEN: usize = 12;
const KEYCHAIN_SERVICE: &str = "sushiai-launch-key";

/// What a wake needs besides the session record. No `Debug`: it holds secrets.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub claude_settings: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl Launch {
    pub fn of(p: &SessionCreate) -> Launch {
        Launch {
            env: p.env.clone(),
            claude_settings: p.claude_settings.clone(),
            model: p.model.clone(),
            extra_args: p.extra_args.clone(),
        }
    }
}

enum KeySource {
    File(PathBuf),
    Keychain,
}

pub struct LaunchStore {
    home: Home,
    source: KeySource,
    key: Mutex<Option<[u8; 32]>>,
}

fn is_default_home(home: &Home) -> bool {
    let default = std::env::var_os("HOME").map(|h| Path::new(&h).join(".sushiai"));
    default.is_some_and(|d| d == home.dir())
}

impl LaunchStore {
    pub fn new(home: &Home) -> LaunchStore {
        let from_env = std::env::var_os("SUSHIAI_LAUNCH_KEY_FILE").filter(|p| !p.is_empty());
        let source = match from_env {
            Some(path) => KeySource::File(path.into()),
            None if cfg!(target_os = "macos") && is_default_home(home) => KeySource::Keychain,
            None => KeySource::File(home.launch_key_file()),
        };
        LaunchStore {
            home: home.clone(),
            source,
            key: Mutex::new(None),
        }
    }

    pub fn exists(&self, id: &str) -> bool {
        self.home.launch_file(id).is_file()
    }

    pub fn seal(&self, id: &str, launch: &Launch) -> io::Result<()> {
        let key = self.key()?;
        let plain = serde_json::to_vec(launch).map_err(io::Error::other)?;
        let nonce_bytes =
            unhex(&random_hex(NONCE_LEN)?).ok_or_else(|| io::Error::other("nonce"))?;
        let sealed = ChaCha20Poly1305::new(Key::from_slice(&key))
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: &plain,
                    aad: id.as_bytes(),
                },
            )
            .map_err(|_| io::Error::other("cannot seal the launch"))?;
        let mut bytes = MAGIC.to_vec();
        bytes.extend(&nonce_bytes);
        bytes.extend(sealed);
        let path = self.home.launch_file(id);
        let tmp = path.with_extension("launch.tmp");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    }

    /// An error means the launch is not there or cannot be opened: the session cannot wake.
    pub fn open(&self, id: &str) -> io::Result<Launch> {
        let bytes = fs::read(self.home.launch_file(id))?;
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "the launch cannot be opened");
        if bytes.len() < MAGIC.len() + NONCE_LEN || &bytes[..MAGIC.len()] != MAGIC {
            return Err(bad());
        }
        let (nonce, sealed) = bytes[MAGIC.len()..].split_at(NONCE_LEN);
        let key = self.key()?;
        let plain = ChaCha20Poly1305::new(Key::from_slice(&key))
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: sealed,
                    aad: id.as_bytes(),
                },
            )
            .map_err(|_| bad())?;
        serde_json::from_slice(&plain).map_err(|_| bad())
    }

    pub fn delete(&self, id: &str) {
        let _ = fs::remove_file(self.home.launch_file(id));
    }

    fn key(&self) -> io::Result<[u8; 32]> {
        let mut cached = self.key.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(key) = *cached {
            return Ok(key);
        }
        let key = match &self.source {
            KeySource::File(path) => file_key(path)?,
            KeySource::Keychain => keychain_key()?,
        };
        *cached = Some(key);
        Ok(key)
    }
}

fn parse_key(hex: &str) -> io::Result<[u8; 32]> {
    unhex(hex.trim())
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the launch key is damaged"))
}

/// The key of a 0600 file (hex text), made when it is missing.
fn file_key(path: &Path) -> io::Result<[u8; 32]> {
    if let Some(dir) = path.parent() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    let made = random_hex(32)?;
    let created = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path);
    match created {
        Ok(mut file) => {
            file.write_all(made.as_bytes())?;
            file.sync_all()?;
            parse_key(&made)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => parse_key(&fs::read_to_string(path)?),
        Err(e) => Err(e),
    }
}

/// The key of the login keychain, made when it is missing. The key reaches `security` as an
/// argument for a moment: the same exposure as any `security add-generic-password -w`.
fn keychain_key() -> io::Result<[u8; 32]> {
    let account = std::env::var("USER").unwrap_or_else(|_| "sushiai".into());
    let found = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            &account,
            "-w",
        ])
        .output()?;
    if found.status.success() {
        return parse_key(&String::from_utf8_lossy(&found.stdout));
    }
    let made = random_hex(32)?;
    let added = Command::new("/usr/bin/security")
        .args([
            "add-generic-password",
            "-s",
            KEYCHAIN_SERVICE,
            "-a",
            &account,
            "-w",
            &made,
            "-U",
        ])
        .output()?;
    if !added.status.success() {
        return Err(io::Error::other("the keychain refused the launch key"));
    }
    parse_key(&made)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn store(dir: &tempfile::TempDir) -> LaunchStore {
        let home = Home::new(dir.path().to_path_buf());
        fs::create_dir_all(home.sessions()).expect("sessions");
        LaunchStore {
            source: KeySource::File(dir.path().join("keys").join("k")),
            home,
            key: Mutex::new(None),
        }
    }

    fn launch() -> Launch {
        Launch {
            env: BTreeMap::from([("API_TOKEN".into(), "secret-value-123".into())]),
            claude_settings: None,
            model: Some("m1".into()),
            extra_args: vec!["--flag".into()],
        }
    }

    #[test]
    fn a_launch_round_trips_and_is_not_plaintext() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store(&dir);
        store.seal("s1", &launch()).expect("seal");
        let path = dir.path().join("sessions/s1.launch");
        let raw = fs::read(&path).expect("read");
        assert!(!raw.windows(16).any(|w| w == b"secret-value-123"));
        assert_eq!(fs::metadata(&path).expect("meta").mode() & 0o777, 0o600);
        let back = store.open("s1").expect("open");
        assert_eq!(back.env["API_TOKEN"], "secret-value-123");
        assert_eq!(back.extra_args, ["--flag"]);
    }

    #[test]
    fn a_launch_moved_to_another_session_or_cut_does_not_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store(&dir);
        store.seal("s1", &launch()).expect("seal");
        let sessions = dir.path().join("sessions");
        fs::copy(sessions.join("s1.launch"), sessions.join("s2.launch")).expect("copy");
        assert!(store.open("s2").is_err(), "the session id is bound in");
        let mut raw = fs::read(sessions.join("s1.launch")).expect("read");
        raw.truncate(raw.len() - 1);
        fs::write(sessions.join("s1.launch"), raw).expect("write");
        assert!(store.open("s1").is_err());
        assert!(store.open("missing").is_err());
    }

    #[test]
    fn the_key_file_is_made_once_with_mode_0600() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = store(&dir);
        a.seal("s1", &launch()).expect("seal");
        let key_path = dir.path().join("keys").join("k");
        assert_eq!(fs::metadata(&key_path).expect("meta").mode() & 0o777, 0o600);
        // A second store (a restarted daemon) reads the same key.
        let b = store(&dir);
        assert!(b.open("s1").is_ok());
        b.delete("s1");
        assert!(!b.exists("s1"));
    }
}
