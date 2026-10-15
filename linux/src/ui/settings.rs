//! Settings: account, this device, devices, voicemail greetings, appearance, desktop behaviour,
//! about and sign out.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use ring_proto::{
    Device, GreetingsPatch, Me, Settings, SettingsPatch, Voice, VoicemailPatchSettings,
    VoicemailReason, limits,
};

use crate::app::App;
use crate::config::Appearance;
use crate::ui::widgets;
use crate::{APP_ID, VERSION};

/// Greeting rows; an empty row means Ring's built-in greeting for that case.
const GREETINGS: [(VoicemailReason, &str); 3] = [
    (
        VoicemailReason::Busy,
        "Greeting when you're on another call",
    ),
    (VoicemailReason::Declined, "Greeting when you decline"),
    (VoicemailReason::Missed, "Greeting when you miss a call"),
];

#[derive(Clone)]
pub struct SettingsPage {
    root: gtk::Box,
    account_row: adw::ActionRow,
    account_avatar: adw::Bin,
    device_name: adw::EntryRow,
    ring_here: adw::SwitchRow,
    devices: adw::PreferencesGroup,
    device_rows: Rc<RefCell<HashMap<String, adw::ActionRow>>>,
    voicemail_enabled: adw::SwitchRow,
    voice: adw::ComboRow,
    voices: Rc<RefCell<Vec<Voice>>>,
    greetings: Rc<Vec<(VoicemailReason, adw::EntryRow, gtk::Button)>>,
    open_at_login: adw::SwitchRow,
    keep_running: adw::SwitchRow,
    /// Set while values are written from the server, so change handlers don't echo them back.
    updating: Rc<Cell<bool>>,
    settings: Rc<RefCell<Option<Settings>>>,
}

/// Server limits count code points; GtkText's max length does too.
fn limit_length(row: &adw::EntryRow, limit: usize) {
    if let Some(text) = row.delegate().and_downcast::<gtk::Text>() {
        text.set_max_length(limit as i32);
    }
}

fn greeting_patch(reason: VoicemailReason, value: Option<String>) -> SettingsPatch {
    let mut greetings = GreetingsPatch::default();
    match reason {
        VoicemailReason::Busy => greetings.busy = Some(value),
        VoicemailReason::Declined => greetings.declined = Some(value),
        VoicemailReason::Missed => greetings.missed = Some(value),
    }
    SettingsPatch {
        voicemail: Some(VoicemailPatchSettings {
            greetings: Some(greetings),
            ..Default::default()
        }),
        ..Default::default()
    }
}

