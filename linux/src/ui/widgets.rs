//! Small building blocks shared by every screen: labels in the three brand faces, identity
//! badges, avatars with remote photos, and time formatting.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use adw::prelude::*;
use gtk::{gdk, glib};
use ring_proto::{
    Account, AccountKind, Call, CallState, DeviceRef, Direction, EndReason, InvitationState,
    Platform, Time,
};

use crate::app::App;
use crate::runtime;

pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build();
    for class in classes {
        label.add_css_class(class);
    }
    label
}

/// An uppercase Geist Pixel label (status, meta lines, section labels).
pub fn pixel(text: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text.to_uppercase())
        .xalign(0.0)
        .build();
    label.add_css_class("pixel");
    label
}

/// An account id (`c:alice`, `si:tos`) in Geist Pixel. Ids keep their case.
pub fn pixel_id(id: &str) -> gtk::Label {
    let label = gtk::Label::builder().label(id).xalign(0.0).build();
    label.add_css_class("pixel");
    label
}

/// A Hedvig Letters Serif display label.
pub fn display(text: &str, size: &str) -> gtk::Label {
    let label = gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .build();
    label.add_css_class("display");
    label.add_css_class(size);
    label
}

/// The SILICON badge for silicons; nothing for carbons, who are the default.
pub fn kind_badge(account: &Account) -> Option<gtk::Label> {
    (account.kind == AccountKind::Silicon).then(|| badge("Silicon", "silicon"))
}

pub fn badge(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(&text.to_uppercase()));
    label.add_css_class("badge");
    if !class.is_empty() {
        label.add_css_class(class);
    }
    label.set_valign(gtk::Align::Center);
    label.set_halign(gtk::Align::Start);
    label
}

