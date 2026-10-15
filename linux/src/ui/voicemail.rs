//! Voicemail: the list with inline playback, and the detail dialog.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use ring_proto::{AudioStatus, TranscriptStatus, Voicemail, VoicemailReason};

use crate::app::App;
use crate::runtime;
use crate::ui::player::Player;
use crate::ui::recents::{empty_state, section_frame};
use crate::ui::widgets;

/// "MISSED", "DECLINED · In a meeting", "BUSY".
pub fn reason_text(voicemail: &Voicemail) -> String {
    match (voicemail.reason, voicemail.decline_reason.as_deref()) {
        (VoicemailReason::Declined, Some(reason)) if !reason.is_empty() => {
            format!("DECLINED · {reason}")
        }
        (reason, _) => reason.as_str().to_uppercase(),
    }
}

struct Inline {
    voicemail_id: String,
    button: gtk::Button,
}

#[derive(Clone)]
pub struct VoicemailPage {
    root: gtk::Box,
    stack: gtk::Stack,
    list: gtk::ListBox,
    error: adw::StatusPage,
    items: Rc<RefCell<Vec<Voicemail>>>,
    player: Player,
    inline: Rc<RefCell<Option<Inline>>>,
    /// Bumped by every inline play, so the previous play's button-follower stops.
    inline_generation: Rc<Cell<u64>>,
}

impl VoicemailPage {
    pub fn new() -> VoicemailPage {
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.set_valign(gtk::Align::Start);

        let loading = gtk::Spinner::builder()
            .spinning(true)
            .halign(gtk::Align::Center)
            .margin_top(48)
            .build();
        let empty = empty_state(
            "No voicemail",
            "Messages people leave when you can't answer appear here.",
        );
        let error = adw::StatusPage::builder()
            .icon_name("network-offline-symbolic")
            .title("Couldn't load voicemail")
            .build();
        error.add_css_class("compact");
        let retry = gtk::Button::with_label("Try again");
        retry.add_css_class("pill");
        retry.set_halign(gtk::Align::Center);
        retry.connect_clicked(|_| App::get().refresh_voicemail());
        error.set_child(Some(&retry));

        let stack = gtk::Stack::new();
        stack.add_named(&loading, Some("loading"));
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&error, Some("error"));
        stack.add_named(&list, Some("list"));
        stack.set_visible_child_name("loading");

        let root = section_frame("Voicemail", &stack);
        let page = VoicemailPage {
            root,
            stack,
            list: list.clone(),
            error,
            items: Rc::default(),
            player: Player::new(),
            inline: Rc::default(),
            inline_generation: Rc::default(),
        };
        let this = page.clone();
        list.connect_row_activated(move |_, row| {
            let voicemail = this
                .items
                .borrow()
                .iter()
                .find(|v| v.voicemail_id == row.widget_name().as_str())
                .cloned();
            if let Some(voicemail) = voicemail {
                this.stop_inline();
                open_detail(voicemail);
            }
        });
        page
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    pub fn on_shown(&self) {}

    pub fn set_error(&self, message: &str) {
        if self.items.borrow().is_empty() {
            self.error
                .set_description(Some(&glib::markup_escape_text(message)));
            self.stack.set_visible_child_name("error");
        }
    }

    pub fn set_voicemails(&self, voicemails: Vec<Voicemail>) {
        self.items.replace(voicemails);
        self.rebuild();
    }

    pub fn upsert(&self, voicemail: Voicemail) {
        {
            let mut items = self.items.borrow_mut();
            match items
                .iter_mut()
                .find(|v| v.voicemail_id == voicemail.voicemail_id)
            {
                Some(existing) => *existing = voicemail,
                None => items.insert(0, voicemail),
            }
            items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        }
        self.rebuild();
    }

    pub fn remove(&self, voicemail_id: &str) {
        self.items
            .borrow_mut()
            .retain(|v| v.voicemail_id != voicemail_id);
        self.rebuild();
    }

    pub fn unread_count(&self) -> usize {
        self.items.borrow().iter().filter(|v| !v.read).count()
    }

    fn rebuild(&self) {
        let playing = self
            .inline
            .borrow()
            .as_ref()
            .map(|i| i.voicemail_id.clone());
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let items = self.items.borrow().clone();
        for voicemail in &items {
            let (row, button) = self.row(voicemail);
            if playing.as_deref() == Some(voicemail.voicemail_id.as_str()) {
                if let Some(inline) = self.inline.borrow_mut().as_mut() {
                    inline.button = button.clone();
                }
                set_play_icon(&button, self.player.is_playing());
            }
            self.list.append(&row);
        }
        self.stack
            .set_visible_child_name(if items.is_empty() { "empty" } else { "list" });
    }

