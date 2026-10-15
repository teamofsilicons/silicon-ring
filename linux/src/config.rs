//! Local, non-secret preferences and the locations of bundled data (fonts, sounds, icons).
//!
//! The session token is not stored here; see `secret.rs`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::APP_ID;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub appearance: Appearance,
    /// Keep listening for calls after the window is closed.
    #[serde(default = "default_true")]
    pub keep_running: bool,
    /// Only meaningful under Flatpak, where the Background portal owns autostart and cannot be
    /// queried; outside Flatpak the autostart file is the source of truth.
    #[serde(default)]
    pub open_at_login: bool,
    /// Preferred audio devices, by platform id. `None` follows the system default.
    #[serde(default)]
    pub input_device: Option<String>,
    #[serde(default)]
    pub output_device: Option<String>,
    /// Set once the user has been told that closing the window keeps Ring running.
    #[serde(default)]
    pub told_about_background: bool,
    /// Set once "Open at login" has been turned on by the first sign-in, so a later choice in
    /// Settings is never overridden.
    #[serde(default)]
    pub open_at_login_initialized: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config {
            appearance: Appearance::System,
            keep_running: true,
            open_at_login: false,
            input_device: None,
            output_device: None,
            told_about_background: false,
            open_at_login_initialized: false,
        }
    }
}

/// `~/.config/com.teamofsilicons.Ring`, created with mode 0700.
pub fn config_dir() -> PathBuf {
    glib::user_config_dir().join(APP_ID)
}

pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// Writes `contents` atomically with mode 0600.
pub fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        ensure_private_dir(dir)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)
}

impl Config {
    fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    pub fn load() -> Config {
        match fs::read(Self::path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                tracing::warn!("ignoring unreadable settings file: {err}");
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) {
        let json = serde_json::to_vec_pretty(self).expect("settings serialize");
        if let Err(err) = write_private(&Self::path(), &json) {
            tracing::warn!("could not save settings: {err}");
        }
    }
}

/// Finds a bundled data directory (`fonts`, `sounds` or `icons`).
///
/// Installed builds keep them in `<prefix>/share/com.teamofsilicons.Ring/`, found relative to the
/// executable so the same lookup works for /usr, /usr/local and an AppImage. A development build
/// falls back to the repository (`assets/` and `linux/data/`). `RING_DATA_DIR` overrides both.
pub fn data_dir(kind: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os("RING_DATA_DIR") {
        candidates.push(PathBuf::from(dir).join(kind));
    }
    if let Some(prefix) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent()?.parent().map(Path::to_path_buf))
    {
        candidates.push(prefix.join("share").join(APP_ID).join(kind));
    }
    for dir in glib::system_data_dirs() {
        candidates.push(dir.join(APP_ID).join(kind));
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    candidates.push(match kind {
        "icons" => manifest.join("data").join("icons"),
        other => manifest.join("..").join("assets").join(other),
    });
    candidates.into_iter().find(|dir| dir.is_dir())
}