thread_local! {
    static TEXTURES: RefCell<HashMap<String, gdk::Texture>> = RefCell::new(HashMap::new());
    static PENDING: RefCell<HashMap<String, Vec<glib::WeakRef<adw::Avatar>>>> = RefCell::new(HashMap::new());
    static FAILED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// A circular avatar: the account's photo when it has one, otherwise its initial.
pub fn avatar(account: &Account, size: i32) -> adw::Avatar {
    let avatar = adw::Avatar::new(size, Some(&initial_source(account)), true);
    avatar.add_css_class("ring-avatar");
    if let Some(url) = account
        .pfp_url
        .as_deref()
        .filter(|u| u.starts_with("https://"))
    {
        load_photo(&avatar, url);
    }
    avatar
}

/// The word AdwAvatar takes its initial from: one letter, as design.md asks, from the first word
/// of the display name, or the handle of an id (`c:alice` → `alice`).
fn initial_source(account: &Account) -> String {
    let name = account
        .display_name
        .as_deref()
        .and_then(|n| n.split_whitespace().next())
        .unwrap_or_else(|| {
            account
                .id
                .split_once(':')
                .map_or(account.id.as_str(), |(_, handle)| handle)
        });
    name.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn load_photo(avatar: &adw::Avatar, url: &str) {
    if let Some(texture) = TEXTURES.with(|t| t.borrow().get(url).cloned()) {
        avatar.set_custom_image(Some(&texture));
        return;
    }
    if FAILED.with(|f| f.borrow().contains(url)) {
        return;
    }
    let first = PENDING.with(|p| {
        let mut pending = p.borrow_mut();
        let waiting = pending.entry(url.to_string()).or_default();
        waiting.push(avatar.downgrade());
        waiting.len() == 1
    });
    if !first {
        return;
    }
    let Some(app) = App::try_get() else { return };
    let client = app.client();
    let url = url.to_string();
    glib::spawn_future_local(async move {
        let fetch_url = url.clone();
        let bytes = runtime::run(async move { client.download(&fetch_url).await }).await;
        let texture = bytes
            .ok()
            .and_then(|bytes| gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok());
        let waiting = PENDING
            .with(|p| p.borrow_mut().remove(&url))
            .unwrap_or_default();
        match texture {
            Some(texture) => {
                for avatar in waiting.iter().filter_map(|w| w.upgrade()) {
                    avatar.set_custom_image(Some(&texture));
                }
                TEXTURES.with(|t| t.borrow_mut().insert(url, texture));
            }
            None => {
                FAILED.with(|f| f.borrow_mut().insert(url));
            }
        }
    });
}

/// An avatar inside a padded circle that carries the "ringing" and "speaking" ring animations.
pub fn ringed_avatar(account: &Account, size: i32) -> (gtk::Box, adw::Avatar) {
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("avatar-ring");
    frame.set_halign(gtk::Align::Center);
    frame.set_valign(gtk::Align::Center);
    let avatar = avatar(account, size);
    frame.append(&avatar);
    (frame, avatar)
}

/// Name (serif, large) above the id (pixel, muted), as every call screen shows identity.
pub fn identity_block(account: &Account, size_class: &str) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.set_halign(gtk::Align::Center);
    let name = display(account.label(), size_class);
    column.append(&name);
    if account
        .display_name
        .as_deref()
        .is_some_and(|n| !n.trim().is_empty())
    {
        let id = pixel_id(&account.id);
        id.add_css_class("muted");
        id.set_halign(gtk::Align::Center);
        column.append(&id);
    }
    if let Some(badge) = kind_badge(account) {
        badge.set_halign(gtk::Align::Center);
        column.append(&badge);
    }
    column
}

pub fn local(t: &Time) -> Option<glib::DateTime> {
    glib::DateTime::from_unix_local(t.millis().div_euclid(1000)).ok()
}

/// "14:05", "Yesterday 14:05", "Mon 14:05", "3 Oct", "3 Oct 2025".
pub fn format_when(t: &Time) -> String {
    let (Some(then), Ok(now)) = (local(t), glib::DateTime::now_local()) else {
        return t.to_rfc3339();
    };
    let fmt = |f: &str| then.format(f).map(|s| s.to_string()).unwrap_or_default();
    let (ty, tm, td) = then.ymd();
    let (ny, nm, nd) = now.ymd();
    if (ty, tm, td) == (ny, nm, nd) {
        return fmt("%H:%M");
    }
    let yesterday = now.add_days(-1).map(|d| d.ymd()).ok();
    if yesterday == Some((ty, tm, td)) {
        return format!("Yesterday {}", fmt("%H:%M"));
    }
    let age_days = now.difference(&then).as_days();
    if (0..6).contains(&age_days) {
        return fmt("%a %H:%M");
    }
    if ty == ny {
        fmt("%-e %b")
    } else {
        fmt("%-e %b %Y")
    }
}

/// Full date and time for details screens.
pub fn format_full(t: &Time) -> String {
    local(t)
        .and_then(|d| d.format("%-e %B %Y, %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| t.to_rfc3339())
}

/// `m:ss`, or `h:mm:ss` past an hour.
pub fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn platform_icon(platform: Option<Platform>) -> &'static str {
    match platform {
        Some(Platform::Ios | Platform::Android) => "phone-symbolic",
        Some(Platform::Ipados) => "tablet-symbolic",
        Some(Platform::Web) => "web-browser-symbolic",
        Some(Platform::Cli) => "utilities-terminal-symbolic",
        _ => "computer-symbolic",
    }
}

pub fn platform_name(platform: Option<Platform>) -> &'static str {
    match platform {
        Some(Platform::Ios) => "iPhone",
        Some(Platform::Ipados) => "iPad",
        Some(Platform::Macos) => "Mac",
        Some(Platform::Android) => "Android",
        Some(Platform::Windows) => "Windows",
        Some(Platform::Linux) => "Linux",
        Some(Platform::Web) => "Web",
        Some(Platform::Cli) => "CLI",
        None => "device",
    }
}

/// How a device is named in "Moved to …" and "On a call … on …": its name, else a word for its
/// platform.
pub fn device_label(device: &DeviceRef) -> String {
    if let Some(name) = device
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        return name.to_string();
    }
    match device.platform {
        Some(Platform::Ios) => "iPhone",
        Some(Platform::Ipados) => "iPad",
        Some(Platform::Macos) => "Mac",
        Some(Platform::Android) => "Android phone",
        Some(Platform::Windows) => "Windows PC",
        Some(Platform::Linux) => "Linux computer",
        Some(Platform::Web) => "the web",
        Some(Platform::Cli) => "the command line",
        None => "another device",
    }
    .to_string()
}