    fn row(&self, voicemail: &Voicemail) -> (gtk::ListBoxRow, gtk::Button) {
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.set_margin_top(10);
        line.set_margin_bottom(10);
        line.set_margin_start(8);
        line.set_margin_end(8);

        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.set_valign(gtk::Align::Center);
        dot.set_size_request(8, 8);
        if !voicemail.read {
            dot.add_css_class("unread-dot");
        }
        line.append(&dot);
        line.append(&widgets::avatar(&voicemail.from, 40));

        let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
        text.set_hexpand(true);
        let title_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::builder()
            .label(voicemail.from.label())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("heading");
        title_line.append(&name);
        if let Some(badge) = widgets::kind_badge(&voicemail.from) {
            title_line.append(&badge);
        }
        text.append(&title_line);
        let mut meta = vec![reason_text(voicemail)];
        if let Some(ms) = voicemail.duration_ms {
            meta.push(widgets::format_duration(ms));
        }
        meta.push(widgets::format_when(&voicemail.created_at).to_uppercase());
        let meta = widgets::pixel(&meta.join(" · "));
        meta.add_css_class("tiny");
        meta.add_css_class("muted");
        text.append(&meta);
        let preview = match (voicemail.transcript_status, voicemail.transcript.as_deref()) {
            (_, Some(t)) if !t.trim().is_empty() => t.to_string(),
            (TranscriptStatus::Pending, _) => "Transcribing…".to_string(),
            _ => "No transcript".to_string(),
        };
        let preview = gtk::Label::builder()
            .label(preview)
            .xalign(0.0)
            .wrap(true)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        preview.add_css_class("secondary");
        text.append(&preview);
        line.append(&text);

        let play = gtk::Button::from_icon_name("media-playback-start-symbolic");
        play.add_css_class("flat");
        play.add_css_class("circular");
        play.set_valign(gtk::Align::Center);
        play.set_tooltip_text(Some("Play"));
        play.set_sensitive(voicemail.audio_status == AudioStatus::Ready);
        let this = self.clone();
        let id = voicemail.voicemail_id.clone();
        play.connect_clicked(move |button| this.play_inline(&id, button));
        line.append(&play);

        let row = gtk::ListBoxRow::builder()
            .child(&line)
            .activatable(true)
            .name(voicemail.voicemail_id.as_str())
            .build();
        (row, play)
    }

    fn stop_inline(&self) {
        self.inline_generation
            .set(self.inline_generation.get() + 1);
        if let Some(inline) = self.inline.take() {
            set_play_icon(&inline.button, false);
        }
        self.player.stop();
    }

    fn play_inline(&self, voicemail_id: &str, button: &gtk::Button) {
        let same = self
            .inline
            .borrow()
            .as_ref()
            .is_some_and(|i| i.voicemail_id == voicemail_id);
        if same && self.player.media().is_some() {
            self.player.toggle();
            set_play_icon(button, self.player.is_playing());
            return;
        }
        self.stop_inline();
        self.inline.replace(Some(Inline {
            voicemail_id: voicemail_id.to_string(),
            button: button.clone(),
        }));
        set_play_icon(button, true);
        let generation = self.inline_generation.get() + 1;
        self.inline_generation.set(generation);

        let app = App::get();
        let client = app.client();
        let this = self.clone();
        let id = voicemail_id.to_string();
        glib::spawn_future_local(async move {
            let target = id.clone();
            let result = runtime::run(async move { client.voicemail(&target).await }).await;
            if this.inline_generation.get() != generation {
                return;
            }
            match result {
                Ok(voicemail) => {
                    let Some(url) = voicemail.audio_url.clone() else {
                        App::get().toast("This voicemail's audio isn't ready yet.");
                        this.stop_inline();
                        return;
                    };
                    this.player.load(&id, url, None, true);
                    let inline = this.inline.clone();
                    let player = this.player.clone();
                    let current_generation = this.inline_generation.clone();
                    // Follow the stream so the row's button shows play/pause correctly.
                    glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
                        if current_generation.get() != generation {
                            return glib::ControlFlow::Break;
                        }
                        let Some(current) = inline.borrow().as_ref().map(|i| i.button.clone())
                        else {
                            return glib::ControlFlow::Break;
                        };
                        match player.media() {
                            Some(media) => {
                                set_play_icon(&current, media.is_playing());
                                if media.is_ended() {
                                    set_play_icon(&current, false);
                                }
                                glib::ControlFlow::Continue
                            }
                            None => glib::ControlFlow::Continue,
                        }
                    });
                    if !voicemail.read {
                        App::get().mark_voicemail_read(&voicemail.voicemail_id);
                    }
                }
                Err(err) => {
                    App::get().error(&err);
                    this.stop_inline();
                }
            }
        });
    }
}

fn set_play_icon(button: &gtk::Button, playing: bool) {
    button.set_icon_name(if playing {
        "media-playback-pause-symbolic"
    } else {
        "media-playback-start-symbolic"
    });
    button.set_tooltip_text(Some(if playing { "Pause" } else { "Play" }));
}

thread_local! {
    static OPEN: RefCell<HashMap<String, adw::Dialog>> = RefCell::new(HashMap::new());
}

