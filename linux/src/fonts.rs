//! App-local brand fonts (BDO Grotesk, Hedvig Letters Serif, Geist Pixel Square).
//!
//! The fonts ship inside the package rather than in the system font directories, so they are
//! added to fontconfig's application font set before GTK creates its Pango font map.

use std::ffi::{CString, c_int, c_uchar, c_void};
use std::os::unix::ffi::OsStrExt;

use gtk::prelude::*;

use crate::config;

#[link(name = "fontconfig")]
unsafe extern "C" {
    fn FcConfigAppFontAddDir(config: *mut c_void, dir: *const c_uchar) -> c_int;
}

/// Font families as fontconfig names them (Hedvig's family name really does repeat "24pt").
pub const UI: &str = "BDO Grotesk";
pub const DISPLAY: &str = "Hedvig Letters Serif 24pt 24pt";
pub const PIXEL: &str = "Geist Pixel Square";

/// Must run before GTK is initialised.
pub fn load() {
    let Some(dir) = config::data_dir("fonts") else {
        tracing::warn!("brand fonts not found; using system fonts");
        return;
    };
    let Ok(path) = CString::new(dir.as_os_str().as_bytes()) else {
        return;
    };
    // A null config means the current (default) fontconfig configuration, which Pango uses.
    let ok = unsafe { FcConfigAppFontAddDir(std::ptr::null_mut(), path.as_ptr().cast()) };
    if ok == 0 {
        tracing::warn!("fontconfig could not add {}", dir.display());
    } else {
        tracing::debug!("loaded brand fonts from {}", dir.display());
    }
}

/// Logs which brand families Pango can actually resolve. Call after GTK is initialised.
pub fn check() {
    let context = gtk::Label::new(None).pango_context();
    let families: Vec<String> = context
        .list_families()
        .iter()
        .map(|f| f.name().to_string())
        .collect();
    for wanted in [UI, DISPLAY, PIXEL] {
        let base = wanted.split(' ').take(2).collect::<Vec<_>>().join(" ");
        if families
            .iter()
            .any(|name| name.eq_ignore_ascii_case(wanted) || name.starts_with(&base))
        {
            tracing::debug!("font family available: {wanted}");
        } else {
            tracing::warn!("font family not available: {wanted}");
        }
    }
}