/// The people on a call other than `me`: joined participants first, then pending invitees.
pub fn others(call: &Call, me: &str) -> Vec<Account> {
    let mut people: Vec<Account> = Vec::new();
    let mut push = |a: &Account| {
        if a.uuid != me && !people.iter().any(|p| p.uuid == a.uuid) {
            people.push(a.clone());
        }
    };
    for p in &call.participants {
        push(&p.account);
    }
    for i in &call.invitations {
        push(&i.to);
        push(&i.from);
    }
    push(&call.created_by);
    people
}

/// Whether a call counts as missed for this viewer (an incoming call nobody here answered).
pub fn is_missed(call: &Call, me: &str) -> bool {
    call.direction == Direction::Incoming
        && call.answered_at.is_none()
        && call.state == CallState::Ended
        && call.invitations.iter().any(|i| {
            i.to.uuid == me && matches!(i.state, InvitationState::Missed | InvitationState::Busy)
        })
}

/// One short pixel-font outcome line for history rows: "4:12", "MISSED", "DECLINED", …
pub fn outcome(call: &Call, me: &str) -> String {
    match call.state {
        CallState::Ringing => return "RINGING".into(),
        CallState::Active => return "ON CALL".into(),
        CallState::Voicemail => return "VOICEMAIL".into(),
        CallState::Ended => {}
    }
    if let (Some(start), Some(end)) = (call.answered_at, call.ended_at) {
        return format_duration((end.millis() - start.millis()).max(0) as u64);
    }
    if is_missed(call, me) {
        return "MISSED".into();
    }
    if call.voicemail_id.is_some() {
        return "VOICEMAIL".into();
    }
    match call.end_reason {
        Some(EndReason::Declined) => "DECLINED".into(),
        Some(EndReason::Cancelled) => "CANCELLED".into(),
        Some(EndReason::Busy) => "BUSY".into(),
        Some(EndReason::Missed) => "NO ANSWER".into(),
        Some(EndReason::Failed) => "FAILED".into(),
        Some(EndReason::ServerRestart) => "INTERRUPTED".into(),
        Some(EndReason::Completed) | None => "ENDED".into(),
    }
}

/// A short human description for error toasts, including the server's hint.
pub fn error_text(err: &ring_core::Error) -> String {
    use ring_proto::codes;
    let friendly = match err.code() {
        codes::ACCOUNT_NOT_FOUND => Some("No one on Silicon Accounts has that id."),
        codes::INVITATION_NOT_RINGING => Some("That call is no longer ringing."),
        codes::INVITATION_EXPIRED => Some("That call stopped ringing."),
        codes::SILICON_USE_CLI => Some("Silicons use the ring CLI. Sign in with a carbon account."),
        codes::RATE_LIMITED => Some("Too many requests. Try again in a moment."),
        codes::DEPENDENCY_UNAVAILABLE => {
            Some("Ring is having trouble reaching a service. Try again shortly.")
        }
        "network" => Some("Can't reach Ring. Check your connection."),
        _ => None,
    };
    if let Some(text) = friendly {
        return text.to_string();
    }
    match (err, err.hint()) {
        (ring_core::Error::Api { error, .. }, Some(hint)) => format!("{} {}", error.message, hint),
        (ring_core::Error::Api { error, .. }, None) => error.message.clone(),
        (other, _) => other.to_string(),
    }
}