/// Fetches a voicemail (for its fresh audio URL) and opens it.
pub fn open_detail_by_id(voicemail_id: &str) {
    let client = App::get().client();
    let id = voicemail_id.to_string();
    glib::spawn_future_local(async move {
        match runtime::run(async move { client.voicemail(&id).await }).await {
            Ok(voicemail) => open_detail(voicemail),
            Err(err) => App::get().error(&err),
        }
    });
}

/// From, the reason, a player with a scrubber, the transcript, and Call back.
pub fn open_detail(voicemail: Voicemail) {
    if let Some(dialog) = OPEN.with(|o| o.borrow().get(&voicemail.voicemail_id).cloned()) {
        dialog.present(Some(&App::get().window().window));
        return;
    }
    let app = App::get();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 14);
    content.set_margin_top(12);
    content.set_margin_bottom(24);
    content.set_margin_start(20);
    content.set_margin_end(20);

    let (frame, _) = widgets::ringed_avatar(&voicemail.from, 88);
    content.append(&frame);
    content.append(&widgets::identity_block(&voicemail.from, "medium"));

    let meta = widgets::pixel(&format!(
        "{} · {}",
        reason_text(&voicemail),
        widgets::format_full(&voicemail.created_at)
    ));
    meta.add_css_class("muted");
    meta.set_halign(gtk::Align::Center);
    // GTK estimates this pixel face's width low, so a wrapping label would break early.
    meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&meta);

    let player = Player::new();
    content.append(player.widget());
    match (voicemail.audio_status, voicemail.audio_url.clone()) {
        (AudioStatus::Ready, Some(url)) => player.load(&voicemail.voicemail_id, url, None, false),
        // Listings carry no audio link; fetch the voicemail for a fresh presigned one.
        (AudioStatus::Ready, None) => {
            player.set_status("Loading…");
            let client = app.client();
            let id = voicemail.voicemail_id.clone();
            let player = player.clone();
            glib::spawn_future_local(async move {
                let target = id.clone();
                match runtime::run(async move { client.voicemail(&target).await }).await {
                    Ok(fresh) => match fresh.audio_url {
                        Some(url) => player.load(&id, url, None, false),
                        None => player.set_status("The audio is still processing"),
                    },
                    Err(err) => player.set_status(&widgets::error_text(&err)),
                }
            });
        }
        (AudioStatus::Failed, _) => player.set_status("The audio could not be saved"),
        _ => player.set_status("The audio is still processing"),
    }

    let heading = widgets::pixel("Transcript");
    heading.add_css_class("muted");
    heading.set_margin_top(8);
    content.append(&heading);
    let transcript = match (voicemail.transcript_status, voicemail.transcript.as_deref()) {
        (_, Some(text)) if !text.trim().is_empty() => text.to_string(),
        (TranscriptStatus::Pending, _) => "Transcribing…".to_string(),
        (TranscriptStatus::Failed, _) => "This message could not be transcribed.".to_string(),
        _ => "No words were detected.".to_string(),
    };
    let transcript = widgets::label(&transcript, &[]);
    transcript.set_selectable(true);
    content.append(&transcript);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    actions.set_halign(gtk::Align::Center);
    actions.set_margin_top(12);
    let call_back = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("call-start-symbolic")
                .label("Call back")
                .build(),
        )
        .build();
    call_back.add_css_class("suggested-action");
    call_back.add_css_class("pill");
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Delete")
        .build();
    delete.add_css_class("circular");
    actions.append(&call_back);
    actions.append(&delete);
    content.append(&actions);

    let header = adw::HeaderBar::new();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&content)
        .build();
    view.set_content(Some(&scroller));
    let dialog = adw::Dialog::builder()
        .title("Voicemail")
        .content_width(440)
        .child(&view)
        .build();

    let id = voicemail.voicemail_id.clone();
    let to = voicemail.from.id.clone();
    call_back.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
            App::get().start_call(to.clone());
        }
    ));
    let delete_id = id.clone();
    delete.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            let confirm = adw::AlertDialog::new(
                Some("Delete this voicemail?"),
                Some("It is removed from all your devices."),
            );
            confirm.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
            confirm.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
            confirm.set_close_response("cancel");
            let id = delete_id.clone();
            let detail = dialog.clone();
            confirm.connect_response(None, move |_, response| {
                if response == "delete" {
                    App::get().delete_voicemail(&id);
                    detail.close();
                }
            });
            confirm.present(Some(&dialog));
        }
    ));
    let closed_id = id.clone();
    dialog.connect_closed(move |_| {
        player.stop();
        OPEN.with(|o| o.borrow_mut().remove(&closed_id));
    });
    OPEN.with(|o| o.borrow_mut().insert(id.clone(), dialog.clone()));
    dialog.present(Some(&app.window().window));
    // Otherwise the selectable transcript takes the focus and opens fully selected.
    dialog.set_focus(Some(&call_back));
    if !voicemail.read {
        app.mark_voicemail_read(&id);
    }
}