impl SettingsPage {
    pub fn new() -> SettingsPage {
        let page = adw::PreferencesPage::new();
        let updating = Rc::new(Cell::new(false));

        // The serif page title scrolls with the groups and lines up with them.
        let title = adw::PreferencesGroup::new();
        let heading = widgets::display("Settings", "large");
        heading.set_xalign(0.0);
        heading.set_justify(gtk::Justification::Left);
        title.add(&heading);
        page.add(&title);

        // Account
        let account = adw::PreferencesGroup::builder().title("Account").build();
        let account_avatar = adw::Bin::new();
        let account_row = adw::ActionRow::builder().title("Signed in").build();
        account_row.add_prefix(&account_avatar);
        account.add(&account_row);
        let edit = adw::ActionRow::builder()
            .title("Edit profile on Silicon Accounts")
            .subtitle("Your name and photo come from Silicon Accounts")
            .activatable(true)
            .build();
        edit.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
        edit.connect_activated(|_| {
            let url = App::get()
                .meta()
                .map(|m| m.accounts_url)
                .unwrap_or_else(|| "https://accounts.teamofsilicons.com".into());
            gtk::UriLauncher::new(&url).launch(
                Some(&App::get().window().window),
                None::<&gtk::gio::Cancellable>,
                |_| {},
            );
        });
        account.add(&edit);
        page.add(&account);

        // This device
        let this_device = adw::PreferencesGroup::builder()
            .title("This device")
            .build();
        let device_name = adw::EntryRow::builder()
            .title("Device name")
            .show_apply_button(true)
            .build();
        limit_length(&device_name, limits::DEVICE_NAME);
        device_name.connect_apply(|row| {
            let name = row.text().trim().to_string();
            if !name.is_empty() {
                App::get().rename_device(name);
            }
        });
        this_device.add(&device_name);
        let ring_here = adw::SwitchRow::builder()
            .title("Ring on this device")
            .subtitle("Incoming calls ring here and show a notification")
            .build();
        let guard = updating.clone();
        ring_here.connect_active_notify(move |row| {
            if !guard.get() {
                App::get().set_ring_enabled(row.is_active());
            }
        });
        this_device.add(&ring_here);
        page.add(&this_device);

        // Devices
        let devices = adw::PreferencesGroup::builder()
            .title("Devices")
            .description("Every device signed in to Ring. Calls ring on all of them.")
            .build();
        page.add(&devices);

        // Voicemail
        let voicemail = adw::PreferencesGroup::builder().title("Voicemail").build();
        let voicemail_enabled = adw::SwitchRow::builder()
            .title("Voicemail")
            .subtitle("Callers can leave a message when you can't answer")
            .build();
        let guard = updating.clone();
        voicemail_enabled.connect_active_notify(move |row| {
            if !guard.get() {
                let patch = SettingsPatch {
                    voicemail: Some(VoicemailPatchSettings {
                        enabled: Some(row.is_active()),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                App::get().update_settings(patch, "");
            }
        });
        voicemail.add(&voicemail_enabled);
        let voice = adw::ComboRow::builder()
            .title("Greeting voice")
            .subtitle("Reads your greetings to callers")
            .build();
        voicemail.add(&voice);
        let mut greetings = Vec::new();
        for (reason, title) in GREETINGS {
            let row = adw::EntryRow::builder()
                .title(title)
                .show_apply_button(true)
                .build();
            limit_length(&row, limits::GREETING);
            let reset = gtk::Button::from_icon_name("edit-undo-symbolic");
            reset.add_css_class("flat");
            reset.set_valign(gtk::Align::Center);
            reset.set_tooltip_text(Some("Use the default greeting"));
            row.add_suffix(&reset);
            row.connect_apply(move |row| {
                let text = row.text().trim().to_string();
                let value = (!text.is_empty()).then_some(text);
                App::get().update_settings(greeting_patch(reason, value), "Greeting saved");
            });
            reset.connect_clicked(move |_| {
                App::get().update_settings(greeting_patch(reason, None), "Greeting reset")
            });
            voicemail.add(&row);
            greetings.push((reason, row, reset));
        }
        page.add(&voicemail);

        // Appearance
        let appearance_group = adw::PreferencesGroup::builder().title("Appearance").build();
        let appearance = adw::ComboRow::builder()
            .title("Style")
            .model(&gtk::StringList::new(&["System", "Light", "Dark"]))
            .build();
        appearance.set_selected(
            match App::try_get()
                .map(|a| a.config().appearance)
                .unwrap_or_default()
            {
                Appearance::System => 0,
                Appearance::Light => 1,
                Appearance::Dark => 2,
            },
        );
        appearance.connect_selected_notify(|row| {
            App::get().set_appearance(match row.selected() {
                1 => Appearance::Light,
                2 => Appearance::Dark,
                _ => Appearance::System,
            });
        });
        appearance_group.add(&appearance);
        page.add(&appearance_group);

        // Desktop
        let desktop = adw::PreferencesGroup::builder().title("Desktop").build();
        let open_at_login = adw::SwitchRow::builder()
            .title("Open at login")
            .subtitle("Start Ring in the background when you sign in to this computer")
            .build();
        let guard = updating.clone();
        open_at_login.connect_active_notify(move |row| {
            if !guard.get() {
                App::get().set_open_at_login(row.is_active());
            }
        });
        desktop.add(&open_at_login);
        let keep_running = adw::SwitchRow::builder()
            .title("Keep running in the background")
            .subtitle("Keep listening for calls after the window is closed")
            .build();
        let guard = updating.clone();
        keep_running.connect_active_notify(move |row| {
            if !guard.get() {
                App::get().set_keep_running(row.is_active());
            }
        });
        desktop.add(&keep_running);
        page.add(&desktop);

        // About
        let about = adw::PreferencesGroup::builder().title("About").build();
        let version = adw::ActionRow::builder().title("Version").build();
        let version_label = gtk::Label::new(Some(VERSION));
        version_label.add_css_class("dim-label");
        version.add_suffix(&version_label);
        about.add(&version);
        let source = adw::ActionRow::builder()
            .title("Source")
            .subtitle("github.com/teamofsilicons/silicon-ring")
            .activatable(true)
            .build();
        source.add_suffix(&gtk::Image::from_icon_name("adw-external-link-symbolic"));
        source.connect_activated(|_| {
            gtk::UriLauncher::new("https://github.com/teamofsilicons/silicon-ring").launch(
                Some(&App::get().window().window),
                None::<&gtk::gio::Cancellable>,
                |_| {},
            );
        });
        about.add(&source);
        let privacy = adw::ActionRow::builder()
            .title("Privacy")
            .subtitle("Ring sends no telemetry")
            .build();
        privacy.add_prefix(&gtk::Image::from_icon_name(&format!("{APP_ID}-symbolic")));
        about.add(&privacy);
        page.add(&about);

        // Sign out
        let sign_out_group = adw::PreferencesGroup::new();
        let sign_out = gtk::Button::with_label("Sign out");
        sign_out.add_css_class("destructive-action");
        sign_out.add_css_class("pill");
        sign_out.set_halign(gtk::Align::Center);
        sign_out.connect_clicked(|button| {
            let confirm = adw::AlertDialog::new(
                Some("Sign out of Ring?"),
                Some("This computer will stop ringing for your calls."),
            );
            confirm.add_responses(&[("cancel", "Cancel"), ("sign-out", "Sign out")]);
            confirm.set_response_appearance("sign-out", adw::ResponseAppearance::Destructive);
            confirm.set_close_response("cancel");
            confirm.connect_response(None, |_, response| {
                if response == "sign-out" {
                    App::get().sign_out();
                }
            });
            confirm.present(Some(button));
        });
        sign_out_group.add(&sign_out);
        page.add(&sign_out_group);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.set_vexpand(true);
        root.append(&page);

        let this = SettingsPage {
            root,
            account_row,
            account_avatar,
            device_name,
            ring_here,
            devices,
            device_rows: Rc::default(),
            voicemail_enabled,
            voice: voice.clone(),
            voices: Rc::default(),
            greetings: Rc::new(greetings),
            open_at_login,
            keep_running,
            updating,
            settings: Rc::default(),
        };
        let page_ref = this.clone();
        voice.connect_selected_notify(move |row| {
            if page_ref.updating.get() {
                return;
            }
            let id = page_ref
                .voices
                .borrow()
                .get(row.selected() as usize)
                .map(|v| v.id.clone());
            if let Some(id) = id {
                App::get().update_settings(
                    SettingsPatch {
                        voice: Some(id),
                        ..Default::default()
                    },
                    "Greeting voice changed",
                );
            }
        });
        this.refresh_local();
        this
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    fn quietly(&self, f: impl FnOnce()) {
        self.updating.set(true);
        f();
        self.updating.set(false);
    }

    /// Values that live on this computer rather than on the server.
    fn refresh_local(&self) {
        let Some(app) = App::try_get() else { return };
        let config = app.config();
        self.quietly(|| {
            self.keep_running.set_active(config.keep_running);
            self.open_at_login.set_active(app.open_at_login());
        });
    }

    pub fn set_open_at_login(&self, enabled: bool) {
        self.quietly(|| self.open_at_login.set_active(enabled));
    }

    pub fn set_me(&self, me: &Me) {
        self.account_row
            .set_title(&glib::markup_escape_text(me.account.label()));
        self.account_row.set_subtitle(&me.account.id);
        self.account_avatar
            .set_child(Some(&widgets::avatar(&me.account, 48)));
        self.set_this_device(&me.device);
        self.refresh_local();
    }

    pub fn set_this_device(&self, device: &Device) {
        self.quietly(|| {
            if !self.device_name.has_focus() {
                self.device_name
                    .set_text(device.name.as_deref().unwrap_or_default());
            }
            self.ring_here.set_active(device.ring_enabled);
        });
    }

    pub fn set_devices(&self, devices: &[Device]) {
        for (_, row) in self.device_rows.borrow_mut().drain() {
            self.devices.remove(&row);
        }
        for device in devices {
            self.upsert_device(device);
        }
    }

    pub fn upsert_device(&self, device: &Device) {
        let row = device_row(device);
        let mut rows = self.device_rows.borrow_mut();
        if let Some(old) = rows.remove(&device.device_id) {
            self.devices.remove(&old);
        }
        self.devices.add(&row);
        rows.insert(device.device_id.clone(), row);
    }

    pub fn remove_device(&self, device_id: &str) {
        if let Some(row) = self.device_rows.borrow_mut().remove(device_id) {
            self.devices.remove(&row);
        }
    }

    pub fn set_voices(&self, voices: &[Voice]) {
        self.voices.replace(voices.to_vec());
        let names: Vec<String> = voices
            .iter()
            .map(|v| format!("{} — {}", v.name, v.description))
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        self.quietly(|| self.voice.set_model(Some(&gtk::StringList::new(&names))));
        // Release the borrow first: set_settings stores the settings again.
        let settings = self.settings.borrow().clone();
        if let Some(settings) = settings {
            self.set_settings(&settings);
        }
    }

    pub fn set_settings(&self, settings: &Settings) {
        self.settings.replace(Some(settings.clone()));
        self.quietly(|| {
            self.voicemail_enabled
                .set_active(settings.voicemail.enabled);
            if let Some(index) = self
                .voices
                .borrow()
                .iter()
                .position(|v| v.id == settings.voice)
            {
                self.voice.set_selected(index as u32);
            }
            for (reason, row, reset) in self.greetings.iter() {
                let custom = settings.voicemail.greetings.get(*reason);
                if !row.has_focus() {
                    row.set_text(custom.unwrap_or_default());
                }
                reset.set_visible(custom.is_some());
                row.set_tooltip_text(Some(if custom.is_some() {
                    "Your greeting"
                } else {
                    "Using Ring's default greeting"
                }));
            }
        });
    }

    pub fn reset(&self) {
        self.account_row.set_title("Signed in");
        self.account_row.set_subtitle("");
        self.account_avatar.set_child(None::<&gtk::Widget>);
        self.set_devices(&[]);
        self.settings.replace(None);
    }
}

fn device_row(device: &Device) -> adw::ActionRow {
    let name = device
        .name
        .clone()
        .unwrap_or_else(|| widgets::platform_name(device.platform).to_string());
    let mut details = vec![widgets::platform_name(device.platform).to_string()];
    if device.current {
        details.push("This device".into());
    } else if device.online {
        details.push("Online".into());
    } else if let Some(seen) = device.last_seen_at.as_ref() {
        details.push(format!("Last seen {}", widgets::format_when(seen)));
    }
    if !device.ring_enabled {
        details.push("Ringing off".into());
    }
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&name))
        .subtitle(details.join(" · "))
        .build();
    row.add_prefix(&gtk::Image::from_icon_name(widgets::platform_icon(
        device.platform,
    )));
    if !device.current {
        let revoke = gtk::Button::from_icon_name("user-trash-symbolic");
        revoke.add_css_class("flat");
        revoke.set_valign(gtk::Align::Center);
        revoke.set_tooltip_text(Some("Sign out this device"));
        let id = device.device_id.clone();
        let label = name.clone();
        revoke.connect_clicked(move |button| {
            let confirm = adw::AlertDialog::new(
                Some(&format!("Sign out {label}?")),
                Some("It stops ringing for your calls until you sign in there again."),
            );
            confirm.add_responses(&[("cancel", "Cancel"), ("revoke", "Sign out")]);
            confirm.set_response_appearance("revoke", adw::ResponseAppearance::Destructive);
            confirm.set_close_response("cancel");
            let id = id.clone();
            confirm.connect_response(None, move |_, response| {
                if response == "revoke" {
                    App::get().revoke_device(id.clone());
                }
            });
            confirm.present(Some(button));
        });
        row.add_suffix(&revoke);
    }
    row
}
