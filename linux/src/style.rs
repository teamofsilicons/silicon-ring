//! Ring's design tokens (udd/design.md) applied on top of libadwaita.
//!
//! Only token values change: the paper/ink backgrounds, the blue accent, beige selection and the
//! status colours are fed into libadwaita's own colour slots, so every widget keeps its native
//! Adwaita behaviour. libadwaita 1.6 moved those slots from named colours to CSS variables, so the
//! override is generated for whichever version is running, and regenerated when the
//! light/dark appearance flips.

use gtk::gdk;

use crate::config::Appearance;
use crate::fonts;

struct Tokens {
    background: &'static str,
    surface: &'static str,
    surface_raised: &'static str,
    surface_muted: &'static str,
    beige: &'static str,
    foreground: &'static str,
    ink: &'static str,
    text_secondary: &'static str,
    text_muted: &'static str,
    border: &'static str,
    border_strong: &'static str,
    accent: &'static str,
    accent_hover: &'static str,
    on_primary: &'static str,
    accent_text: &'static str,
    accent_subtle: &'static str,
    accent_border: &'static str,
    success: &'static str,
    danger: &'static str,
    warning: &'static str,
}

const LIGHT: Tokens = Tokens {
    background: "#FFFDF9",
    surface: "#FFFFFF",
    surface_raised: "#FFFFFF",
    surface_muted: "#F6F2EA",
    beige: "#EDE8E0",
    foreground: "#1E1E1C",
    ink: "#181818",
    text_secondary: "#4A4842",
    text_muted: "#6E6B63",
    border: "#E7E3D8",
    border_strong: "#D9D2C4",
    accent: "#1F5CB1",
    accent_hover: "#184A94",
    on_primary: "#FFFFFF",
    accent_text: "#1F5CB1",
    accent_subtle: "rgba(31,92,177,0.08)",
    accent_border: "rgba(31,92,177,0.25)",
    success: "#1F7A4D",
    danger: "#C8322B",
    warning: "#B66400",
};

const DARK: Tokens = Tokens {
    background: "#181818",
    surface: "#1E1E1C",
    surface_raised: "#252421",
    surface_muted: "#22211E",
    beige: "#2E2C28",
    foreground: "#EDE8E0",
    ink: "#F6F2EA",
    text_secondary: "#C9C3B6",
    text_muted: "#9C978C",
    border: "#2F2D29",
    border_strong: "#3D3A35",
    accent: "#3872CA",
    accent_hover: "#4A80D2",
    on_primary: "#FFFFFF",
    accent_text: "#8DB2EC",
    accent_subtle: "rgba(91,143,217,0.16)",
    accent_border: "rgba(91,143,217,0.35)",
    success: "#4CC38A",
    danger: "#F2706A",
    warning: "#F0B54A",
};

fn uses_css_variables() -> bool {
    (adw::major_version(), adw::minor_version()) >= (1, 6)
}

/// Ring's own named colours, used by the stylesheet below.
fn ring_colors(t: &Tokens) -> String {
    let pairs = [
        ("ring_background", t.background),
        ("ring_surface", t.surface),
        ("ring_surface_raised", t.surface_raised),
        ("ring_surface_muted", t.surface_muted),
        ("ring_beige", t.beige),
        ("ring_foreground", t.foreground),
        ("ring_ink", t.ink),
        ("ring_text_secondary", t.text_secondary),
        ("ring_text_muted", t.text_muted),
        ("ring_border", t.border),
        ("ring_border_strong", t.border_strong),
        ("ring_accent", t.accent),
        ("ring_accent_hover", t.accent_hover),
        ("ring_on_primary", t.on_primary),
        ("ring_accent_text", t.accent_text),
        ("ring_accent_subtle", t.accent_subtle),
        ("ring_accent_border", t.accent_border),
        ("ring_success", t.success),
        ("ring_danger", t.danger),
        ("ring_warning", t.warning),
    ];
    pairs
        .iter()
        .map(|(name, value)| format!("@define-color {name} {value};\n"))
        .collect()
}

