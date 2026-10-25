//! "Open at login" and permission to keep running in the background.
//!
//! Outside Flatpak, autostart is an XDG autostart entry in `~/.config/autostart`. Inside Flatpak
//! the sandbox cannot write there, so the Background portal records it instead.

use std::fs;
use std::path::PathBuf;

use crate::{APP_ID, runtime};

const BACKGROUND_REASON: &str = "Ring listens for incoming calls while its window is closed.";

fn entry_path() -> PathBuf {
    glib::user_config_dir()
        .join("autostart")
        .join(format!("{APP_ID}.desktop"))
}

pub fn sandboxed() -> bool {
    ashpd::is_sandboxed()
}

/// The command that relaunches this build: the AppImage itself, the AppRun of an extracted AppImage
/// (as Silicon Apps installs it, which sets up the bundled GTK), the installed binary on PATH, or
/// the absolute path of a development build.
fn launch_command() -> String {
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return quote(&appimage.to_string_lossy());
    }
    // Checked after APPIMAGE: a mounted AppImage's APPDIR is a temporary mount point.
    if let Some(dir) = std::env::var_os("APPDIR") {
        let run = std::path::Path::new(&dir).join("AppRun");
        if run.is_file() {
            return quote(&run.to_string_lossy());
        }
    }
    match std::env::current_exe() {
        Ok(exe) if exe.starts_with("/usr/") => "ring-desktop".to_string(),
        Ok(exe) => quote(&exe.to_string_lossy()),
        Err(_) => "ring-desktop".to_string(),
    }
}

/// Desktop-entry Exec quoting.
fn quote(arg: &str) -> String {
    if arg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
    {
        arg.to_string()
    } else {
        let escaped = arg
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$");
        format!("\"{escaped}\"")
    }
}

/// Whether the XDG autostart entry is present and enabled (non-Flatpak only).
pub fn entry_enabled() -> bool {
    match fs::read_to_string(entry_path()) {
        Ok(text) => !text.lines().any(|line| {
            let line = line.trim();
            line.eq_ignore_ascii_case("Hidden=true")
                || line.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
        }),
        Err(_) => false,
    }
}

fn write_entry(enabled: bool) -> Result<(), String> {
    let path = entry_path();
    if !enabled {
        return match fs::remove_file(&path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                Err(format!("Could not remove {}: {err}", path.display()))
            }
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|err| format!("Could not create {}: {err}", dir.display()))?;
    }
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Ring\n\
         Comment=Listens for Ring calls in the background\n\
         Exec={} --background\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n\
         X-GNOME-UsesNotifications=true\n",
        launch_command()
    );
    fs::write(&path, entry).map_err(|err| format!("Could not write {}: {err}", path.display()))
}

/// Asks the Background portal (Flatpak) to allow running in the background and, optionally,
/// to start at login. Returns whether autostart was granted.
async fn request_background(autostart: bool) -> Result<bool, String> {
    let request = ashpd::desktop::background::Background::request()
        .reason(BACKGROUND_REASON)
        .auto_start(autostart)
        .command(["ring-desktop", "--background"])
        .dbus_activatable(false);
    let response = request
        .send()
        .await
        .and_then(|r| r.response())
        .map_err(|err| err.to_string())?;
    Ok(response.auto_start())
}

/// Turns "Open at login" on or off. Resolves on the GLib side with the resulting state.
pub async fn set_open_at_login(enabled: bool) -> Result<bool, String> {
    if sandboxed() {
        runtime::run(request_background(enabled)).await
    } else {
        write_entry(enabled).map(|()| enabled)
    }
}

/// Inside Flatpak, asks once per launch to keep running when the window is closed, restating
/// the autostart choice (the portal resets it otherwise). Outside a sandbox nothing is needed:
/// the process simply keeps its GApplication hold.
pub fn request_keep_running(open_at_login: bool) {
    if !sandboxed() {
        return;
    }
    runtime::spawn(async move {
        if let Err(err) = request_background(open_at_login).await {
            tracing::info!("background permission not granted: {err}");
        }
    });
}
