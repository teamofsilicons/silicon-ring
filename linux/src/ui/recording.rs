//! The recording notice (as on Apple, `RecordingNoticeView.swift`): Ring records and transcribes
//! calls, and which providers process the audio. It shows once after sign-in, and before a call
//! until this device agrees. "Agree and continue" (Enter) carries on with what waited for it;
//! "Not now" (Esc) drops it. Over a call that connected before this device agreed, with its
//! microphone off, agreeing turns the microphone on and "Not now" ends the call.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;

use crate::app::App;
use crate::ui::widgets;

pub const PRIVACY_URL: &str = "https://ring.teamofsilicons.com/privacy";

const TITLE: &str = "Ring records calls";
const AGREE: &str = "Agree and continue";
const NOT_NOW: &str = "Not now";
const PRIVACY_POLICY: &str = "Privacy Policy";
/// On the outgoing and in-call screens, and atop the notice.
const INDICATOR: &str = "Recorded \u{b7} Transcribed";

/// Over a call that connected before this device agreed, while its microphone is off.
pub const OVER_CALL: &str = "This call is recorded and transcribed, like every Ring call. Your microphone stays off until you agree; Not now ends the call.";
/// The same notice once that call has ended.
pub const AFTER_CALL: &str = "Your last call was recorded and transcribed, like every Ring call.";

/// A small red dot and "Recorded · Transcribed".
pub fn indicator() -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("recorded-dot");
    dot.set_valign(gtk::Align::Center);
    row.append(&dot);
    let label = widgets::pixel(INDICATOR);
    label.add_css_class("tiny");
    label.add_css_class("muted");
    row.append(&label);
    row.set_tooltip_text(Some("This call is recorded and transcribed"));
    row
}

fn item(icon: &str, text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let image = gtk::Image::from_icon_name(icon);
    image.add_css_class("notice-icon");
    image.set_valign(gtk::Align::Start);
    image.set_margin_top(2);
    row.append(&image);
    let label = widgets::label(text, &["notice-item"]);
    label.set_hexpand(true);
    row.append(&label);
    row
}

/// One notice on screen. The controller names it by `id`, so a notice it has already replaced or
/// answered cannot act again.
#[derive(Clone)]
pub struct NoticeDialog {
    dialog: adw::Dialog,
    context: gtk::Label,
    not_now: gtk::Button,
    /// Cleared once it closes or is being closed, so only the person closing it (Esc, or swiping
    /// a bottom sheet away) counts as "Not now".
    open: Rc<Cell<bool>>,
}

impl NoticeDialog {
    /// `context` is the sentence about a call that connected first; `not_now_tip` says what
    /// "Not now" does.
    pub fn new(id: u64, context: Option<&str>, not_now_tip: &str) -> NoticeDialog {
        let column = gtk::Box::new(gtk::Orientation::Vertical, 16);
        column.add_css_class("recording-notice");
        column.set_margin_top(24);
        column.set_margin_bottom(24);
        column.set_margin_start(24);
        column.set_margin_end(24);

        let indicator = indicator();
        indicator.set_halign(gtk::Align::Start);
        indicator.set_tooltip_text(None);
        column.append(&indicator);

        let title = widgets::display(TITLE, "large");
        title.set_xalign(0.0);
        title.set_justify(gtk::Justification::Left);
        column.append(&title);

        let context_label = widgets::label("", &["notice-context"]);
        column.append(&context_label);

        column.append(&widgets::label(
            "Every call is recorded and transcribed, so the people on it can read it and play it back later. Everyone on a call sees the same transcript.",
            &[],
        ));
        let items = gtk::Box::new(gtk::Orientation::Vertical, 10);
        items.append(&item(
            "audio-input-microphone-symbolic",
            "Deepgram transcribes what people say.",
        ));
        items.append(&item(
            "audio-speakers-symbolic",
            "When a silicon is on the call, OpenAI produces its voice and hears the call.",
        ));
        column.append(&items);
        column.append(&widgets::label(
            "Some places require everyone\u{2019}s consent to record a call. Let the people you call know.",
            &["muted"],
        ));

        let privacy = gtk::LinkButton::with_label(PRIVACY_URL, PRIVACY_POLICY);
        privacy.set_halign(gtk::Align::Start);
        column.append(&privacy);

        let not_now = gtk::Button::with_label(NOT_NOW);
        let agree = gtk::Button::with_label(AGREE);
        agree.add_css_class("suggested-action");
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        buttons.set_halign(gtk::Align::End);
        buttons.set_margin_top(4);
        buttons.append(&not_now);
        buttons.append(&agree);
        column.append(&buttons);

        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&column)
            .build();
        // Enter agrees (Agree has the focus and is the default); Esc is "Not now".
        let dialog = adw::Dialog::builder()
            .title(TITLE)
            .content_width(460)
            .child(&scroller)
            .default_widget(&agree)
            .focus_widget(&agree)
            .build();

        let open = Rc::new(Cell::new(true));
        agree.connect_clicked(move |_| App::get().agree_to_recording_notice(id));
        not_now.connect_clicked(move |_| App::get().dismiss_recording_notice(id));
        let closing = open.clone();
        dialog.connect_closed(move |_| {
            if closing.replace(false) {
                App::get().dismiss_recording_notice(id);
            }
        });

        let this = NoticeDialog {
            dialog,
            context: context_label,
            not_now,
            open,
        };
        this.set_context(context, not_now_tip);
        this
    }

    pub fn set_context(&self, context: Option<&str>, not_now_tip: &str) {
        self.context.set_label(context.unwrap_or_default());
        self.context.set_visible(context.is_some());
        self.not_now.set_tooltip_text(Some(not_now_tip));
    }

    pub fn present(&self, parent: &impl IsA<gtk::Widget>) {
        self.dialog.present(Some(parent));
    }

    /// Closes it without counting as an answer.
    pub fn close(&self) {
        if self.open.replace(false) {
            self.dialog.force_close();
        }
    }
}
