//! Private local state in `$SILICON_HOME/.ring` (or `~/.ring`): `session.json` and
//! `login-attempt.json`. Directories are 0700 and files 0600; every write is atomic.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ring_proto::{Account, Time};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{CliError, CliResult};

const SESSION: &str = "session.json";
const LOGIN_ATTEMPT: &str = "login-attempt.json";

/// A Ring session, bound to the API origin that issued it.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredSession {
    pub api_url: String,
    pub session_token: String,
    pub expires_at: Time,
    pub account: Account,
    pub device_id: String,
    pub created_at: Time,
}

impl std::fmt::Debug for StoredSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredSession")
            .field("api_url", &self.api_url)
            .field("account", &self.account.id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

/// Written before a token exchange so that retrying the same token reuses the same
/// `Idempotency-Key`. Holds a hash of the token, never the token.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginAttempt {
    pub api_url: String,
    pub token_sha256: String,
    pub idempotency_key: String,
    pub started_at: Time,
}

pub struct State {
    dir: PathBuf,
}

impl State {
    pub fn locate() -> CliResult<Self> {
        let base = match std::env::var_os("SILICON_HOME") {
            Some(home) if home.is_empty() => {
                return Err(CliError::input("SILICON_HOME is set but empty.")
                    .hint("Point SILICON_HOME at your private home directory, or unset it to use ~/.ring."));
            }
            Some(home) => PathBuf::from(home),
            None => std::env::home_dir().ok_or_else(|| {
                CliError::input("Cannot find a home directory for Ring's state.")
                    .hint("Set SILICON_HOME or HOME.")
            })?,
        };
        if !base.is_absolute() {
            return Err(CliError::input(format!(
                "The state directory {} is not an absolute path.",
                base.display()
            ))
            .hint("Set SILICON_HOME (or HOME) to an absolute path."));
        }
        Ok(State {
            dir: base.join(".ring"),
        })
    }

    pub fn session(&self) -> CliResult<Option<StoredSession>> {
        self.read(SESSION)
    }

    pub fn save_session(&self, session: &StoredSession) -> CliResult<()> {
        self.write(SESSION, session)
    }

    pub fn remove_session(&self) -> CliResult<()> {
        self.remove(SESSION)
    }

    pub fn login_attempt(&self) -> CliResult<Option<LoginAttempt>> {
        self.read(LOGIN_ATTEMPT)
    }

    pub fn save_login_attempt(&self, attempt: &LoginAttempt) -> CliResult<()> {
        self.write(LOGIN_ATTEMPT, attempt)
    }

    pub fn remove_login_attempt(&self) -> CliResult<()> {
        self.remove(LOGIN_ATTEMPT)
    }

    /// Reads a state file. A missing file (or directory) is `None`, and nothing is created, so
    /// read-only commands work on a read-only home.
    fn read<T: DeserializeOwned>(&self, name: &str) -> CliResult<Option<T>> {
        let path = self.dir.join(name);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(CliError::io(
                    &format!("Could not read {}", path.display()),
                    e,
                ));
            }
        };
        tighten_permissions(&path);
        serde_json::from_slice(&bytes).map(Some).map_err(|e| {
            CliError::new("local_state", format!("{} is damaged: {e}", path.display())).hint(
                format!("Delete it and sign in again: {}", crate::error::SIGN_IN),
            )
        })
    }

    fn write<T: Serialize>(&self, name: &str, value: &T) -> CliResult<()> {
        self.ensure_dir()?;
        let bytes = serde_json::to_vec_pretty(value).expect("state always serializes");
        atomic_write(&self.dir.join(name), &bytes)
    }

    fn remove(&self, name: &str) -> CliResult<()> {
        let path = self.dir.join(name);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CliError::io(
                &format!("Could not remove {}", path.display()),
                e,
            )),
        }
    }

    fn ensure_dir(&self) -> CliResult<()> {
        if let Ok(meta) = fs::symlink_metadata(&self.dir) {
            if meta.file_type().is_symlink() {
                return Err(CliError::input(format!(
                    "{} must be a directory, not a symlink.",
                    self.dir.display()
                )));
            }
            if !meta.is_dir() {
                return Err(CliError::input(format!(
                    "{} exists and is not a directory.",
                    self.dir.display()
                )));
            }
            tighten_permissions(&self.dir);
            return Ok(());
        }
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
            .create(&self.dir)
            .map_err(|e| CliError::io(&format!("Could not create {}", self.dir.display()), e))?;
        tighten_permissions(&self.dir);
        Ok(())
    }
}

/// Writes `bytes` to a private temporary file beside `path`, syncs it and renames it into place,
/// so a crash never leaves a half-written state file.
fn atomic_write(path: &Path, bytes: &[u8]) -> CliResult<()> {
    let dir = path.parent().expect("state files live in a directory");
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("state");
    let temp = dir.join(format!(
        ".{file_name}.{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        if let Ok(dir) = fs::File::open(dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    result.map_err(|e: std::io::Error| {
        let _ = fs::remove_file(&temp);
        CliError::io(&format!("Could not write {}", path.display()), e)
    })
}

/// Restores owner-only permissions if something loosened them.
fn tighten_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let wanted = if meta.is_dir() { 0o700 } else { 0o600 };
            if meta.permissions().mode() & 0o777 != wanted {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(wanted));
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Writes an output file the user asked for (audio). Refuses to replace an existing file unless
/// `overwrite` is set.
pub fn write_output(path: &Path, bytes: &[u8], overwrite: bool) -> CliResult<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true);
    if overwrite {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            CliError::new("file_exists", format!("{} already exists.", path.display()))
                .hint("Choose another path, or pass --overwrite to replace it.")
                .exit(5)
        } else {
            CliError::io(&format!("Could not open {}", path.display()), e)
        }
    })?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| CliError::io(&format!("Could not write {}", path.display()), e))
}