/// libadwaita's colour slots mapped to Ring tokens.
fn adwaita_slots(t: &Tokens, dark: bool) -> Vec<(&'static str, String)> {
    let warning_fg = if dark { "rgba(0,0,0,0.8)" } else { "#FFFFFF" };
    let pairs: [(&'static str, &str); 30] = [
        ("accent_bg_color", t.accent),
        ("accent_fg_color", t.on_primary),
        ("accent_color", t.accent_text),
        ("destructive_bg_color", t.danger),
        ("destructive_fg_color", "#FFFFFF"),
        ("destructive_color", t.danger),
        ("success_bg_color", t.success),
        ("success_fg_color", "#FFFFFF"),
        ("success_color", t.success),
        ("warning_bg_color", t.warning),
        ("warning_fg_color", warning_fg),
        ("warning_color", t.warning),
        ("error_bg_color", t.danger),
        ("error_fg_color", "#FFFFFF"),
        ("error_color", t.danger),
        ("window_bg_color", t.background),
        ("window_fg_color", t.foreground),
        ("view_bg_color", t.surface),
        ("view_fg_color", t.foreground),
        ("headerbar_bg_color", t.background),
        ("headerbar_fg_color", t.foreground),
        ("headerbar_backdrop_color", t.background),
        ("sidebar_bg_color", t.surface_muted),
        ("sidebar_fg_color", t.foreground),
        ("sidebar_backdrop_color", t.surface_muted),
        ("card_bg_color", t.surface),
        ("card_fg_color", t.foreground),
        ("dialog_bg_color", t.surface_raised),
        ("popover_bg_color", t.surface_raised),
        ("thumbnail_bg_color", t.surface),
    ];
    let mut slots: Vec<(&'static str, String)> =
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect();
    slots.push(("dialog_fg_color", t.foreground.to_string()));
    slots.push(("popover_fg_color", t.foreground.to_string()));
    slots
}

fn adwaita_overrides(t: &Tokens, dark: bool) -> String {
    let slots = adwaita_slots(t, dark);
    if uses_css_variables() {
        let body: String = slots
            .iter()
            .map(|(name, value)| format!("  --{}: {value};\n", name.replace('_', "-")))
            .collect();
        format!(":root {{\n{body}}}\n")
    } else {
        slots
            .iter()
            .map(|(name, value)| format!("@define-color {name} {value};\n"))
            .collect()
    }
}

fn stylesheet() -> String {
    let ui = fonts::UI;
    let display = fonts::DISPLAY;
    let pixel = fonts::PIXEL;
    format!(
        r#"
window, dialog, popover, tooltip {{ font-family: "{ui}", "Adwaita Sans", "Cantarell", sans-serif; }}

.display {{ font-family: "{display}", "Hedvig Letters Serif 24pt", "Hedvig Letters Serif", serif; font-weight: 400; }}
.display.hero {{ font-size: 44px; }}
.display.large {{ font-size: 26px; }}
.display.medium {{ font-size: 22px; }}
.display.small {{ font-size: 18px; }}

.pixel {{ font-family: "{pixel}", "Geist Pixel", monospace; font-size: 12px; letter-spacing: 0.5px; }}
.pixel.tiny {{ font-size: 11px; }}
.pixel.large {{ font-size: 13px; }}
.pixel.timer {{ font-size: 18px; letter-spacing: 1px; }}
.wordmark {{ font-family: "{pixel}", "Geist Pixel", monospace; font-size: 22px; letter-spacing: 3px; }}
.muted {{ color: @ring_text_muted; }}
.secondary {{ color: @ring_text_secondary; }}
.danger-text {{ color: @ring_danger; }}
.success-text {{ color: @ring_success; }}
.accent-text {{ color: @ring_accent_text; }}

/* Badges (SILICON is the site's .wt-badge--silicon) */
.badge {{
  font-family: "{pixel}", "Geist Pixel", monospace; font-size: 11px; letter-spacing: 0.5px;
  min-height: 20px; padding: 0 6px; border-radius: 5px;
  background-color: @ring_beige; color: @ring_text_secondary; border: 1px solid @ring_border_strong;
}}
.badge.silicon {{ background-color: @ring_accent_subtle; color: @ring_accent_text; border-color: @ring_accent_border; }}
.badge.on-ring {{ background-color: alpha(@ring_success, 0.10); color: @ring_success; border-color: alpha(@ring_success, 0.30); }}

/* Avatar fallback: the initial in Hedvig, accent-coloured, on beige */
avatar.ring-avatar {{
  background-image: none; background-color: @ring_beige; color: @ring_accent_text;
  font-family: "{display}", "Hedvig Letters Serif", serif;
  box-shadow: inset 0 0 0 1px @ring_border_strong;
}}

/* Sidebar: a beige pill for the selected section */
.ring-sidebar row {{ border-radius: 8px; transition: background-color 150ms ease; }}
.ring-sidebar row:hover {{ background-color: alpha(@ring_beige, 0.6); }}
.ring-sidebar row:selected {{ background-color: @ring_beige; color: @ring_foreground; }}
.unread-dot {{ min-width: 8px; min-height: 8px; border-radius: 9999px; background-color: @ring_accent; }}
.count-badge {{ font-family: "{pixel}", monospace; font-size: 11px; min-width: 18px; min-height: 18px; padding: 0 4px; border-radius: 9999px; background-color: @ring_accent; color: @ring_on_primary; }}

/* The Ring mark's concentric rings around avatars */
.avatar-ring {{ border-radius: 9999px; padding: 6px; }}
.avatar-ring.ringing {{ animation: ring-expand 3s ease-out infinite; }}
.avatar-ring.speaking {{ animation: ring-pulse 1.2s ease-out infinite; }}
@keyframes ring-expand {{
  0%   {{ box-shadow: 0 0 0 0 alpha(@ring_ink, 0.18), 0 0 0 0 alpha(@ring_ink, 0.10); }}
  100% {{ box-shadow: 0 0 0 28px alpha(@ring_ink, 0), 0 0 0 52px alpha(@ring_ink, 0); }}
}}
@keyframes ring-pulse {{
  0%   {{ box-shadow: 0 0 0 0 alpha(@ring_accent, 0.55); }}
  100% {{ box-shadow: 0 0 0 16px alpha(@ring_accent, 0); }}
}}

/* Round call controls */
.call-control {{ min-width: 56px; min-height: 56px; padding: 0; border-radius: 9999px; }}
.call-control.big {{ min-width: 64px; min-height: 64px; }}
.call-control.accept {{ background-color: @ring_success; color: #FFFFFF; }}
.call-control.accept:hover {{ background-color: shade(@ring_success, 1.08); }}
.call-control:checked {{ background-color: @ring_ink; color: @ring_background; }}
.control-label {{ font-size: 12px; color: @ring_text_muted; }}

/* Transcript: lines move up and fade toward the top */
.transcript-line .speaker {{ font-family: "{pixel}", monospace; font-size: 11px; letter-spacing: 0.5px; color: @ring_text_muted; }}
.transcript-line .text {{ font-size: 17px; }}
.transcript-line.partial .text {{ color: @ring_text_muted; }}
.transcript-line.event {{ font-family: "{pixel}", monospace; font-size: 11px; color: @ring_text_muted; }}
.transcript-fade {{ background-image: linear-gradient(to bottom, @window_bg_color 0%, alpha(@window_bg_color, 0) 100%); }}

/* Call details timeline: a hairline rail with dots */
.timeline-rail {{ min-width: 1px; background-color: @ring_border_strong; }}
.timeline-dot {{ min-width: 7px; min-height: 7px; border-radius: 9999px; background-color: @ring_ink; }}
.timeline-dot.danger {{ background-color: @ring_danger; }}

/* Decline reason chips */
.chip {{ border-radius: 9999px; padding: 4px 12px; }}

/* The incoming call is a "moment" screen in the Ring mark's colours, in both themes */
.moment {{ background-color: #181818; color: #EDE8E0; }}
.moment .muted {{ color: #9C978C; }}
.moment avatar.ring-avatar {{ background-color: #2E2C28; color: #8DB2EC; box-shadow: inset 0 0 0 1px #3D3A35; }}
.moment .avatar-ring.ringing {{ animation: moment-expand 3s ease-out infinite; }}
@keyframes moment-expand {{
  0%   {{ box-shadow: 0 0 0 0 rgba(237,232,224,0.20), 0 0 0 0 rgba(237,232,224,0.10); }}
  100% {{ box-shadow: 0 0 0 28px rgba(237,232,224,0), 0 0 0 52px rgba(237,232,224,0); }}
}}
.moment .call-control.decline {{ background-color: #F2706A; color: #181818; }}
.moment .call-control.accept {{ background-color: #4CC38A; color: #181818; }}
.moment .chip {{ background-color: rgba(237,232,224,0.10); color: #EDE8E0; }}
.moment .chip:hover {{ background-color: rgba(237,232,224,0.18); }}
.moment entry {{ background-color: rgba(237,232,224,0.08); color: #EDE8E0; }}
.moment button.flat {{ color: #C9C3B6; }}
.moment .badge.silicon {{ background-color: rgba(91,143,217,0.16); color: #8DB2EC; border-color: rgba(91,143,217,0.35); }}

/* Voicemail recording indicator */
.recording-dot {{ min-width: 10px; min-height: 10px; border-radius: 9999px; background-color: @ring_danger; animation: rec-pulse 1.4s ease-in-out infinite; }}
@keyframes rec-pulse {{ 0% {{ opacity: 1; }} 50% {{ opacity: 0.3; }} 100% {{ opacity: 1; }} }}

.lookup-card {{ padding: 12px; }}
.ring-empty image.icon {{ opacity: 0.3; }}
"#
    )
}

fn css(dark: bool) -> String {
    let tokens = if dark { &DARK } else { &LIGHT };
    format!(
        "{}{}{}",
        ring_colors(tokens),
        adwaita_overrides(tokens, dark),
        stylesheet()
    )
}

/// Installs the stylesheet for the default display and keeps it in step with the appearance.
pub fn install() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
    let style = adw::StyleManager::default();
    provider.load_from_string(&css(style.is_dark()));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    style.connect_dark_notify(move |style| provider.load_from_string(&css(style.is_dark())));

    if let Some(icons) = crate::config::data_dir("icons") {
        gtk::IconTheme::for_display(&display).add_search_path(icons);
    }
}

pub fn apply_appearance(appearance: Appearance) {
    adw::StyleManager::default().set_color_scheme(match appearance {
        Appearance::System => adw::ColorScheme::Default,
        Appearance::Light => adw::ColorScheme::ForceLight,
        Appearance::Dark => adw::ColorScheme::ForceDark,
    });
}
